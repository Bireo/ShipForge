#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analyzer;
mod artifacts;
mod build_guard;
mod config;
mod credentials;
mod deploy;
mod detector;
mod git_ops;
mod guardrails;
mod health_report;
mod local_server;
mod models;
mod packager;
mod preflight;
mod presets;
mod repo_client;
mod runner;
mod storage;
mod toolchain;
mod transaction;
mod updater;
mod updater_host;
mod version;

use crate::local_server::LocalFeedServer;
use crate::models::*;
use crate::runner::{emit_log, emit_progress, TaskRunner};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, State};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Clone, Default)]
struct ProjectWatchSnapshot {
    fingerprint: String,
    version: String,
    artifact_fp: String,
    /// Per-project non-Git filesystem baseline (rel_path -> (size, mtime_secs, mtime_nanos))
    fs_snapshot: Option<HashMap<String, (u64, u64, u32)>>,
}

pub(crate) struct AppState {
    pub(crate) config: Mutex<AppConfig>,
    pub(crate) runner: Arc<TaskRunner>,
    pub(crate) server: Arc<Mutex<LocalFeedServer>>,
    /// Per-project strictly isolated watch snapshot for auto/manual change detection
    watch_state: Arc<Mutex<HashMap<String, ProjectWatchSnapshot>>>,
}

fn find_project(state: &State<AppState>, project_id: &str) -> Result<ProjectConfig, String> {
    let guard = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    guard
        .projects
        .iter()
        .find(|p| p.id == project_id)
        .cloned()
        .ok_or_else(|| format!("找不到项目：{}", project_id))
}

fn other_project_roots(state: &State<AppState>, exclude_project_id: &str) -> Vec<PathBuf> {
    state
        .config
        .lock()
        .ok()
        .map(|g| {
            g.projects
                .iter()
                .filter(|p| p.id != exclude_project_id)
                .map(|p| PathBuf::from(&p.root))
                .collect()
        })
        .unwrap_or_default()
}

/// Allocates a collision-free project ID that neither collides case-insensitively or by workspace key
/// with any active project NOR reuses an orphaned on-disk workspace directory belonging to a different root.
fn allocate_unique_project_id(existing: &[ProjectConfig], base_id: &str, candidate_root: &Path) -> String {
    let clean_base = if base_id.trim().is_empty() {
        "project"
    } else {
        base_id.trim()
    };
    let mut cand = clean_base.to_string();
    let mut suffix = 2usize;
    while existing.iter().any(|p| {
        p.id.eq_ignore_ascii_case(&cand)
            || storage::project_workspace_key(&p.id)
                .eq_ignore_ascii_case(&storage::project_workspace_key(&cand))
    }) || storage::workspace_owned_by_other_root(&cand, candidate_root)
    {
        cand = format!("{}-{}", clean_base, suffix);
        suffix += 1;
    }
    cand
}

fn ensure_root_exists(project: &ProjectConfig) -> Result<(), String> {
    if !Path::new(&project.root).is_dir() {
        return Err(format!("项目目录已不存在或被移动：{}", project.root));
    }
    Ok(())
}

/// Single source of truth for building a project's `ChangeReport` (including `deploy_readiness`),
/// shared by BOTH `get_project_overview` and `detect_project_changes` so overview & background
/// polling can NEVER diverge and cause spurious change-banner loops.
#[allow(clippy::too_many_arguments)]
fn build_change_report_from_parts(
    project: &ProjectConfig,
    root: &Path,
    versions: &VersionState,
    git: &GitStatus,
    engine_stale: bool,
    artifacts: &[ArtifactInfo],
    prev: Option<&ProjectWatchSnapshot>,
    other_roots: &[PathBuf],
) -> (ChangeReport, Option<HashMap<String, (u64, u64, u32)>>) {
    let active_artifacts: Vec<&ArtifactInfo> = artifacts.iter().filter(|a| !a.is_archived).collect();
    let artifact_summary = active_artifacts
        .iter()
        .map(|a| format!("{}:{}:{}", a.name, a.size, a.modified))
        .collect::<Vec<_>>()
        .join("|");

    let (scan_mode, raw_changes, non_git_fp, fs_snap_out) = if git.is_repo {
        let filtered: Vec<(String, String)> = git
            .changes
            .iter()
            .filter(|c| !storage::is_under_other_project(root, &c.path, other_roots))
            .map(|c| (c.status.clone(), c.path.clone()))
            .collect();
        ("git", filtered, None, None)
    } else {
        let (fs_fp, snap) = storage::scan_non_git_workspace(root, project, other_roots);
        // Diff against the persistent per-project build/seal baseline (so auto-watch refresh never erases
        // detected local changes on the KPI card!), falling back to watch_state or seeding baseline on first scan.
        let baseline = storage::load_fs_baseline(&project.id).or_else(|| {
            if let Some(old) = prev.and_then(|p| p.fs_snapshot.as_ref()) {
                storage::save_fs_baseline(&project.id, old);
                Some(old.clone())
            } else {
                storage::save_fs_baseline(&project.id, &snap);
                None
            }
        });
        let diffs = baseline
            .as_ref()
            .map(|old| storage::diff_non_git_snapshots(old, &snap))
            .unwrap_or_default();
        ("filesystem", diffs, Some(fs_fp), Some(snap))
    };

    let mut rep = storage::build_change_report_classified(
        project,
        root,
        &versions.current,
        &git.head_sha,
        scan_mode,
        raw_changes,
        non_git_fp.as_deref(),
        engine_stale,
        active_artifacts.len(),
        &artifact_summary,
        prev.map(|p| p.fingerprint.as_str()),
        prev.map(|p| p.version.as_str()),
        prev.map(|p| p.artifact_fp.as_str()),
    );
    rep.deploy_readiness = Some(deploy::inspect_deploy_readiness(
        project,
        versions,
        git,
        artifacts,
        &rep,
        engine_stale,
    ));
    (rep, fs_snap_out)
}

fn compute_change_report_for_project(
    project: &ProjectConfig,
    prev: Option<&ProjectWatchSnapshot>,
    other_roots: &[PathBuf],
) -> (ChangeReport, Option<HashMap<String, (u64, u64, u32)>>) {
    let root = Path::new(&project.root);
    let versions = version::read_project_versions(root, &project.version_files);
    let mut git = git_ops::inspect_git_for_project(root, &project.release_dir);
    if git.is_repo && !other_roots.is_empty() {
        git.changes
            .retain(|c| !storage::is_under_other_project(root, &c.path, other_roots));
        git.dirty_count = git.changes.len();
    }
    let engine_stale =
        build_guard::check_engine_stale(root, &project.engine_src_dir, &project.engine_exe_path)
            .map(|e| e.stale)
            .unwrap_or(false);
    let release_dir = config::resolve_release_dir(project);
    let artifacts = packager::scan_artifacts(&release_dir);
    build_change_report_from_parts(
        project,
        root,
        &versions,
        &git,
        engine_stale,
        &artifacts,
        prev,
        other_roots,
    )
}

fn record_build_into_version_bucket(project: &ProjectConfig, built_version: &str, other_roots: &[PathBuf]) {
    save_version_bucket_snapshot(project, built_version, true, other_roots);
}

/// Refreshes the version bucket fingerprint after post-build pipeline steps (manifest / git commit)
/// WITHOUT incrementing `build_count`, preventing false "检测到连续修改" warnings right after a clean pipeline!
fn sync_version_bucket_snapshot(project: &ProjectConfig, built_version: &str, other_roots: &[PathBuf]) {
    save_version_bucket_snapshot(project, built_version, false, other_roots);
}

fn save_version_bucket_snapshot(
    project: &ProjectConfig,
    built_version: &str,
    increment_count: bool,
    other_roots: &[PathBuf],
) {
    let root = Path::new(&project.root);
    let git = git_ops::inspect_git_for_project(root, &project.release_dir);
    let release_dir = config::resolve_release_dir(project);
    let artifacts = packager::scan_artifacts(&release_dir);
    let active: Vec<&ArtifactInfo> = artifacts.iter().filter(|a| !a.is_archived).collect();
    let artifact_summary = active
        .iter()
        .map(|a| format!("{}:{}:{}", a.name, a.size, a.modified))
        .collect::<Vec<_>>()
        .join("|");
    let (dirty_files, non_git_fp) = if git.is_repo {
        let files = git
            .changes
            .iter()
            .filter(|c| !storage::is_under_other_project(root, &c.path, other_roots))
            .map(|c| c.path.clone())
            .collect();
        (files, None)
    } else {
        let (fs_fp, snap) = storage::scan_non_git_workspace(root, project, other_roots);
        // Refresh persistent non-Git baseline right after a build/pipeline sync so the freshly built
        // version starts clean with zero stale diff noise.
        storage::save_fs_baseline(&project.id, &snap);
        (vec![], Some(fs_fp))
    };
    let fp = storage::compute_workspace_fingerprint_ext(
        built_version,
        &git.head_sha,
        &dirty_files,
        root,
        &artifact_summary,
        non_git_fp.as_deref(),
    );

    let mut bucket = storage::load_version_bucket(&project.id, built_version);
    if bucket.base_git_sha.is_empty() {
        bucket.base_git_sha = git.head_sha.clone();
    }
    bucket.last_built_git_sha = git.head_sha;
    bucket.last_built_fingerprint = fp;
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    if increment_count || bucket.last_built_at.is_empty() {
        bucket.last_built_at = now.clone();
    }
    if increment_count {
        bucket.build_count = bucket.build_count.saturating_add(1);
    }
    if !bucket.sealed {
        bucket.changed_files = dirty_files;
    }
    for a in active {
        if a.version.as_deref() == Some(built_version) {
            if let Ok(h) = packager::sha256_file(Path::new(&a.path)) {
                bucket.artifact_hashes.insert(a.name.clone(), h);
            }
        }
    }
    bucket.updated_at = now;
    let _ = storage::save_version_bucket(&project.id, &bucket);
}

#[tauri::command]
fn get_app_state(state: State<AppState>) -> Result<AppConfig, String> {
    let guard = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    Ok(guard.clone())
}

#[tauri::command]
fn set_auto_watch(state: State<AppState>, enabled: bool) -> Result<AppConfig, String> {
    let mut guard = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    guard.auto_watch = enabled;
    config::save_config(&guard).map_err(|e| e.to_string())?;
    Ok(guard.clone())
}

#[tauri::command]
fn set_scan_roots(state: State<AppState>, scan_roots: Vec<String>) -> Result<AppConfig, String> {
    let mut guard = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    let mut deduped: Vec<String> = Vec::new();
    for raw in scan_roots {
        let trimmed = raw.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        if !deduped.iter().any(|s| s.eq_ignore_ascii_case(&trimmed)) {
            deduped.push(trimmed);
        }
    }
    guard.scan_roots = deduped;
    config::save_config(&guard).map_err(|e| e.to_string())?;
    Ok(guard.clone())
}

#[tauri::command]
async fn discover_workspace_projects(
    state: State<'_, AppState>,
    scan_dir: Option<String>,
    save_root: Option<bool>,
) -> Result<Vec<DiscoveredProject>, String> {
    let (existing_roots, mut configured_roots) = {
        let mut guard = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
        if let (Some(ref dir), true) = (&scan_dir, save_root.unwrap_or(false)) {
            let trimmed = dir.trim().to_string();
            if !trimmed.is_empty()
                && Path::new(&trimmed).is_dir()
                && !guard.scan_roots.iter().any(|r| r.eq_ignore_ascii_case(&trimmed))
            {
                guard.scan_roots.push(trimmed);
                let _ = config::save_config(&guard);
            }
        }
        (
            guard.projects.iter().map(|p| p.root.clone()).collect::<Vec<_>>(),
            guard.scan_roots.clone(),
        )
    };

    let roots: Vec<PathBuf> = if let Some(dir) = scan_dir.filter(|s| !s.trim().is_empty()) {
        let p = PathBuf::from(dir.trim());
        if !p.is_dir() {
            return Err(format!("扫描目录不存在：{}", p.display()));
        }
        vec![p]
    } else {
        if configured_roots.is_empty() {
            if let Ok(exe) = std::env::current_exe() {
                if let Some(parent) = exe.parent().and_then(|d| d.parent()) {
                    let lower = parent.to_string_lossy().to_ascii_lowercase();
                    if !lower.contains("program files")
                        && !lower.contains("\\windows\\")
                        && !lower.contains("\\appdata\\")
                        && parent.parent().is_some()
                    {
                        configured_roots.push(parent.to_string_lossy().to_string());
                    }
                }
            }
        }
        configured_roots
            .into_iter()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .collect()
    };

    if roots.is_empty() {
        return Ok(vec![]);
    }

    tauri::async_runtime::spawn_blocking(move || {
        detector::discover_projects_in_roots(&roots, &existing_roots, 2)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn import_projects_batch(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<AppConfig, String> {
    let detected: Vec<ProjectConfig> = tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        for p in paths {
            let trimmed = p.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(proj) = detector::detect_project(Path::new(trimmed)) {
                out.push(proj);
            }
        }
        out
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut guard = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    for mut proj in detected {
        let cand_key = storage::normalize_root_key(Path::new(&proj.root));
        if guard
            .projects
            .iter()
            .any(|p| storage::normalize_root_key(Path::new(&p.root)) == cand_key)
        {
            continue;
        }
        proj.id = allocate_unique_project_id(&guard.projects, &proj.id, Path::new(&proj.root));
        storage::bind_workspace_owner(&proj.id, Path::new(&proj.root));
        let new_id = proj.id.clone();
        guard.projects.push(proj);
        if guard.active_project_id.is_none() {
            guard.active_project_id = Some(new_id);
        }
    }
    config::save_config(&guard).map_err(|e| e.to_string())?;
    Ok(guard.clone())
}

#[tauri::command]
fn detect_project_github_repo(state: State<AppState>, project_id: String) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    Ok(detector::detect_github_repo(Path::new(&project.root)))
}

#[tauri::command]
fn launch_gh_auth(mode: String) -> Result<String, String> {
    git_ops::launch_gh_auth_interactive(&mode).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_active_project(state: State<AppState>, project_id: String) -> Result<AppConfig, String> {
    let mut guard = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    if !guard.projects.iter().any(|p| p.id == project_id) {
        return Err("项目不存在".into());
    }
    guard.active_project_id = Some(project_id);
    config::save_config(&guard).map_err(|e| e.to_string())?;
    Ok(guard.clone())
}

#[tauri::command]
async fn pick_folder(title: Option<String>) -> Option<String> {
    let t = title.unwrap_or_else(|| "选择软件项目根目录".into());
    tauri::async_runtime::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_title(&t)
            .pick_folder()
            .map(|p| p.to_string_lossy().to_string())
    })
    .await
    .ok()
    .flatten()
}

#[tauri::command]
async fn add_project_by_path(
    state: State<'_, AppState>,
    path: String,
) -> Result<ProjectConfig, String> {
    let p_clone = path.trim().to_string();
    let mut detected =
        tauri::async_runtime::spawn_blocking(move || detector::detect_project(Path::new(&p_clone)))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;

    let mut guard = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    let cand_key = storage::normalize_root_key(Path::new(&detected.root));
    if let Some(existing) = guard
        .projects
        .iter()
        .find(|p| storage::normalize_root_key(Path::new(&p.root)) == cand_key)
    {
        let cloned = existing.clone();
        guard.active_project_id = Some(cloned.id.clone());
        config::save_config(&guard).map_err(|e| e.to_string())?;
        return Ok(cloned);
    }

    detected.id = allocate_unique_project_id(&guard.projects, &detected.id, Path::new(&detected.root));
    storage::bind_workspace_owner(&detected.id, Path::new(&detected.root));

    guard.active_project_id = Some(detected.id.clone());
    guard.projects.push(detected.clone());
    config::save_config(&guard).map_err(|e| e.to_string())?;
    Ok(detected)
}

fn merge_rescanned_project(existing: &ProjectConfig, mut detected: ProjectConfig) -> ProjectConfig {
    detected.id = existing.id.clone();
    if !existing.github_repo.is_empty() && detected.github_repo.is_empty() {
        detected.github_repo = existing.github_repo.clone();
    }
    // Preserve user's explicit multi-repo binding (provider / base_url / slug / credential_id)
    if !existing.repo_binding.repo_slug.trim().is_empty()
        || !existing.repo_binding.credential_id.trim().is_empty()
        || !existing.repo_binding.base_url.trim().is_empty()
    {
        detected.repo_binding = existing.repo_binding.clone();
    }
    // Preserve custom package profiles, retention count, and custom env overrides
    if !existing.package_profiles.is_empty() {
        detected.package_profiles = existing.package_profiles.clone();
    }
    if existing.keep_releases >= 1 {
        detected.keep_releases = existing.keep_releases;
    }
    for (k, v) in &existing.env {
        detected.env.insert(k.clone(), v.clone());
    }
    // Preserve user-defined custom actions without clobbering freshly detected preset actions
    for act in &existing.actions {
        let is_user_custom = act.id.starts_with("custom-") || act.category == "custom";
        if is_user_custom && !detected.actions.iter().any(|d| d.id == act.id) {
            detected.actions.push(act.clone());
        }
    }
    detected
}

fn validate_project_config(project: &mut ProjectConfig) -> Result<(), String> {
    if project.id.trim().is_empty() {
        return Err("项目 ID 不能为空".into());
    }
    if project.name.trim().is_empty() {
        return Err("项目显示名称不能为空".into());
    }
    if project.root.trim().is_empty() {
        return Err("项目根目录不能为空".into());
    }
    let root = Path::new(project.root.trim());
    project.keep_releases = project.keep_releases.clamp(1, 50);

    let rel_dir = project.release_dir.trim();
    if !rel_dir.is_empty() {
        guardrails::resolve_confined_path(root, rel_dir)
            .map_err(|e| format!("发布目录路径非法（仅允许项目根目录内相对路径）：{}", e))?;
    }
    let cl_path = project.changelog_path.trim();
    if !cl_path.is_empty() {
        guardrails::resolve_confined_path(root, cl_path)
            .map_err(|e| format!("变更日志路径非法（仅允许项目根目录内相对路径）：{}", e))?;
    }
    let exe_path = project.launch_exe.trim();
    if !exe_path.is_empty() {
        guardrails::resolve_confined_path(root, exe_path)
            .map_err(|e| format!("运行副本路径非法（仅允许项目根目录内相对路径）：{}", e))?;
    }
    let eng_src = project.engine_src_dir.trim();
    if !eng_src.is_empty() {
        guardrails::resolve_confined_path(root, eng_src)
            .map_err(|e| format!("子引擎源码路径非法（仅允许项目根目录内相对路径）：{}", e))?;
    }
    let eng_exe = project.engine_exe_path.trim();
    if !eng_exe.is_empty() {
        guardrails::resolve_confined_path(root, eng_exe)
            .map_err(|e| format!("子引擎产物路径非法（仅允许项目根目录内相对路径）：{}", e))?;
    }
    let lock_path = project.lock_file.trim();
    if !lock_path.is_empty() {
        if lock_path.starts_with("\\\\")
            || lock_path.contains("..")
            || lock_path.chars().any(|c| c.is_control())
        {
            return Err("外部构建锁路径包含非法字符或 UNC/越界路径".into());
        }
        let lower_name = Path::new(lock_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !lower_name.contains("lock") {
            return Err("外部构建锁文件名必须包含 lock（防误删非锁文件）".into());
        }
    }
    let journal_path = project.journal_file.trim();
    if !journal_path.is_empty() {
        if journal_path.starts_with("\\\\")
            || journal_path.contains("..")
            || journal_path.chars().any(|c| c.is_control())
        {
            return Err("外部构建台账路径包含非法字符或 UNC/越界路径".into());
        }
        if !journal_path.to_ascii_lowercase().ends_with(".jsonl") {
            return Err("外部构建台账文件须为 .jsonl 格式".into());
        }
    }
    if !project.github_repo.trim().is_empty() {
        repo_client::validate_repo_slug(&project.github_repo)
            .map_err(|e| format!("GitHub 仓库格式非法：{}", e))?;
    }
    if !project.repo_binding.repo_slug.trim().is_empty() {
        repo_client::validate_repo_slug(&project.repo_binding.repo_slug)
            .map_err(|e| format!("绑定仓库格式非法：{}", e))?;
    }
    for vf in &project.version_files {
        guardrails::resolve_confined_path(root, &vf.path)
            .map_err(|e| format!("版本文件路径非法（{}）：{}", vf.path, e))?;
        if vf.format == "regex" {
            if vf.pattern.trim().is_empty() {
                return Err(format!("版本文件 {} 使用 regex 格式时必须提供正则表达式", vf.path));
            }
            regex::Regex::new(&vf.pattern)
                .map_err(|e| format!("版本文件 {} 的正则表达式无效：{}", vf.path, e))?;
        }
    }
    for act in &project.actions {
        if !act.cwd.trim().is_empty() {
            guardrails::resolve_confined_path(root, &act.cwd)
                .map_err(|e| format!("工序 {} 的子目录越界或非法（{}）：{}", act.label, act.cwd, e))?;
        }
    }
    Ok(())
}

fn is_path_within_allowed_roots(state: &State<AppState>, target: &Path) -> Result<PathBuf, String> {
    let raw = target.to_string_lossy();
    if raw.trim().is_empty() || raw.starts_with("\\\\") || raw.chars().any(|c| c.is_control()) {
        return Err("路径无效或禁止访问 UNC 网络路径".into());
    }
    if !target.exists() {
        return Err(format!("路径不存在：{}", raw));
    }
    let canon_target = target
        .canonicalize()
        .map_err(|e| format!("无法解析路径：{}", e))?;

    let mut allowed: Vec<PathBuf> = Vec::new();
    let data_root = storage::active_data_root();
    if let Ok(c) = data_root.canonicalize() {
        allowed.push(c);
    }
    if let Ok(cfg) = state.config.lock() {
        for p in &cfg.projects {
            if let Ok(c) = Path::new(&p.root).canonicalize() {
                allowed.push(c);
            }
            let rdir = config::resolve_release_dir(p);
            if let Ok(c) = rdir.canonicalize() {
                allowed.push(c);
            }
        }
    }

    if allowed.iter().any(|root| canon_target.starts_with(root)) {
        Ok(canon_target)
    } else {
        Err("安全护栏：仅允许访问已导入项目目录或 ShipForge 数据目录内的路径".into())
    }
}

#[tauri::command]
async fn rescan_project(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<ProjectConfig, String> {
    let existing = find_project(&state, &project_id)?;
    let root_str = existing.root.clone();
    let detected =
        tauri::async_runtime::spawn_blocking(move || detector::detect_project(Path::new(&root_str)))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;

    let merged = merge_rescanned_project(&existing, detected);
    let mut guard = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    if let Some(slot) = guard.projects.iter_mut().find(|p| p.id == project_id) {
        *slot = merged.clone();
    }
    config::save_config(&guard).map_err(|e| e.to_string())?;
    Ok(merged)
}

#[tauri::command]
fn update_project_config(
    state: State<AppState>,
    mut project: ProjectConfig,
) -> Result<ProjectConfig, String> {
    validate_project_config(&mut project)?;
    let new_root_key = storage::normalize_root_key(Path::new(&project.root));
    let root_changed = {
        let mut guard = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
        if guard.projects.iter().any(|p| {
            p.id != project.id
                && storage::normalize_root_key(Path::new(&p.root)) == new_root_key
        }) {
            return Err("该根目录已绑定到其他项目，禁止多个项目指向同一目录造成数据混淆".into());
        }
        let Some(slot) = guard.projects.iter_mut().find(|p| p.id == project.id) else {
            return Err("找不到要更新的项目".into());
        };
        let old_root_key = storage::normalize_root_key(Path::new(&slot.root));
        let changed = old_root_key != new_root_key;
        if changed && !Path::new(&project.root).is_dir() {
            return Err(format!("新的项目根目录不存在：{}", project.root));
        }
        // Keep `repo_binding.repo_slug` and `github_repo` consistently synchronized without split-brain
        let gh_trimmed = project.github_repo.trim().to_string();
        let binding_prov = project.repo_binding.provider.trim().to_ascii_lowercase();
        if binding_prov.is_empty() || binding_prov == "github" {
            if gh_trimmed != slot.github_repo.trim() {
                if binding_prov.is_empty() && !gh_trimmed.is_empty() {
                    project.repo_binding.provider = "github".into();
                }
                project.repo_binding.repo_slug = gh_trimmed.clone();
            }
            project.github_repo = gh_trimmed;
        } else {
            // Non-GitHub binding (gitee/gitlab/gitea): keep github_repo cleared and validate repo_slug
            if !project.repo_binding.repo_slug.trim().is_empty() {
                repo_client::validate_repo_slug(&project.repo_binding.repo_slug)
                    .map_err(|e| format!("绑定仓库格式非法：{}", e))?;
            }
            project.github_repo.clear();
        }
        *slot = project.clone();
        config::save_config(&guard).map_err(|e| e.to_string())?;
        changed
    };
    if root_changed {
        if let Ok(mut ws) = state.watch_state.lock() {
            ws.remove(&project.id);
        }
        storage::clear_fs_baseline(&project.id);
        storage::bind_workspace_owner(&project.id, Path::new(&project.root));
    }
    Ok(project)
}

#[tauri::command]
fn remove_project(state: State<AppState>, project_id: String) -> Result<AppConfig, String> {
    if let Some(tid) = state.runner.is_project_busy(&project_id) {
        return Err(format!(
            "该项目正在执行任务（{}），请等待完成或先点击「停止」后再移出列表",
            tid
        ));
    }
    let mut guard = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    guard.projects.retain(|p| p.id != project_id);
    if guard.active_project_id.as_deref() == Some(&project_id) {
        guard.active_project_id = guard.projects.first().map(|p| p.id.clone());
    }
    config::save_config(&guard).map_err(|e| e.to_string())?;
    if let Ok(mut ws) = state.watch_state.lock() {
        ws.remove(&project_id);
    }
    if let Ok(mut srv) = state.server.lock() {
        if srv.status().project_id == project_id {
            srv.stop();
        }
    }
    Ok(guard.clone())
}

#[tauri::command]
async fn get_project_overview(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<ProjectOverview, String> {
    let project = find_project(&state, &project_id)?;
    let prev = state
        .watch_state
        .lock()
        .ok()
        .and_then(|g| g.get(&project.id).cloned());
    let other_roots = other_project_roots(&state, &project.id);

    let (ov, next_fs_snap) = tauri::async_runtime::spawn_blocking(move || {
        let root = Path::new(&project.root);
        let root_exists = root.is_dir();
        if !root_exists {
            let empty_bucket = storage::load_version_bucket(&project.id, "0.0.0");
            return (
                ProjectOverview {
                    versions: VersionState {
                        current: "0.0.0".into(),
                        synced: false,
                        readings: vec![],
                    },
                    git: GitStatus {
                        is_repo: false,
                        branch: String::new(),
                        head_sha: String::new(),
                        remote_url: String::new(),
                        dirty_count: 0,
                        ahead_count: 0,
                        behind_count: 0,
                        has_upstream: false,
                        changes: vec![],
                        recent_commits: vec![],
                        tags: vec![],
                        last_tag: None,
                    },
                    lock: None,
                    engine_stale: None,
                    artifacts: vec![],
                    recent_builds: vec![],
                    launch_exe_exists: false,
                    launch_exe_size: 0,
                    health_issues: vec![format!("项目根目录不存在或已被移动：{}", project.root)],
                    change_report: ChangeReport {
                        project_id: project.id.clone(),
                        project_name: project.name.clone(),
                        scan_mode: "filesystem".into(),
                        current_version: "0.0.0".into(),
                        head_sha: String::new(),
                        dirty_count: 0,
                        dirty_files: vec![],
                        classified_changes: vec![],
                        category_counts: HashMap::new(),
                        change_summary: String::new(),
                        artifact_changed: false,
                        source_fingerprint: String::new(),
                        artifact_fingerprint: String::new(),
                        engine_stale: false,
                        artifact_count: 0,
                        fingerprint: String::new(),
                        version_changed: false,
                        has_external_change: false,
                        changed_since_last_build: false,
                        consecutive_warning: String::new(),
                        bucket: empty_bucket,
                        deploy_readiness: None,
                    },
                    project,
                    root_exists: false,
                },
                None,
            );
        }

        storage::bind_workspace_owner(&project.id, root);

        let release_dir = config::resolve_release_dir(&project);
        let journal_p = config::journal_path();
        let (versions, mut git, lock, engine_stale, artifacts, recent_builds) =
            std::thread::scope(|s| {
                let h_ver = s.spawn(|| version::read_project_versions(root, &project.version_files));
                let h_git = s.spawn(|| git_ops::inspect_git_for_project(root, &project.release_dir));
                let h_lock = s.spawn(|| build_guard::inspect_lock(&project.lock_file));
                let h_eng = s.spawn(|| {
                    build_guard::check_engine_stale(
                        root,
                        &project.engine_src_dir,
                        &project.engine_exe_path,
                    )
                });
                let h_art = s.spawn(|| packager::scan_artifacts(&release_dir));
                let h_jrn = s.spawn(|| {
                    build_guard::read_journal_entries(
                        &project.journal_file,
                        &project.root,
                        &journal_p,
                        &project.id,
                        30,
                    )
                });
                (
                    h_ver.join().unwrap_or(VersionState {
                        current: "0.0.0".into(),
                        synced: false,
                        readings: vec![],
                    }),
                    h_git.join().unwrap_or(GitStatus {
                        is_repo: false,
                        branch: String::new(),
                        head_sha: String::new(),
                        remote_url: String::new(),
                        dirty_count: 0,
                        ahead_count: 0,
                        behind_count: 0,
                        has_upstream: false,
                        changes: vec![],
                        recent_commits: vec![],
                        tags: vec![],
                        last_tag: None,
                    }),
                    h_lock.join().ok().flatten(),
                    h_eng.join().ok().flatten(),
                    h_art.join().unwrap_or_default(),
                    h_jrn.join().unwrap_or_default(),
                )
            });

        // Strictly filter out any nested sibling projects from Git status so monorepo/nested projects stay isolated
        if git.is_repo && !other_roots.is_empty() {
            git.changes
                .retain(|c| !storage::is_under_other_project(root, &c.path, &other_roots));
            git.dirty_count = git.changes.len();
        }

        let (change_report, fs_snap_out) = build_change_report_from_parts(
            &project,
            root,
            &versions,
            &git,
            engine_stale.as_ref().map(|e| e.stale).unwrap_or(false),
            &artifacts,
            prev.as_ref(),
            &other_roots,
        );

        let (launch_exe_exists, launch_exe_size) = if project.launch_exe.trim().is_empty() {
            (false, 0)
        } else {
            let p = root.join(&project.launch_exe);
            match std::fs::metadata(&p) {
                Ok(m) => (true, m.len()),
                Err(_) => (false, 0),
            }
        };

        let mut health_issues = Vec::new();
        if !project.version_files.is_empty() && !versions.synced {
            health_issues.push("多处版本文件版本号不一致，建议先在「版本日志」同步".into());
        }
        if let Some(ref lk) = lock {
            if lk.exists && lk.alive {
                health_issues.push(format!("构建锁占用中（PID {} / 版本 {}）", lk.pid, lk.version));
            } else if lk.exists && !lk.alive {
                health_issues.push(format!("检测到残留死锁（PID {} 已退出），可一键清理", lk.pid));
            }
        }
        if let Some(ref es) = engine_stale {
            if es.stale {
                health_issues.push(format!(
                    "子引擎已过期（{} 等 {} 个源文件较新）",
                    es.newest_file, es.newer_count
                ));
            }
        }
        if !change_report.consecutive_warning.is_empty() {
            health_issues.push(change_report.consecutive_warning.clone());
        }

        (
            ProjectOverview {
                project,
                root_exists: true,
                versions,
                git,
                lock,
                engine_stale,
                artifacts,
                recent_builds,
                launch_exe_exists,
                launch_exe_size,
                health_issues,
                change_report,
            },
            fs_snap_out,
        )
    })
    .await
    .map_err(|e| e.to_string())?;

    if let Ok(mut ws) = state.watch_state.lock() {
        ws.insert(
            ov.project.id.clone(),
            ProjectWatchSnapshot {
                fingerprint: ov.change_report.fingerprint.clone(),
                version: ov.change_report.current_version.clone(),
                artifact_fp: ov.change_report.artifact_fingerprint.clone(),
                fs_snapshot: next_fs_snap,
            },
        );
    }

    Ok(ov)
}

#[tauri::command]
async fn detect_project_changes(
    state: State<'_, AppState>,
    project_id: String,
    acknowledge: Option<bool>,
) -> Result<ChangeReport, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let prev = state
        .watch_state
        .lock()
        .ok()
        .and_then(|g| g.get(&project.id).cloned());
    let other_roots = other_project_roots(&state, &project.id);
    let pid = project.id.clone();

    let (report, next_fs_snap) = tauri::async_runtime::spawn_blocking(move || {
        compute_change_report_for_project(&project, prev.as_ref(), &other_roots)
    })
    .await
    .map_err(|e| e.to_string())?;

    if acknowledge.unwrap_or(false) {
        if let Ok(mut ws) = state.watch_state.lock() {
            ws.insert(
                pid,
                ProjectWatchSnapshot {
                    fingerprint: report.fingerprint.clone(),
                    version: report.current_version.clone(),
                    artifact_fp: report.artifact_fingerprint.clone(),
                    fs_snapshot: next_fs_snap,
                },
            );
        }
    }

    Ok(report)
}

#[tauri::command]
fn get_version_buckets(state: State<AppState>, project_id: String) -> Result<Vec<VersionBucket>, String> {
    let project = find_project(&state, &project_id)?;
    Ok(storage::list_version_buckets(&project.id))
}

#[tauri::command]
fn save_version_draft(
    state: State<AppState>,
    project_id: String,
    version_str: String,
    notes_draft: String,
) -> Result<VersionBucket, String> {
    let project = find_project(&state, &project_id)?;
    let mut b = storage::load_version_bucket(&project.id, version_str.trim());
    b.notes_draft = notes_draft;
    b.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    storage::save_version_bucket(&project.id, &b).map_err(|e| e.to_string())?;
    Ok(b)
}

#[tauri::command]
fn seal_version_bucket(
    state: State<AppState>,
    project_id: String,
    version_str: String,
    sealed: bool,
) -> Result<VersionBucket, String> {
    let project = find_project(&state, &project_id)?;
    let mut b = storage::load_version_bucket(&project.id, version_str.trim());
    b.sealed = sealed;
    if sealed && b.build_count == 0 {
        let root = Path::new(&project.root);
        if root.is_dir() && !root.join(".git").exists() {
            let other_roots = other_project_roots(&state, &project.id);
            let (_, snap) = storage::scan_non_git_workspace(root, &project, &other_roots);
            storage::save_fs_baseline(&project.id, &snap);
        }
    }
    b.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    storage::save_version_bucket(&project.id, &b).map_err(|e| e.to_string())?;
    Ok(b)
}

// =========================================================================
// Custom Storage Directory + Safe Atomic Migration & Old Folder Removal
// =========================================================================

#[tauri::command]
async fn get_storage_info() -> StorageInfo {
    tauri::async_runtime::spawn_blocking(storage::get_storage_info)
        .await
        .unwrap_or(StorageInfo {
            data_root: String::new(),
            config_dir: String::new(),
            tools_dir: String::new(),
            cache_dir: String::new(),
            is_ascii: true,
            is_c_drive: false,
            total_bytes: 0,
            managed_tool_count: 0,
        })
}

#[tauri::command]
async fn migrate_storage_root(
    app: AppHandle,
    state: State<'_, AppState>,
    new_root: String,
) -> Result<StorageInfo, String> {
    if state.runner.has_any_running() {
        return Err("有构建或修复任务正在运行，请先停止后再迁移数据目录".into());
    }
    if let Ok(srv) = state.server.lock() {
        if srv.status().running {
            return Err("本机联调更新源正在运行，请先停止服务后再迁移目录".into());
        }
    }

    let project_roots: Vec<String> = state
        .config
        .lock()
        .map(|g| g.projects.iter().map(|p| p.root.clone()).collect())
        .unwrap_or_default();

    let task_id = format!("mig-{}", chrono::Local::now().timestamp_millis());
    runner::register_task_project(&task_id, "__system__");
    let lock_guard = state
        .runner
        .acquire_project_guard("__storage__", &task_id)
        .map_err(|e| e.to_string())?;
    let app_c = app.clone();
    let tid = task_id.clone();

    let info = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<StorageInfo> {
        let _lock_guard = lock_guard;
        storage::migrate_storage_root(&new_root, |pct, msg| {
            let scaled = ((pct as u16 * 70) / 100).min(70) as u8;
            emit_log(&app_c, &tid, "system", &format!("[迁移 {}%] {}", scaled, msg));
            emit_progress(&app_c, &tid, "迁移数据与工具目录", "目录迁移", msg, scaled, false);
        })?;

        let (final_info, adopted) = storage::consolidate_tools_and_caches(&project_roots, |pct, msg| {
            let scaled = (70 + (pct as u16 * 30) / 100).min(100) as u8;
            emit_log(&app_c, &tid, "system", &format!("[收归 {}%] {}", scaled, msg));
            emit_progress(&app_c, &tid, "迁移数据与工具目录", "收归工具与依赖缓存", msg, scaled, false);
        })?;
        for line in adopted {
            emit_log(&app_c, &tid, "stdout", &format!("✓ {}", line));
        }
        Ok(final_info)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    // Reload AppConfig from new location
    let reloaded = config::load_config();
    if let Ok(mut guard) = state.config.lock() {
        *guard = reloaded;
    }

    Ok(info)
}

#[tauri::command]
async fn consolidate_tools_and_caches(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    if state.runner.has_any_running() {
        return Err("有构建或修复任务正在运行，请先停止后再收归工具与缓存".into());
    }
    let project_roots: Vec<String> = state
        .config
        .lock()
        .map(|g| g.projects.iter().map(|p| p.root.clone()).collect())
        .unwrap_or_default();

    let task_id = format!("adopt-{}", chrono::Local::now().timestamp_millis());
    runner::register_task_project(&task_id, "__system__");
    let lock_guard = state
        .runner
        .acquire_project_guard("__storage__", &task_id)
        .map_err(|e| e.to_string())?;
    let app_c = app.clone();
    let tid = task_id.clone();

    let (info, summary) = tauri::async_runtime::spawn_blocking(move || {
        let _lock_guard = lock_guard;
        storage::consolidate_tools_and_caches(&project_roots, |pct, msg| {
            emit_log(&app_c, &tid, "system", &format!("[收归 {}%] {}", pct, msg));
            emit_progress(&app_c, &tid, "收归工具与依赖缓存", "收归迁移", msg, pct, false);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    for line in &summary {
        emit_log(&app, &task_id, "stdout", &format!("✓ {}", line));
    }

    Ok(serde_json::json!({
        "storage": info,
        "summary": summary,
    }))
}

// =========================================================================
// Toolchain Scanning & One-Click Automated Download / Repair
// =========================================================================

#[tauri::command]
async fn scan_toolchains() -> Vec<ToolchainItem> {
    tauri::async_runtime::spawn_blocking(toolchain::scan_toolchains)
        .await
        .unwrap_or_default()
}

#[tauri::command]
async fn repair_toolchain(
    app: AppHandle,
    state: State<'_, AppState>,
    tool_id: String,
) -> Result<ToolchainItem, String> {
    let task_id = format!("repair-{}-{}", tool_id, chrono::Local::now().timestamp_millis());
    runner::register_task_project(&task_id, "__system__");
    let lock_guard = state
        .runner
        .acquire_project_guard("__toolchain__", &task_id)
        .map_err(|e| e.to_string())?;

    let project_roots: Vec<String> = state
        .config
        .lock()
        .map(|g| g.projects.iter().map(|p| p.root.clone()).collect())
        .unwrap_or_default();

    let tid = task_id.clone();
    let tool_c = tool_id.clone();
    let app_c = app.clone();

    let res = tauri::async_runtime::spawn_blocking(move || {
        let _lock_guard = lock_guard;
        toolchain::repair_single_tool(&app_c, &tid, &tool_c, &project_roots)
    })
    .await
    .map_err(|e| e.to_string())?;

    match res {
        Ok(item) => Ok(item),
        Err(e) => {
            emit_log(&app, &task_id, "stderr", &format!("✗ 修复 `{}` 失败：{}", tool_id, e));
            emit_progress(
                &app,
                &task_id,
                &format!("修复工具：{}", tool_id),
                "修复失败",
                &e.to_string(),
                100,
                false,
            );
            Err(e.to_string())
        }
    }
}

#[tauri::command]
async fn repair_all_essential_toolchains(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<ToolchainItem>, String> {
    let task_id = format!("repair-all-{}", chrono::Local::now().timestamp_millis());
    runner::register_task_project(&task_id, "__system__");
    let lock_guard = state
        .runner
        .acquire_project_guard("__toolchain__", &task_id)
        .map_err(|e| e.to_string())?;

    let project_roots: Vec<String> = state
        .config
        .lock()
        .map(|g| g.projects.iter().map(|p| p.root.clone()).collect())
        .unwrap_or_default();

    let tid = task_id.clone();
    let app_c = app.clone();

    let res = tauri::async_runtime::spawn_blocking(move || {
        let _lock_guard = lock_guard;
        let tools = toolchain::scan_toolchains();
        let targets: Vec<String> = tools
            .iter()
            .filter(|t| t.status != "ok" && t.repairable && (t.essential || t.status == "corrupted"))
            .map(|t| t.id.clone())
            .collect();

        if targets.is_empty() {
            emit_log(&app_c, &tid, "system", "✓ 所有核心工具链完整就绪，无需修复");
            emit_progress(&app_c, &tid, "工具链一键修复", "全部就绪", "核心工具无缺失或损坏", 100, false);
            return Ok(tools);
        }

        let mut errs = Vec::new();
        let count = targets.len().max(1);
        for (i, id) in targets.iter().enumerate() {
            let range_lo = (5 + (i * 90) / count) as u8;
            let range_hi = (5 + ((i + 1) * 90) / count).min(95) as u8;
            emit_progress(
                &app_c,
                &tid,
                "一键修复核心工具链",
                &format!("修复 {}/{}：{}", i + 1, targets.len(), id),
                "正在自动下载并校验…",
                range_lo,
                false,
            );
            if let Err(e) =
                toolchain::repair_single_tool_in_range(&app_c, &tid, id, &project_roots, range_lo, range_hi)
            {
                errs.push(format!("{}: {}", id, e));
            }
        }

        let refreshed = toolchain::scan_toolchains();
        if !errs.is_empty() {
            let msg = format!("部分工具修复未完成：{}", errs.join("；"));
            emit_progress(&app_c, &tid, "一键修复核心工具链", "修复失败", &msg, 100, false);
            return Err(anyhow::anyhow!("{}", msg));
        }
        emit_progress(&app_c, &tid, "一键修复核心工具链", "全部修复完成", "工具链已复核就绪", 100, false);
        Ok(refreshed)
    })
    .await
    .map_err(|e| e.to_string())?;

    res.map_err(|e| e.to_string())
}

// =========================================================================
// Version & Changelog Commands
// =========================================================================

fn check_external_build_lock_idle(project: &ProjectConfig) -> Result<(), String> {
    if !project.lock_file.trim().is_empty() {
        if let Some(info) = build_guard::inspect_lock(&project.lock_file) {
            if info.exists && info.alive {
                return Err(format!(
                    "检测到外部构建进程（PID={}，版本={}）正在运行，禁止中途修改版本号或并发构建",
                    info.pid, info.version
                ));
            }
        }
    }
    Ok(())
}

fn ensure_project_idle_for_version_write(
    runner: &TaskRunner,
    project: &ProjectConfig,
) -> Result<(), String> {
    if runner.is_project_busy("__storage__").is_some() {
        return Err("正在迁移存储目录或收归工具，请稍候完成后再修改版本号".into());
    }
    if let Some(tid) = runner.is_project_busy(&project.id) {
        return Err(format!(
            "当前项目正在执行构建或流水线（{}），禁止中途修改版本号以免引发产物自检失败；请等待完成或先点击「停止」",
            tid
        ));
    }
    check_external_build_lock_idle(project)
}

/// Only delegate `-Bump patch|minor|major` to an external `build.ps1` when the current version
/// has no pre-release suffix (e.g. `1.46.7`). Pre-release strings like `1.46.7-beta.1` crash
/// legacy scripts that do `[int]$cur.Split('.')[2]`, so ShipForge always computes & writes those
/// atomically before invoking the build script.
fn should_delegate_bump_to_script(
    has_build_ps1: bool,
    supports_bump: bool,
    current_ver: &str,
    bump_kind: &str,
) -> bool {
    has_build_ps1
        && supports_bump
        && !current_ver.trim().contains('-')
        && matches!(bump_kind, "patch" | "minor" | "major")
}

#[tauri::command]
fn preview_bump(current: String, kind: String) -> Result<String, String> {
    version::bump_semver(&current, &kind).map_err(|e| e.to_string())
}

#[tauri::command]
fn apply_version(
    state: State<AppState>,
    project_id: String,
    new_version: String,
) -> Result<VersionState, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    ensure_project_idle_for_version_write(&state.runner, &project)?;

    // Acquire transient per-project execution lock so `apply_version` and `run_pipeline` /
    // `run_project_action` are atomically mutually exclusive (zero TOCTOU window).
    let ver_tid = format!("ver-{}", chrono::Local::now().timestamp_millis());
    let _lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &ver_tid)
        .map_err(|_| {
            "当前项目正在执行构建或流水线，禁止中途修改版本号；请等待完成或先点击「停止」".to_string()
        })?;

    let st = version::write_project_versions(
        Path::new(&project.root),
        &project.version_files,
        new_version.trim(),
    )
    .map_err(|e| e.to_string())?;
    drop(_lock_guard);
    // Initialize fresh isolated bucket for the new version
    let mut b = storage::load_version_bucket(&project.id, &st.current);
    let git = git_ops::inspect_git_for_project(Path::new(&project.root), &project.release_dir);
    if b.base_git_sha.is_empty() {
        b.base_git_sha = git.head_sha;
    }
    let _ = storage::save_version_bucket(&project.id, &b);
    Ok(st)
}

#[tauri::command]
async fn get_changelog(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<ChangelogData, String> {
    let project = find_project(&state, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        version::read_changelog(Path::new(&project.root), &project.changelog_path)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn add_changelog_entry(
    state: State<AppState>,
    project_id: String,
    version_str: String,
    summary: String,
    body: String,
) -> Result<ChangelogData, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    ensure_project_idle_for_version_write(&state.runner, &project)?;
    let root = Path::new(&project.root);
    let effective_body = if body.trim().is_empty() {
        "- 常规更新与优化。"
    } else {
        body.trim()
    };
    let written_rel = version::upsert_changelog_entry_with_summary(
        root,
        &project.changelog_path,
        &version_str,
        &summary,
        effective_body,
    )
    .map_err(|e| e.to_string())?;
    if project.changelog_path.trim().is_empty() {
        if let Ok(mut cfg) = state.config.lock() {
            if let Some(p) = cfg.projects.iter_mut().find(|p| p.id == project_id) {
                p.changelog_path = written_rel.clone();
                let _ = config::save_config(&cfg);
            }
        }
    }
    let clean_ver = version_str
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    if !clean_ver.is_empty() && !version::is_bare_default_release_note(effective_body) {
        let clean_notes = version::sanitize_public_release_notes(effective_body);
        let mut b = storage::load_version_bucket(&project.id, clean_ver);
        b.notes_draft = clean_notes.clone();
        b.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let _ = storage::save_version_bucket(&project.id, &b);
        let release_dir = config::resolve_release_dir(&project);
        sync_latest_json_notes_if_matching(&release_dir, clean_ver, &clean_notes);
    }
    Ok(version::read_changelog(root, &written_rel))
}

#[tauri::command]
fn upsert_changelog_entry(
    state: State<AppState>,
    project_id: String,
    version_str: String,
    body: String,
    summary: Option<String>,
) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    ensure_project_idle_for_version_write(&state.runner, &project)?;
    let root = Path::new(&project.root);
    let written_rel = version::upsert_changelog_entry_with_summary(
        root,
        &project.changelog_path,
        &version_str,
        summary.as_deref().unwrap_or(""),
        &body,
    )
    .map_err(|e| e.to_string())?;
    if project.changelog_path.trim().is_empty() {
        if let Ok(mut cfg) = state.config.lock() {
            if let Some(p) = cfg.projects.iter_mut().find(|p| p.id == project_id) {
                p.changelog_path = written_rel.clone();
                let _ = config::save_config(&cfg);
            }
        }
    }
    let clean_ver = version_str
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    if !clean_ver.is_empty() && !version::is_bare_default_release_note(&body) {
        let clean_notes = version::sanitize_public_release_notes(&body);
        let mut b = storage::load_version_bucket(&project.id, clean_ver);
        b.notes_draft = clean_notes.clone();
        b.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let _ = storage::save_version_bucket(&project.id, &b);
        let release_dir = config::resolve_release_dir(&project);
        sync_latest_json_notes_if_matching(&release_dir, clean_ver, &clean_notes);
    }
    Ok(written_rel)
}

fn synthesize_release_notes_from_workspace(
    project: &ProjectConfig,
    bucket: &VersionBucket,
    other_roots: &[PathBuf],
) -> Option<String> {
    let root = Path::new(&project.root);
    let mut raw_paths: Vec<(String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // 1. Prefer bucket.changed_files recorded for this version
    for p in &bucket.changed_files {
        let norm = p.trim().replace('\\', "/");
        if !norm.is_empty() && seen.insert(norm.to_ascii_lowercase()) {
            raw_paths.push((norm, "改动".into()));
        }
    }

    // 2. For non-Git projects (or when bucket has no recorded files yet), inspect live non-Git workspace diff
    if raw_paths.is_empty() && root.is_dir() && !root.join(".git").exists() {
        if let Some(base_snap) = storage::load_fs_baseline(&project.id) {
            let (_, cur_snap) = storage::scan_non_git_workspace(root, project, other_roots);
            for (st, p) in storage::diff_non_git_snapshots(&base_snap, &cur_snap) {
                let norm = p.trim().replace('\\', "/");
                if !norm.is_empty() && seen.insert(norm.to_ascii_lowercase()) {
                    let tag = match st.as_str() {
                        "A" => "新增",
                        "D" => "删除",
                        _ => "修改",
                    };
                    raw_paths.push((norm, tag.into()));
                }
            }
        }
    }

    let mut diffs = Vec::new();
    for (p, tag) in raw_paths {
        if guardrails::detect_sensitive_reason(&p).is_some() {
            continue;
        }
        let cat = storage::classify_change_path(&p, project);
        if cat == "artifact" {
            continue;
        }
        // Skip pure version metadata files if other substantive changes exist
        diffs.push(git_ops::FileDiffSummary {
            path: p,
            category: cat,
            added: None,
            deleted: None,
            status_tag: tag,
        });
    }

    let cfg_cl_norm = project
        .changelog_path
        .trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase();
    let substantive: Vec<git_ops::FileDiffSummary> = diffs
        .iter()
        .filter(|d| {
            let low = d.path.to_ascii_lowercase();
            let fname = low.rsplit('/').next().unwrap_or(&low);
            let is_cl_file = (!cfg_cl_norm.is_empty() && low == cfg_cl_norm)
                || matches!(
                    fname,
                    "changelog.md" | "变更记录.md" | "releases.md" | "history.md"
                );
            if is_cl_file {
                return false;
            }
            let is_ver_file = project.version_files.iter().any(|vf| {
                vf.path
                    .trim()
                    .replace('\\', "/")
                    .trim_start_matches("./")
                    .eq_ignore_ascii_case(&low)
            }) || matches!(
                fname,
                "cargo.lock" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock"
            );
            !is_ver_file
        })
        .cloned()
        .collect();

    if substantive.is_empty() {
        return None;
    }
    git_ops::format_categorized_file_changes(&[], &substantive)
}

fn find_previous_published_version(project: &ProjectConfig, root: &Path, target_ver: &str) -> Option<String> {
    if let Some(tag_ver) = git_ops::find_previous_release_tag_for_project(root, target_ver) {
        return Some(tag_ver);
    }
    // Non-Git / untagged fallback: find highest sealed version bucket strictly below target_ver
    let mut best: Option<String> = None;
    for b in storage::list_version_buckets(&project.id) {
        if !b.sealed {
            continue;
        }
        if version::compare_semver_str(&b.version, target_ver) != Some(std::cmp::Ordering::Less) {
            continue;
        }
        match &best {
            None => best = Some(b.version),
            Some(cur)
                if version::compare_semver_str(&b.version, cur) == Some(std::cmp::Ordering::Greater) =>
            {
                best = Some(b.version);
            }
            _ => {}
        }
    }
    best
}

fn resolve_release_notes_detail_for_project(
    project: &ProjectConfig,
    version_str: &str,
    other_roots: &[PathBuf],
) -> ResolvedReleaseNotes {
    let ver = version_str
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    let root = Path::new(&project.root);
    let cl_file_name = Path::new(project.changelog_path.trim())
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("CHANGELOG.md")
        .to_string();

    // Tier 1a: Dedicated per-version release Markdown file (e.g. RELEASE_v1.43.18.md, docs/.../RELEASE_vX.Y.Z.md)
    if let Some((body, rel)) =
        version::extract_dedicated_release_doc_notes(root, &project.changelog_path, ver)
    {
        return ResolvedReleaseNotes {
            version: ver.to_string(),
            notes: body,
            source: "release_doc".into(),
            source_label: format!("版本发布文档 ({})", rel),
            is_fallback: false,
            latest_changelog_version: None,
            latest_changelog_notes: None,
        };
    }

    // Tier 1b: Exact matching substantive entry in project changelog (rejects bare stubs like "QDisk v1.47.3 正式发布。")
    if let Some(body) = version::extract_changelog_entry_exact(root, &project.changelog_path, ver) {
        return ResolvedReleaseNotes {
            version: ver.to_string(),
            notes: body,
            source: "changelog".into(),
            source_label: format!("变更记录 ({})", cl_file_name),
            is_fallback: false,
            latest_changelog_version: None,
            latest_changelog_notes: None,
        };
    }
    if project.changelog_path.trim().is_empty() {
        for cand in ["CHANGELOG.md", "RELEASES.md", "HISTORY.md", "变更记录.md"] {
            if let Some(body) = version::extract_changelog_entry_exact(root, cand, ver) {
                return ResolvedReleaseNotes {
                    version: ver.to_string(),
                    notes: body,
                    source: "changelog".into(),
                    source_label: format!("变更记录 ({})", cand),
                    is_fallback: false,
                    latest_changelog_version: None,
                    latest_changelog_notes: None,
                };
            }
        }
    }

    // Tier 1c: README.md version/changelog section (strictly ignores general marketing intro!)
    if let Some((body, label)) = version::extract_notes_from_readme(root, ver) {
        return ResolvedReleaseNotes {
            version: ver.to_string(),
            notes: body,
            source: "readme".into(),
            source_label: format!("README ({})", label),
            is_fallback: false,
            latest_changelog_version: None,
            latest_changelog_notes: None,
        };
    }

    // Tier 1c2: Unpublished changelog entries strictly after previous formal release tag/sealed version
    // and <= target version (e.g. v1.46.16 -> 1.47.2 automatically collects human notes for 1.46.17/1.47.0/1.47.1!)
    let prev_published = find_previous_published_version(project, root, ver);
    if let Some((combined, vers, fname)) = version::extract_changelog_entries_since(
        root,
        &project.changelog_path,
        prev_published.as_deref(),
        ver,
    ) {
        let label = if vers.len() == 1 {
            format!("变更记录 ({} · v{})", fname, vers[0])
        } else {
            format!(
                "变更记录汇总 ({} · v{}–v{})",
                fname,
                vers.last().map(String::as_str).unwrap_or(""),
                vers.first().map(String::as_str).unwrap_or("")
            )
        };
        return ResolvedReleaseNotes {
            version: ver.to_string(),
            notes: combined,
            source: "changelog_range".into(),
            source_label: label,
            is_fallback: false,
            latest_changelog_version: None,
            latest_changelog_notes: None,
        };
    }

    // Tier 1d: Existing aligned release/latest.json with rich non-fallback notes (e.g. written by external build script)
    let rel_dir = config::resolve_release_dir(project);
    let mf_path = rel_dir.join("latest.json");
    if mf_path.is_file() {
        if let Ok(raw) = std::fs::read_to_string(&mf_path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                let mf_ver = val
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .trim_start_matches('v')
                    .trim_start_matches('V');
                let mf_notes = val.get("notes").and_then(|v| v.as_str()).unwrap_or("");
                if !ver.is_empty()
                    && mf_ver.eq_ignore_ascii_case(ver)
                    && !version::is_bare_default_release_note(mf_notes)
                {
                    return ResolvedReleaseNotes {
                        version: ver.to_string(),
                        notes: version::sanitize_public_release_notes(mf_notes),
                        source: "manifest".into(),
                        source_label: "现有清单 (latest.json)".into(),
                        is_fallback: false,
                        latest_changelog_version: None,
                        latest_changelog_notes: None,
                    };
                }
            }
        }
    }

    // Tier 2: Non-empty, non-fallback draft explicitly saved for this version bucket
    let bucket = storage::load_version_bucket(&project.id, ver);
    if !version::is_bare_default_release_note(&bucket.notes_draft) {
        return ResolvedReleaseNotes {
            version: ver.to_string(),
            notes: version::sanitize_public_release_notes(&bucket.notes_draft),
            source: "draft".into(),
            source_label: "已保存版本草稿".into(),
            is_fallback: false,
            latest_changelog_version: None,
            latest_changelog_notes: None,
        };
    }

    // Tier 3: Unreleased / 待发布 section in project changelog
    if let Some(unrel) = version::extract_unreleased_changelog_notes(root, &project.changelog_path) {
        return ResolvedReleaseNotes {
            version: ver.to_string(),
            notes: unrel,
            source: "unreleased".into(),
            source_label: format!("待发布区 ({})", cl_file_name),
            is_fallback: false,
            latest_changelog_version: None,
            latest_changelog_notes: None,
        };
    }

    let cl_exists = !project.changelog_path.trim().is_empty()
        && guardrails::resolve_confined_path(root, &project.changelog_path)
            .map(|p| p.is_file())
            .unwrap_or(false);
    let latest_cl = version::extract_latest_substantive_changelog_entry(root, &project.changelog_path);
    let (latest_cl_ver, latest_cl_notes) = match latest_cl {
        Some((v, n)) => (Some(v), Some(n)),
        None => (None, None),
    };

    // Tier 4: Automatic extraction from Git changelog diff prose + commits + semantic module summary
    if let Some(git_notes) = git_ops::synthesize_release_notes_from_git(root, project, ver) {
        return ResolvedReleaseNotes {
            version: ver.to_string(),
            notes: git_notes,
            source: "git_commits".into(),
            source_label: if cl_exists {
                format!("{} 尚无 v{}，已从 Git 改动提取", cl_file_name, ver)
            } else {
                "自动提取（Git 提交与代码改动）".into()
            },
            is_fallback: false,
            latest_changelog_version: latest_cl_ver,
            latest_changelog_notes: latest_cl_notes,
        };
    }

    // Tier 5: Non-Git workspace / version bucket file diff summary
    if let Some(ws_notes) = synthesize_release_notes_from_workspace(project, &bucket, other_roots) {
        return ResolvedReleaseNotes {
            version: ver.to_string(),
            notes: ws_notes,
            source: "workspace_diff".into(),
            source_label: if cl_exists {
                format!("{} 尚无 v{}，已从工作区变动提取", cl_file_name, ver)
            } else {
                "自动提取（工作区文件变动）".into()
            },
            is_fallback: false,
            latest_changelog_version: latest_cl_ver,
            latest_changelog_notes: latest_cl_notes,
        };
    }

    // Tier 5b: No code/doc change for this version at all (e.g. a rebuild/re-publish bump) but the changelog
    // already holds a real latest entry -> carry that real content over instead of a bare placeholder.
    if let (Some(lv), Some(ln)) = (latest_cl_ver.as_ref(), latest_cl_notes.as_ref()) {
        let mut lines = ln.lines();
        let first = lines.next().unwrap_or("");
        let rest: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();
        let carried = if first.trim_start().starts_with('#') && !rest.is_empty() {
            rest
        } else {
            ln.clone()
        };
        return ResolvedReleaseNotes {
            version: ver.to_string(),
            notes: carried,
            source: "changelog_carryover".into(),
            source_label: format!("沿用 v{} 日志（v{} 无新增改动，{}）", lv, ver, cl_file_name),
            is_fallback: false,
            latest_changelog_version: latest_cl_ver,
            latest_changelog_notes: latest_cl_notes,
        };
    }

    // Tier 6: Clean fallback when literally no changelog, draft, commit, or substantive code changes exist
    let name = if project.product_name.trim().is_empty() {
        if project.name.trim().is_empty() {
            "Software"
        } else {
            project.name.trim()
        }
    } else {
        project.product_name.trim()
    };
    let fallback_label = if let Some(ref lv) = latest_cl_ver {
        format!("{} 尚无 v{} 实质说明（最新有效记录为 v{}）", cl_file_name, ver, lv)
    } else if cl_exists {
        format!("{} 尚无 v{} 实质说明（可编辑后写入变更记录）", cl_file_name, ver)
    } else {
        "默认兜底（未检测到变动记录）".into()
    };
    ResolvedReleaseNotes {
        version: ver.to_string(),
        notes: format!("{} v{} 正式发布。", name, ver),
        source: "fallback".into(),
        source_label: fallback_label,
        is_fallback: true,
        latest_changelog_version: latest_cl_ver,
        latest_changelog_notes: latest_cl_notes,
    }
}

fn resolve_release_notes_for_project(
    project: &ProjectConfig,
    version_str: &str,
    other_roots: &[PathBuf],
) -> String {
    resolve_release_notes_detail_for_project(project, version_str, other_roots).notes
}

fn sync_latest_json_notes_if_matching(release_dir: &Path, expected_ver: &str, notes: &str) {
    let trimmed = notes.trim();
    if trimmed.is_empty() || expected_ver.trim().is_empty() {
        return;
    }
    let mf_path = release_dir.join("latest.json");
    if !mf_path.is_file() {
        return;
    }
    let Ok(raw) = std::fs::read_to_string(&mf_path) else {
        return;
    };
    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let Some(obj) = val.as_object_mut() else {
        return;
    };
    let mf_ver = obj
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    let exp_clean = expected_ver
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    if !mf_ver.eq_ignore_ascii_case(exp_clean) {
        return;
    }
    let cur_notes = obj.get("notes").and_then(|v| v.as_str()).unwrap_or("");
    if cur_notes.trim() == trimmed {
        return;
    }
    obj.insert("notes".into(), serde_json::Value::String(trimmed.to_string()));
    if let Ok(mut pretty) = serde_json::to_string_pretty(&val) {
        pretty.push('\n');
        let _ = guardrails::atomic_write_bytes(&mf_path, pretty.as_bytes());
    }
}

#[tauri::command]
async fn resolve_release_notes_detail(
    state: State<'_, AppState>,
    project_id: String,
    version_str: String,
) -> Result<ResolvedReleaseNotes, String> {
    let project = find_project(&state, &project_id)?;
    let other_roots = other_project_roots(&state, &project.id);
    tauri::async_runtime::spawn_blocking(move || {
        resolve_release_notes_detail_for_project(&project, &version_str, &other_roots)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn extract_release_notes(
    state: State<'_, AppState>,
    project_id: String,
    version_str: String,
) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    let other_roots = other_project_roots(&state, &project.id);
    tauri::async_runtime::spawn_blocking(move || {
        resolve_release_notes_for_project(&project, &version_str, &other_roots)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn draft_changelog_from_git(
    state: State<'_, AppState>,
    project_id: String,
    version_str: Option<String>,
) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let target_ver = version_str
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            version::read_project_versions(Path::new(&project.root), &project.version_files).current
        });
    tauri::async_runtime::spawn_blocking(move || {
        git_ops::draft_notes_from_git_for_project(Path::new(&project.root), &project, &target_ver)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

// =========================================================================
// Actions & Pipeline Commands
// =========================================================================

#[tauri::command]
fn run_project_action(
    app: AppHandle,
    state: State<AppState>,
    project_id: String,
    action_id: String,
    bump: Option<String>,
    explicit_version: Option<String>,
    channel: Option<String>,
    skip_tests: Option<bool>,
) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let action = project
        .actions
        .iter()
        .find(|a| a.id == action_id)
        .cloned()
        .ok_or_else(|| format!("找不到动作：{}", action_id))?;

    let guardrail_settings = state
        .config
        .lock()
        .map(|c| c.guardrails.clone())
        .unwrap_or_default();
    let is_build_or_pack = matches!(action.category.as_str(), "build" | "package");
    // Check disk headroom BEFORE acquiring lock or mutating any version files
    if is_build_or_pack {
        let rel_dir = config::resolve_release_dir(&project);
        guardrails::ensure_disk_headroom(&rel_dir, 128 * 1024 * 1024, &guardrail_settings)
            .map_err(|e| e.to_string())?;
    }

    // Validate explicit_version strictly before locking or passing -Version to scripts
    if let Some(ref v) = explicit_version {
        let tv = v.trim();
        if !tv.is_empty() {
            version::validate_semver(tv).map_err(|e| e.to_string())?;
        }
    }

    let task_id = format!("task-{}", chrono::Local::now().timestamp_millis());
    let lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &task_id)
        .map_err(|e| e.to_string())?;

    let mut ver_state = version::read_project_versions(Path::new(&project.root), &project.version_files);
    let mut args = action.args.clone();
    let mut prewrite_version: Option<String> = None;
    if project.has_build_ps1 {
        if action.supports_bump {
            if let Some(ref b) = bump {
                if should_delegate_bump_to_script(true, true, &ver_state.current, b) {
                    args.push("-Bump".into());
                    args.push(b.clone());
                } else if matches!(b.as_str(), "patch" | "minor" | "major" | "beta") {
                    check_external_build_lock_idle(&project)?;
                    let next = version::bump_semver(&ver_state.current, b).map_err(|e| e.to_string())?;
                    prewrite_version = Some(next);
                }
            } else if let Some(ref v) = explicit_version {
                let tv = v.trim();
                if !tv.is_empty() {
                    if tv.contains('-') {
                        check_external_build_lock_idle(&project)?;
                        prewrite_version = Some(tv.to_string());
                    } else {
                        args.push("-Version".into());
                        args.push(tv.to_string());
                    }
                }
            }
        }
        if action.supports_channel {
            if let Some(ref ch) = channel {
                if ch == "beta" || ch == "stable" {
                    args.push("-Channel".into());
                    args.push(ch.clone());
                }
            }
        }
        if action.supports_skip_tests && skip_tests.unwrap_or(false) {
            args.push("-SkipTests".into());
        }
    }

    // Capture transaction snapshot AFTER all fallible pre-checks and BEFORE mutating version files
    let tx_snap = if is_build_or_pack {
        transaction::begin_transaction(&project, &action.id).ok()
    } else {
        None
    };

    if let Some(ref next_ver) = prewrite_version {
        match version::write_project_versions(
            Path::new(&project.root),
            &project.version_files,
            next_ver,
        ) {
            Ok(st) => ver_state = st,
            Err(e) => {
                if let Some(ref snap) = tx_snap {
                    transaction::commit_transaction(snap);
                }
                return Err(e.to_string());
            }
        }
    }

    let runner = state.runner.clone();
    let watch_state = state.watch_state.clone();
    let tid = task_id.clone();
    let other_roots = other_project_roots(&state, &project.id);
    runner::register_task_project(&tid, &project.id);

    std::thread::spawn(move || {
        let _lock_guard = lock_guard;
        let res = runner.run_streaming_sync(
            &app,
            &tid,
            &project,
            &action.id,
            &action.label,
            &action.command,
            &args,
            &action.cwd,
            &HashMap::new(),
            &ver_state.current,
            false,
            (5, 98),
        );
        match res {
            Ok((true, _, ms, _)) => {
                if let Some(ref snap) = tx_snap {
                    transaction::commit_transaction(snap);
                }
                if is_build_or_pack {
                    let latest_ver = version::read_project_versions(
                        Path::new(&project.root),
                        &project.version_files,
                    )
                    .current;
                    record_build_into_version_bucket(&project, &latest_ver, &other_roots);
                    let (rep_post, fs_post) = compute_change_report_for_project(&project, None, &other_roots);
                    if let Ok(mut ws) = watch_state.lock() {
                        ws.insert(
                            project.id.clone(),
                            ProjectWatchSnapshot {
                                fingerprint: rep_post.fingerprint,
                                version: rep_post.current_version,
                                artifact_fp: rep_post.artifact_fingerprint,
                                fs_snapshot: fs_post,
                            },
                        );
                    }
                    let rep = health_report::build_ephemeral_report(
                        &project,
                        "build",
                        "ok",
                        ms,
                        format!("{} 完成（v{}）", action.label, latest_ver),
                        None,
                        &guardrail_settings,
                    );
                    let _ = app.emit("health-report-once", &rep);
                }
            }
            Ok((false, code, ms, cancelled)) => {
                if let Some(ref snap) = tx_snap {
                    if guardrail_settings.auto_rollback_on_abort {
                        let trigger = if cancelled { "用户中止工序" } else { "工序失败自动触发" };
                        let proof = transaction::rollback_and_verify(snap, trigger);
                        let rep = health_report::build_ephemeral_report(
                            &project,
                            "rollback",
                            "rolled_back",
                            ms,
                            format!(
                                "{} {}，已自动安全回滚至初始状态",
                                action.label,
                                if cancelled { "已取消".into() } else { format!("失败 (exit {})", code) }
                            ),
                            Some(proof),
                            &guardrail_settings,
                        );
                        let _ = app.emit("health-report-once", &rep);
                    } else {
                        transaction::commit_transaction(snap);
                    }
                }
            }
            Err(_) => {
                if let Some(ref snap) = tx_snap {
                    if guardrail_settings.auto_rollback_on_abort {
                        let _ = transaction::rollback_and_verify(snap, "启动工序异常");
                    } else {
                        transaction::commit_transaction(snap);
                    }
                }
            }
        }
    });

    Ok(task_id)
}

#[tauri::command]
fn run_custom_command(
    app: AppHandle,
    state: State<AppState>,
    project_id: String,
    label: String,
    command: String,
    args: Vec<String>,
    cwd: String,
) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let trimmed_cmd = command.trim().to_string();
    if trimmed_cmd.is_empty() {
        return Err("要执行的命令不能为空".into());
    }
    let trimmed_cwd = cwd.trim().to_string();
    if !trimmed_cwd.is_empty() {
        let resolved_cwd = guardrails::resolve_confined_path(Path::new(&project.root), &trimmed_cwd)
            .map_err(|e| format!("命令工作子目录越界或非法：{}", e))?;
        if !resolved_cwd.is_dir() {
            return Err(format!("指定的子目录不存在：{}", trimmed_cwd));
        }
    }
    let task_id = format!("cmd-{}", chrono::Local::now().timestamp_millis());
    let lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &task_id)
        .map_err(|e| e.to_string())?;

    let ver_state = version::read_project_versions(Path::new(&project.root), &project.version_files);
    let runner = state.runner.clone();
    let tid = task_id.clone();
    runner::register_task_project(&tid, &project.id);

    std::thread::spawn(move || {
        let _lock_guard = lock_guard;
        let _ = runner.run_streaming_sync(
            &app,
            &tid,
            &project,
            "custom",
            &label,
            &trimmed_cmd,
            &args,
            &trimmed_cwd,
            &HashMap::new(),
            &ver_state.current,
            false,
            (8, 96),
        );
    });
    Ok(task_id)
}

fn preflight_pipeline(project: &ProjectConfig, req: &PipelineRequest) -> Result<(), String> {
    if req.run_build || req.bump_kind != "none" {
        check_external_build_lock_idle(project)?;
    }
    if !matches!(
        req.bump_kind.as_str(),
        "none" | "patch" | "minor" | "major" | "beta" | "custom"
    ) {
        return Err(format!("不支持的版本递增策略：{}", req.bump_kind));
    }
    if req.bump_kind == "custom" {
        let v = req.custom_version.as_deref().unwrap_or("").trim();
        version::validate_semver(v).map_err(|e| e.to_string())?;
    } else if matches!(req.bump_kind.as_str(), "patch" | "minor" | "major" | "beta")
        && !project.version_files.is_empty()
    {
        let vs = version::read_project_versions(Path::new(&project.root), &project.version_files);
        if let Some(bad) = vs.readings.iter().find(|r| !r.exists || r.error.is_some()) {
            return Err(format!(
                "版本配置文件异常（{}：{}），请先在「版本与源码」修复后再执行发版流水线",
                bad.path,
                bad.error.as_deref().unwrap_or("文件缺失")
            ));
        }
        version::bump_semver(&vs.current, &req.bump_kind)
            .map_err(|e| format!("当前项目版本号无效（{}），无法递增：{}", vs.current, e))?;
    }
    if req.bump_kind == "none"
        && (req.generate_manifest
            || matches!(req.git_mode.as_str(), "commit_tag" | "commit_tag_push")
            || req.upload_github)
        && !project.version_files.is_empty()
    {
        let vs = version::read_project_versions(Path::new(&project.root), &project.version_files);
        if !vs.synced {
            return Err("检测到多个版本文件版本号不一致或存在损坏，请先在「版本与源码」对齐后再执行发版".into());
        }
        version::validate_semver(&vs.current)
            .map_err(|e| format!("当前项目版本号无效（{}）：{}", vs.current, e))?;
    }
    if req.run_build {
        let has_build = project
            .actions
            .iter()
            .any(|a| a.id == req.build_action_id || a.category == "build" || a.category == "package");
        if !has_build {
            return Err("未找到可执行的构建或打包动作，请先配置或取消勾选「构建打包」".into());
        }
    }
    if matches!(req.git_mode.as_str(), "commit_tag" | "commit_tag_push") {
        let root = Path::new(&project.root);
        if !root.join(".git").exists() {
            return Err("当前项目未初始化 Git 仓库，请关闭流水线中的自动 Git 提交或先初始化仓库".into());
        }
        if req.git_mode == "commit_tag_push" && !git_ops::git_has_remote(root) {
            return Err("当前 Git 仓库尚未配置远端 (origin)，请先配置远端或改为「自动提交 + 打 Tag」".into());
        }
    }
    if req.upload_github {
        let repo = if !req.github_repo.trim().is_empty() {
            req.github_repo.trim()
        } else if !project.repo_binding.repo_slug.trim().is_empty() {
            project.repo_binding.repo_slug.trim()
        } else {
            project.github_repo.trim()
        };
        if !repo.contains('/') {
            return Err("上传发布前请先在「产物与仓库」绑定目标仓库（owner/repo）".into());
        }
        let prov = if !req.repo_provider.trim().is_empty() {
            req.repo_provider.as_str()
        } else if !project.repo_binding.provider.trim().is_empty() {
            project.repo_binding.provider.as_str()
        } else {
            "github"
        };
        let base_url = if !req.repo_base_url.trim().is_empty() {
            req.repo_base_url.as_str()
        } else {
            project.repo_binding.base_url.as_str()
        };
        let cred_id = if !req.credential_id.trim().is_empty() {
            req.credential_id.as_str()
        } else {
            project.repo_binding.credential_id.as_str()
        };
        if let Err(e) = credentials::resolve_token(prov, base_url, cred_id) {
            return Err(format!(
                "上传发布前置检查失败：未检测到可用的 {} 访问令牌（{}），请先在「产物与仓库」保存令牌",
                prov, e
            ));
        }
    }
    Ok(())
}

#[tauri::command]
fn run_pipeline(
    app: AppHandle,
    state: State<AppState>,
    req: PipelineRequest,
) -> Result<String, String> {
    let project = find_project(&state, &req.project_id)?;
    ensure_root_exists(&project)?;
    preflight_pipeline(&project, &req)?;

    let (guardrail_settings, default_local_port) = state
        .config
        .lock()
        .map(|c| (c.guardrails.clone(), c.default_port))
        .unwrap_or_else(|_| (GuardrailSettings::default(), 18765));

    // Enforce disk headroom safety buffer prior to acquiring lock or mutating any file
    let release_dir_check = config::resolve_release_dir(&project);
    guardrails::ensure_disk_headroom(&release_dir_check, 128 * 1024 * 1024, &guardrail_settings)
        .map_err(|e| e.to_string())?;

    let pipeline_id = format!("pipe-{}", chrono::Local::now().timestamp_millis());
    let lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &pipeline_id)
        .map_err(|e| e.to_string())?;

    // Capture atomic crash-recoverable transaction snapshot BEFORE any mutation
    let tx_snap = Arc::new(Mutex::new(
        transaction::begin_transaction(&project, "pipeline")
            .map_err(|e| format!("创建事务快照失败：{}", e))?,
    ));

    let cancel_flag = state.runner.register_pipeline(&pipeline_id);
    let runner = state.runner.clone();
    let watch_state = state.watch_state.clone();
    let pid = pipeline_id.clone();
    let other_roots = other_project_roots(&state, &project.id);
    runner::register_task_project(&pid, &project.id);

    std::thread::spawn(move || {
        let _lock_guard = lock_guard;
        let started = Instant::now();
        let proj_id = project.id.clone();
        let root = PathBuf::from(&project.root);
        let release_dir = config::resolve_release_dir(&project);

        let mut steps: Vec<(&str, &str)> = Vec::new();
        if req.bump_kind != "none" {
            steps.push(("version", "版本更新"));
        }
        if req.run_tests && !(project.has_build_ps1 && req.run_build) {
            steps.push(("test", "测试门禁"));
        }
        if req.run_build {
            steps.push(("build", "构建与打包"));
        }
        if req.generate_manifest {
            steps.push(("manifest", "生成清单与校验"));
        }
        if matches!(req.git_mode.as_str(), "commit_tag" | "commit_tag_push") {
            let git_label = if req.git_mode == "commit_tag_push" {
                "Git 提交、打标与推送"
            } else {
                "Git 自动提交与打标"
            };
            steps.push(("git", git_label));
        }
        if req.upload_github {
            let prov_lbl = if !req.repo_provider.trim().is_empty() {
                req.repo_provider.as_str()
            } else if !project.repo_binding.provider.trim().is_empty() {
                project.repo_binding.provider.as_str()
            } else {
                "github"
            };
            steps.push((
                "publish",
                match prov_lbl {
                    "gitee" => "发布 Gitee Release",
                    "gitlab" => "发布 GitLab Release",
                    "gitea" => "发布 Gitea Release",
                    _ => "发布 GitHub Release",
                },
            ));
        }

        let finish_pipeline = |success: bool, cancelled: bool, summary: String| {
            let mut rollback_proof: Option<RollbackVerification> = None;
            let mut final_summary = summary.clone();
            let snap_guard = tx_snap.lock().ok();
            let mut preserved_verified_build = false;
            if success {
                if let Some(ref snap) = snap_guard {
                    transaction::commit_transaction(snap);
                }
            } else if guardrail_settings.auto_rollback_on_abort {
                let trigger = if cancelled { "用户主动中止" } else { "步骤失败自动触发" };
                if let Some(ref snap) = snap_guard {
                    preserved_verified_build = snap.remote_pushed || snap.build_verified;
                    emit_log(
                        &app,
                        &pid,
                        "system",
                        &if preserved_verified_build {
                            format!(
                                "↺ 编译与产物验收已通过（{}）：仅清理运行锁与临时事务，完整保留版本配置、Git 与 release/ 成品…",
                                trigger
                            )
                        } else {
                            format!(
                                "↺ 触发安全事务回滚（{}）：正在还原版本号、Git 状态、半成品与构建锁…",
                                trigger
                            )
                        },
                    );
                    let proof = transaction::rollback_and_verify(snap, trigger);
                    emit_log(
                        &app,
                        &pid,
                        "system",
                        &format!(
                            "{} 事务回滚复核完成（{} 项检查，耗时 {} ms）",
                            if proof.all_restored { "✓" } else { "!" },
                            proof.checks.len(),
                            proof.duration_ms
                        ),
                    );
                    if preserved_verified_build {
                        final_summary = format!(
                            "{}（已完整保留验收产物，可直接前往「产物与仓库」重试发布，无需重编）",
                            summary
                        );
                    } else if proof.all_restored {
                        final_summary = format!("{}（已自动安全回滚至初始状态）", summary);
                    } else {
                        final_summary = format!("{}（已执行回滚，部分项请查看健康报告）", summary);
                    }
                    rollback_proof = Some(proof);
                }
            } else if let Some(ref snap) = snap_guard {
                transaction::commit_transaction(snap);
            }

            runner.unregister_pipeline(&pid);
            runner.release_project(&proj_id, &pid);
            if success {
                let (rep_post, fs_post) =
                    compute_change_report_for_project(&project, None, &other_roots);
                if let Ok(mut ws) = watch_state.lock() {
                    ws.insert(
                        proj_id.clone(),
                        ProjectWatchSnapshot {
                            fingerprint: rep_post.fingerprint,
                            version: rep_post.current_version,
                            artifact_fp: rep_post.artifact_fingerprint,
                            fs_snapshot: fs_post,
                        },
                    );
                }
            }
            let elapsed_ms = started.elapsed().as_millis() as u64;
            emit_progress(
                &app,
                &pid,
                "构建发布流程",
                if success {
                    "已完成"
                } else if preserved_verified_build {
                    "已保留产物待续传"
                } else if cancelled {
                    "已取消并回滚"
                } else {
                    "已中止并回滚"
                },
                &final_summary,
                if success { 100 } else { 0 },
                false,
            );
            let _ = app.emit(
                "pipeline-finished",
                &PipelineFinishedEvent {
                    pipeline_id: pid.clone(),
                    project_id: proj_id.clone(),
                    success,
                    cancelled,
                    duration_ms: elapsed_ms,
                    summary: final_summary.clone(),
                },
            );

            // Emit pure in-memory one-time Integrity & Health Analysis Report (NEVER persisted!)
            let outcome = if success {
                "ok"
            } else if rollback_proof.is_some() {
                "rolled_back"
            } else {
                "error"
            };
            let report = health_report::build_ephemeral_report(
                &project,
                if success { "pipeline" } else { "rollback" },
                outcome,
                elapsed_ms,
                final_summary,
                rollback_proof,
                &guardrail_settings,
            );
            let _ = app.emit("health-report-once", &report);
        };

        if steps.is_empty() {
            emit_log(&app, &pid, "system", "流水线未勾选任何步骤，已跳过。");
            finish_pipeline(true, false, "未选择任何执行步骤".into());
            return;
        }

        let total = steps.len();
        let emit_step = |idx: usize, name: &str, status: &str, msg: &str| {
            let _ = app.emit(
                "pipeline-step",
                &PipelineStepEvent {
                    pipeline_id: pid.clone(),
                    project_id: proj_id.clone(),
                    index: idx,
                    total,
                    name: name.to_string(),
                    status: status.to_string(),
                    message: msg.to_string(),
                },
            );
        };

        let mut current_ver = version::read_project_versions(&root, &project.version_files).current;
        let mut delegated_bump = false;
        let mut resolved_pipeline_notes: Option<String> =
            if version::is_bare_default_release_note(&req.release_notes) {
                None
            } else {
                Some(version::sanitize_public_release_notes(&req.release_notes))
            };

        for (i, (key, name)) in steps.iter().enumerate() {
            if cancel_flag.load(Ordering::SeqCst) {
                emit_step(i, name, "failed", "已取消");
                finish_pipeline(false, true, format!("已在「{}」前取消", name));
                return;
            }
            let step_start = ((i * 95) / total).max(2) as u8;
            let step_end = (((i + 1) * 95) / total).min(98) as u8;

            emit_step(i, name, "running", "执行中…");
            emit_progress(
                &app,
                &pid,
                "一键发版流水线",
                &format!("[{}/{}] {}", i + 1, total, name),
                "正在执行…",
                step_start,
                false,
            );
            emit_log(
                &app,
                &pid,
                "system",
                &format!("=== 流水线 [{}/{}] {} ===", i + 1, total, name),
            );

            match *key {
                "version" => {
                    let build_act_supports_bump = project
                        .actions
                        .iter()
                        .find(|a| a.id == req.build_action_id)
                        .map(|a| a.supports_bump)
                        .unwrap_or(false);
                    if should_delegate_bump_to_script(
                        project.has_build_ps1 && req.run_build,
                        build_act_supports_bump,
                        &current_ver,
                        &req.bump_kind,
                    ) {
                        match version::bump_semver(&current_ver, &req.bump_kind) {
                            Ok(next) => {
                                current_ver = next.clone();
                                delegated_bump = true;
                                emit_step(i, name, "ok", &format!("锁内递增至 v{}", next));
                            }
                            Err(e) => {
                                let msg = e.to_string();
                                emit_step(i, name, "failed", &msg);
                                finish_pipeline(false, false, msg);
                                return;
                            }
                        }
                    } else {
                        let target = if req.bump_kind == "custom" {
                            req.custom_version.clone().unwrap_or_default()
                        } else {
                            match version::bump_semver(&current_ver, &req.bump_kind) {
                                Ok(v) => v,
                                Err(e) => {
                                    let msg = e.to_string();
                                    emit_step(i, name, "failed", &msg);
                                    finish_pipeline(false, false, msg);
                                    return;
                                }
                            }
                        };
                        match version::write_project_versions(&root, &project.version_files, &target) {
                            Ok(st) => {
                                current_ver = st.current;
                                emit_step(i, name, "ok", &format!("已原子同步为 v{}", current_ver));
                            }
                            Err(e) => {
                                let msg = e.to_string();
                                emit_step(i, name, "failed", &msg);
                                finish_pipeline(false, false, msg);
                                return;
                            }
                        }
                    }
                }
                "test" => {
                    if let Some(test_act) = project.actions.iter().find(|a| a.category == "test") {
                        let res = runner.run_streaming_sync(
                            &app,
                            &pid,
                            &project,
                            &test_act.id,
                            &test_act.label,
                            &test_act.command,
                            &test_act.args,
                            &test_act.cwd,
                            &HashMap::new(),
                            &current_ver,
                            true,
                            (step_start, step_end),
                        );
                        match res {
                            Ok((true, _, _, _)) => emit_step(i, name, "ok", "测试全部通过"),
                            Ok((_, code, _, cancelled)) => {
                                let msg = if cancelled {
                                    "已取消".to_string()
                                } else {
                                    format!("测试失败（退出码 {}）", code)
                                };
                                emit_step(i, name, "failed", &msg);
                                finish_pipeline(false, cancelled, msg);
                                return;
                            }
                            Err(e) => {
                                let msg = e.to_string();
                                emit_step(i, name, "failed", &msg);
                                finish_pipeline(false, false, msg);
                                return;
                            }
                        }
                    } else {
                        emit_step(i, name, "skipped", "未配置测试命令");
                    }
                }
                "build" => {
                    let act = project
                        .actions
                        .iter()
                        .find(|a| a.id == req.build_action_id)
                        .or_else(|| {
                            project
                                .actions
                                .iter()
                                .find(|a| a.category == "build" || a.category == "package")
                        });
                    let Some(build_act) = act else {
                        emit_step(i, name, "failed", "未找到可用构建动作");
                        finish_pipeline(false, false, "未找到可用构建动作".into());
                        return;
                    };
                    let mut args = build_act.args.clone();
                    if project.has_build_ps1 {
                        if delegated_bump && build_act.supports_bump {
                            args.push("-Bump".into());
                            args.push(req.bump_kind.clone());
                        }
                        if version::resolve_release_channel(&req.channel, &current_ver).channel == "beta"
                            && build_act.supports_channel
                        {
                            args.push("-Channel".into());
                            args.push("beta".into());
                        }
                        if !req.run_tests && build_act.supports_skip_tests {
                            args.push("-SkipTests".into());
                        }
                    }
                    let res = runner.run_streaming_sync(
                        &app,
                        &pid,
                        &project,
                        &build_act.id,
                        &build_act.label,
                        &build_act.command,
                        &args,
                        &build_act.cwd,
                        &HashMap::new(),
                        &current_ver,
                        true,
                        (step_start, step_end),
                    );
                    match res {
                        Ok((true, _, ms, _)) => {
                            current_ver =
                                version::read_project_versions(&root, &project.version_files).current;
                            record_build_into_version_bucket(&project, &current_ver, &other_roots);
                            if let Ok(mut snap) = tx_snap.lock() {
                                snap.mark_build_verified();
                            }
                            emit_step(
                                i,
                                name,
                                "ok",
                                &format!(
                                    "完成（{} · v{}）",
                                    runner::format_duration_ms(ms, true),
                                    current_ver
                                ),
                            );
                        }
                        Ok((_, code, _, cancelled)) => {
                            let msg = if cancelled {
                                "已取消".to_string()
                            } else {
                                format!("构建失败（退出码 {}）", code)
                            };
                            emit_step(i, name, "failed", &msg);
                            finish_pipeline(false, cancelled, msg);
                            return;
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            emit_step(i, name, "failed", &msg);
                            finish_pipeline(false, false, msg);
                            return;
                        }
                    }
                }
                "manifest" => {
                    let artifacts = packager::scan_artifacts(&release_dir);
                    let inst = artifacts
                        .iter()
                        .find(|a| {
                            !a.is_archived
                                && a.kind == "installer"
                                && a.version.as_deref() == Some(current_ver.as_str())
                        })
                        .map(|a| a.name.clone());
                    let port = artifacts
                        .iter()
                        .find(|a| {
                            !a.is_archived
                                && a.kind == "portable"
                                && a.version.as_deref() == Some(current_ver.as_str())
                        })
                        .map(|a| a.name.clone());

                    let notes = if let Some(ref cached) = resolved_pipeline_notes {
                        cached.clone()
                    } else {
                        let detail = resolve_release_notes_detail_for_project(
                            &project,
                            &current_ver,
                            &other_roots,
                        );
                        emit_log(
                            &app,
                            &pid,
                            "system",
                            &format!("✓ 已自动提取更新日志（来源：{}）", detail.source_label),
                        );
                        if !detail.is_fallback {
                            let mut b = storage::load_version_bucket(&project.id, &current_ver);
                            if b.notes_draft.trim().is_empty() {
                                b.notes_draft = detail.notes.clone();
                                b.updated_at =
                                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                                let _ = storage::save_version_bucket(&project.id, &b);
                            }
                        }
                        resolved_pipeline_notes = Some(detail.notes.clone());
                        detail.notes
                    };
                    if !version::is_bare_default_release_note(&req.release_notes)
                        && !version::is_bare_default_release_note(&notes)
                    {
                        let mut b = storage::load_version_bucket(&project.id, &current_ver);
                        b.notes_draft = notes.clone();
                        b.updated_at =
                            chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                        let _ = storage::save_version_bucket(&project.id, &b);
                    }
                    let repo = if !req.github_repo.trim().is_empty() {
                        req.github_repo.clone()
                    } else if !project.repo_binding.repo_slug.trim().is_empty() {
                        project.repo_binding.repo_slug.clone()
                    } else {
                        project.github_repo.clone()
                    };
                    let prov = if !req.repo_provider.trim().is_empty() {
                        req.repo_provider.as_str()
                    } else if !project.repo_binding.provider.trim().is_empty() {
                        project.repo_binding.provider.as_str()
                    } else {
                        "github"
                    };
                    let custom_base = if prov == "github" || !repo.contains('/') {
                        String::new()
                    } else {
                        let host = if !req.repo_base_url.trim().is_empty() {
                            req.repo_base_url.trim().trim_end_matches('/')
                        } else if !project.repo_binding.base_url.trim().is_empty() {
                            project.repo_binding.base_url.trim().trim_end_matches('/')
                        } else {
                            match prov {
                                "gitee" => "https://gitee.com",
                                "gitlab" => "https://gitlab.com",
                                "gitea" => "https://gitea.com",
                                _ => "https://github.com",
                            }
                        };
                        match prov {
                            "gitlab" => format!(
                                "{}/api/v4/projects/{}/packages/generic/release/{}",
                                host,
                                packager::encode_uri_component(&repo),
                                current_ver
                            ),
                            _ => format!("{}/{}/releases/download/v{}", host, repo, current_ver),
                        }
                    };

                    let mf_req = ManifestRequest {
                        project_id: project.id.clone(),
                        version: current_ver.clone(),
                        channel: version::resolve_release_channel(&req.channel, &current_ver).channel.to_string(),
                        url_mode: if !repo.contains('/') {
                            "local".into()
                        } else if prov == "github" {
                            "github".into()
                        } else {
                            "custom".into()
                        },
                        github_repo: repo,
                        base_url: custom_base,
                        local_port: default_local_port,
                        installer_file: inst,
                        portable_file: port,
                        extra_files: vec![],
                        min_version: String::new(),
                        notes,
                    };
                    match packager::generate_manifest(&release_dir, &mf_req) {
                        Ok(_) => {
                            if req.run_build {
                                sync_version_bucket_snapshot(&project, &current_ver, &other_roots);
                            }
                            emit_log(&app, &pid, "system", "✓ 已生成 latest.json 与 SHA256SUMS.txt");
                            emit_step(i, name, "ok", "清单与校验表已就绪");
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            emit_log(&app, &pid, "stderr", &msg);
                            emit_step(i, name, "failed", &msg);
                            finish_pipeline(false, false, msg);
                            return;
                        }
                    }
                }
                "git" => {
                    if cancel_flag.load(Ordering::SeqCst) {
                        emit_step(i, name, "failed", "已取消");
                        finish_pipeline(false, true, "流水线已取消（未执行 Git 操作）".into());
                        return;
                    }
                    let tag = format!("v{}", current_ver);
                    let commit_msg = if req.git_commit_message.trim().is_empty() {
                        format!("chore(release): {}", tag)
                    } else {
                        req.git_commit_message.trim().to_string()
                    };
                    emit_log(&app, &pid, "system", &format!("▶ 执行 Git 自动化（模式：{}，已自动排除 release 产物目录）", req.git_mode));
                    let commit_out = match git_ops::git_commit_for_release(&root, &project.release_dir, &commit_msg) {
                        Ok(msg) => {
                            if req.run_build {
                                sync_version_bucket_snapshot(&project, &current_ver, &other_roots);
                            }
                            emit_log(&app, &pid, "stdout", &format!("✓ {}", msg));
                            msg
                        }
                        Err(e) => {
                            let msg = format!("Git 提交失败：{}", e);
                            emit_log(&app, &pid, "stderr", &msg);
                            emit_step(i, name, "failed", &msg);
                            finish_pipeline(false, false, msg);
                            return;
                        }
                    };
                    if cancel_flag.load(Ordering::SeqCst) {
                        emit_step(i, name, "failed", "已取消（跳过打标与推送）");
                        finish_pipeline(false, true, "流水线已取消（未推送远端）".into());
                        return;
                    }
                    let tag_out = match git_ops::git_ensure_tag(&root, &tag, &format!("Release {}", tag)) {
                        Ok(msg) => {
                            emit_log(&app, &pid, "stdout", &format!("✓ {}", msg));
                            msg
                        }
                        Err(e) => {
                            let msg = format!("Git 打标失败：{}", e);
                            emit_log(&app, &pid, "stderr", &msg);
                            emit_step(i, name, "failed", &msg);
                            finish_pipeline(false, false, msg);
                            return;
                        }
                    };
                    if req.git_mode == "commit_tag_push" {
                        if cancel_flag.load(Ordering::SeqCst) {
                            emit_step(i, name, "failed", "已取消（跳过推送远端）");
                            finish_pipeline(false, true, "流水线已取消（未推送远端）".into());
                            return;
                        }
                        let push_res = git_ops::git_push_with_log(&root, true, |_pct, stage, line| {
                            if stage == "branch_pushed" {
                                if let Ok(mut snap) = tx_snap.lock() {
                                    snap.mark_remote_pushed();
                                }
                            }
                            emit_log(&app, &pid, "stdout", line);
                        });
                        match push_res {
                            Ok(push_msg) => {
                                if let Ok(mut snap) = tx_snap.lock() {
                                    snap.mark_remote_pushed();
                                }
                                if req.run_build {
                                    sync_version_bucket_snapshot(&project, &current_ver, &other_roots);
                                }
                                emit_step(i, name, "ok", &format!("{} · {} · {}", commit_out, tag_out, push_msg));
                            }
                            Err(e) => {
                                let msg = format!("Git 推送失败：{}", e);
                                emit_log(&app, &pid, "stderr", &msg);
                                emit_step(i, name, "failed", &msg);
                                finish_pipeline(false, false, msg);
                                return;
                            }
                        }
                    } else {
                        if req.run_build {
                            sync_version_bucket_snapshot(&project, &current_ver, &other_roots);
                        }
                        emit_step(i, name, "ok", &format!("{} · {}", commit_out, tag_out));
                    }
                }
                "publish" => {
                    if cancel_flag.load(Ordering::SeqCst) {
                        emit_step(i, name, "failed", "已取消（跳过发布）");
                        finish_pipeline(false, true, "流水线已取消（未上传 GitHub）".into());
                        return;
                    }
                    let artifacts = packager::scan_artifacts(&release_dir);
                    let mut files = Vec::new();
                    for a in artifacts {
                        if !a.is_archived
                            && matches!(a.kind.as_str(), "installer" | "portable")
                            && a.version.as_deref() == Some(current_ver.as_str())
                        {
                            files.push(a.name);
                        }
                    }
                    // Upload checksums then latest.json strictly AFTER binary artifacts
                    for fixed in ["SHA256SUMS.txt", "latest.json"] {
                        if release_dir.join(fixed).exists() {
                            files.push(fixed.to_string());
                        }
                    }
                    let notes = if let Some(ref cached) = resolved_pipeline_notes {
                        cached.clone()
                    } else {
                        let detail = resolve_release_notes_detail_for_project(
                            &project,
                            &current_ver,
                            &other_roots,
                        );
                        emit_log(
                            &app,
                            &pid,
                            "system",
                            &format!("✓ 已自动提取更新日志（来源：{}）", detail.source_label),
                        );
                        if !detail.is_fallback {
                            let mut b = storage::load_version_bucket(&project.id, &current_ver);
                            if b.notes_draft.trim().is_empty() {
                                b.notes_draft = detail.notes.clone();
                                b.updated_at =
                                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                                let _ = storage::save_version_bucket(&project.id, &b);
                            }
                        }
                        resolved_pipeline_notes = Some(detail.notes.clone());
                        detail.notes
                    };
                    if !version::is_bare_default_release_note(&req.release_notes)
                        && !version::is_bare_default_release_note(&notes)
                    {
                        let mut b = storage::load_version_bucket(&project.id, &current_ver);
                        b.notes_draft = notes.clone();
                        b.updated_at =
                            chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                        let _ = storage::save_version_bucket(&project.id, &b);
                    }
                    if files.iter().any(|f| f.eq_ignore_ascii_case("latest.json")) {
                        sync_latest_json_notes_if_matching(&release_dir, &current_ver, &notes);
                    }
                    let repo = if !req.github_repo.trim().is_empty() {
                        req.github_repo.clone()
                    } else if !project.repo_binding.repo_slug.trim().is_empty() {
                        project.repo_binding.repo_slug.clone()
                    } else {
                        project.github_repo.clone()
                    };
                    let prov = if !req.repo_provider.trim().is_empty() {
                        req.repo_provider.clone()
                    } else if !project.repo_binding.provider.trim().is_empty() {
                        project.repo_binding.provider.clone()
                    } else {
                        "github".into()
                    };
                    let base_url = if !req.repo_base_url.trim().is_empty() {
                        req.repo_base_url.clone()
                    } else {
                        project.repo_binding.base_url.clone()
                    };
                    let cred_id = if !req.credential_id.trim().is_empty() {
                        req.credential_id.clone()
                    } else {
                        project.repo_binding.credential_id.clone()
                    };
                    let tag = format!("v{}", current_ver);
                    let title = format!(
                        "{} {}",
                        if project.product_name.is_empty() {
                            &project.name
                        } else {
                            &project.product_name
                        },
                        tag
                    );
                    let pub_files = files.clone();
                    let ch_decision = version::resolve_release_channel(&req.channel, &current_ver);
                    if let Some(n) = &ch_decision.note {
                        emit_log(&app, &pid, "system", &format!("⚠ 通道裁决：{}", n));
                    }
                    emit_log(
                        &app,
                        &pid,
                        "system",
                        &format!(
                            "发布通道：{}（{}）",
                            ch_decision.channel,
                            if ch_decision.prerelease { "预发布" } else { "正式发布" }
                        ),
                    );
                    let mp_req = MultiPublishRequest {
                        project_id: project.id.clone(),
                        provider: prov.clone(),
                        base_url,
                        credential_id: cred_id,
                        repo,
                        tag,
                        title,
                        notes,
                        files,
                        prerelease: ch_decision.prerelease,
                        draft: req.draft,
                        overwrite_assets: true,
                    };
                    let app_up = app.clone();
                    let pid_up = pid.clone();
                    match repo_client::publish_release_multi(&release_dir, &mp_req, |pct, msg| {
                        let mapped = step_start
                            + (((pct as usize) * (step_end.saturating_sub(step_start) as usize))
                                / 100) as u8;
                        emit_progress(&app_up, &pid_up, "构建发布流程", "上传发布产物", msg, mapped, false);
                    }) {
                        Ok(url) => {
                            let mut b = storage::load_version_bucket(&project.id, &current_ver);
                            b.sealed = true;
                            let _ = storage::save_version_bucket(&project.id, &b);
                            let _ = artifacts::mark_artifacts_stage_with_dir(
                                &project.id,
                                Some(&release_dir),
                                &pub_files,
                                "published",
                                Some(&url),
                                Some(&prov),
                                false,
                            );
                            emit_log(&app, &pid, "system", &format!("✓ Release 已上线：{}", url));
                            emit_step(i, name, "ok", &format!("已发布 {}", url));
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            emit_log(&app, &pid, "stderr", &msg);
                            emit_step(i, name, "failed", &msg);
                            finish_pipeline(false, false, msg);
                            return;
                        }
                    }
                }
                _ => {}
            }
        }

        finish_pipeline(true, false, format!("流水线全部完成（v{}）", current_ver));
    });

    Ok(pipeline_id)
}

/// Dedicated zero-rebuild "Deploy & Push" pipeline for already-built & packaged artifacts across ANY framework.
/// Strictly runs fail-fast preflight, verifies SHA-256 checksums, ensures/syncs latest.json & SHA256SUMS.txt,
/// optionally commits/tags/pushes Git, and stream-uploads assets to GitHub / Gitee / GitLab / Gitea.
#[tauri::command]
fn deploy_existing_release(
    app: AppHandle,
    state: State<AppState>,
    req: DeployPushRequest,
) -> Result<String, String> {
    let project = find_project(&state, &req.project_id)?;
    ensure_root_exists(&project)?;
    let root = PathBuf::from(&project.root);
    let release_dir = config::resolve_release_dir(&project);
    let other_roots = other_project_roots(&state, &project.id);
    let (guardrail_settings, default_local_port) = state
        .config
        .lock()
        .map(|c| (c.guardrails.clone(), c.default_port))
        .unwrap_or_else(|_| (GuardrailSettings::default(), 18765));

    // Fail-fast preflight BEFORE acquiring background thread or mutating anything
    let versions = version::read_project_versions(&root, &project.version_files);
    let git = git_ops::inspect_git_for_project(&root, &project.release_dir);
    let artifacts_all = packager::scan_artifacts(&release_dir);
    let (rep_now, _) = compute_change_report_for_project(&project, None, &other_roots);
    let readiness = rep_now.deploy_readiness.clone().unwrap_or_else(|| {
        deploy::inspect_deploy_readiness(&project, &versions, &git, &artifacts_all, &rep_now, rep_now.engine_stale)
    });
    let target_ver = if req.version.trim().is_empty() {
        versions.current.clone()
    } else {
        req.version
            .trim()
            .trim_start_matches('v')
            .trim_start_matches('V')
            .to_string()
    };
    let selected_artifacts =
        deploy::select_deploy_artifacts(&release_dir, &artifacts_all, &target_ver, &req.artifact_files)
            .map_err(|e| e.to_string())?;
    deploy::preflight_deploy_push(
        &project,
        &req,
        &readiness,
        &versions,
        &git,
        &release_dir,
        &selected_artifacts,
    )
    .map_err(|e| e.to_string())?;

    let pipeline_id = format!("deploy-{}", chrono::Local::now().timestamp_millis());
    runner::register_task_project(&pipeline_id, &project.id);
    let lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &pipeline_id)
        .map_err(|e| e.to_string())?;

    let cancel_flag = state.runner.register_pipeline(&pipeline_id);
    let runner = state.runner.clone();
    let watch_state = state.watch_state.clone();
    let pid = pipeline_id.clone();
    let proj_id = project.id.clone();

    // Begin transaction snapshot with build_verified=true so pre-built release/ binaries are NEVER deleted on network error
    let mut snap_init = transaction::begin_transaction(&project, "deploy").map_err(|e| {
        runner.unregister_pipeline(&pid);
        format!("初始化部署事务快照失败：{}", e)
    })?;
    snap_init.mark_build_verified();
    let tx_snap = Arc::new(Mutex::new(snap_init));

    std::thread::spawn(move || {
        let _lock_guard = lock_guard;
        let started = Instant::now();
        let finish_deploy = |success: bool, cancelled: bool, summary: String| {
            let mut rollback_proof: Option<RollbackVerification> = None;
            let mut final_summary = summary.clone();
            let snap_guard = tx_snap.lock().ok();
            if success {
                if let Some(ref snap) = snap_guard {
                    transaction::commit_transaction(snap);
                }
            } else if let Some(ref snap) = snap_guard {
                let trigger = if cancelled { "用户中止部署" } else { "部署步骤失败" };
                let proof = transaction::rollback_and_verify(snap, trigger);
                final_summary = format!(
                    "{}（已完整保留本地成品与版本状态，可直接重试部署推送）",
                    summary
                );
                rollback_proof = Some(proof);
            }

            runner.unregister_pipeline(&pid);
            runner.release_project(&proj_id, &pid);
            if success {
                let (rep_post, fs_post) =
                    compute_change_report_for_project(&project, None, &other_roots);
                if let Ok(mut ws) = watch_state.lock() {
                    ws.insert(
                        proj_id.clone(),
                        ProjectWatchSnapshot {
                            fingerprint: rep_post.fingerprint,
                            version: rep_post.current_version,
                            artifact_fp: rep_post.artifact_fingerprint,
                            fs_snapshot: fs_post,
                        },
                    );
                }
            }
            let elapsed_ms = started.elapsed().as_millis() as u64;
            emit_progress(
                &app,
                &pid,
                "部署推送流程",
                if success {
                    "已完成"
                } else if cancelled {
                    "已取消（保留成品）"
                } else {
                    "已中止（保留成品）"
                },
                &final_summary,
                if success { 100 } else { 0 },
                false,
            );
            let _ = app.emit(
                "pipeline-finished",
                &PipelineFinishedEvent {
                    pipeline_id: pid.clone(),
                    project_id: proj_id.clone(),
                    success,
                    cancelled,
                    summary: final_summary.clone(),
                    duration_ms: elapsed_ms,
                },
            );
            let report = health_report::build_ephemeral_report(
                &project,
                "deploy",
                if success { "ok" } else { "rolled_back" },
                elapsed_ms,
                final_summary,
                rollback_proof,
                &guardrail_settings,
            );
            let _ = app.emit("health-report-once", &report);
        };

        let mut steps: Vec<(&str, &str)> = vec![("verify_manifest", "校验产物与更新清单")];
        if req.git_mode != "none" {
            steps.push(("git_deploy", "Git 标签与远端推送"));
        }
        if req.upload_release {
            let prov_label = match project.repo_binding.provider.as_str() {
                "gitee" => "Gitee",
                "gitlab" => "GitLab",
                "gitea" => "Gitea",
                _ => "GitHub",
            };
            steps.push((
                "upload_release",
                if prov_label == "Gitee" {
                    "发布 Gitee Release"
                } else if prov_label == "GitLab" {
                    "发布 GitLab Release"
                } else if prov_label == "Gitea" {
                    "发布 Gitea Release"
                } else {
                    "发布 GitHub Release"
                },
            ));
        }

        let total = steps.len();
        let emit_step = |idx: usize, name: &str, status: &str, msg: &str| {
            let _ = app.emit(
                "pipeline-step",
                &PipelineStepEvent {
                    pipeline_id: pid.clone(),
                    project_id: proj_id.clone(),
                    index: idx,
                    total,
                    name: name.to_string(),
                    status: status.to_string(),
                    message: msg.to_string(),
                },
            );
        };

        let repo_slug = if !project.repo_binding.repo_slug.trim().is_empty() {
            project.repo_binding.repo_slug.clone()
        } else {
            project.github_repo.clone()
        };
        let prov = if !project.repo_binding.provider.trim().is_empty() {
            project.repo_binding.provider.clone()
        } else {
            "github".into()
        };
        let base_url = project.repo_binding.base_url.clone();
        let cred_id = project.repo_binding.credential_id.clone();

        // Resolve effective release notes ONCE so latest.json and online Release stay 100% byte-identical
        let effective_notes = if !version::is_bare_default_release_note(&req.release_notes) {
            version::sanitize_public_release_notes(&req.release_notes)
        } else {
            let detail = resolve_release_notes_detail_for_project(&project, &target_ver, &other_roots);
            emit_log(
                &app,
                &pid,
                "system",
                &format!("✓ 已自动读取发布说明（来源：{}）", detail.source_label),
            );
            detail.notes
        };

        let mut upload_files: Vec<String> = selected_artifacts.iter().map(|a| a.name.clone()).collect();

        for (i, (key, name)) in steps.iter().enumerate() {
            if cancel_flag.load(Ordering::SeqCst) {
                emit_step(i, name, "failed", "已取消");
                finish_deploy(false, true, format!("已在「{}」前取消部署", name));
                return;
            }
            let step_start = ((i * 95) / total).max(5) as u8;
            let step_end = (((i + 1) * 95) / total).min(98) as u8;
            emit_step(i, name, "running", "执行中…");
            emit_progress(
                &app,
                &pid,
                "部署推送流程",
                &format!("[{}/{}] {}", i + 1, total, name),
                "正在执行…",
                step_start,
                false,
            );
            emit_log(&app, &pid, "system", &format!("=== 部署推送 [{}/{}] {} ===", i + 1, total, name));

            match *key {
                "verify_manifest" => {
                    // 1. Verify every selected binary file exists, is non-empty, and record SHA-256
                    let mut verified_count = 0usize;
                    for a in &selected_artifacts {
                        match packager::sha256_file(Path::new(&a.path)) {
                            Ok(h) => {
                                verified_count += 1;
                                emit_log(
                                    &app,
                                    &pid,
                                    "stdout",
                                    &format!("✓ 校验通过：{} (sha256: {}…)", a.name, &h[..12]),
                                );
                            }
                            Err(e) => {
                                let msg = format!("产物哈希校验失败（{}）：{}", a.name, e);
                                emit_log(&app, &pid, "stderr", &msg);
                                emit_step(i, name, "failed", &msg);
                                finish_deploy(false, false, msg);
                                return;
                            }
                        }
                    }

                    // 2. Ensure/refresh latest.json & SHA256SUMS.txt across ALL framework artifact layouts
                    if req.ensure_manifest && !target_ver.is_empty() && target_ver != "0.0.0" {
                        let inst = selected_artifacts
                            .iter()
                            .find(|a| a.kind == "installer")
                            .map(|a| a.name.clone());
                        let port = selected_artifacts
                            .iter()
                            .find(|a| a.kind == "portable")
                            .map(|a| a.name.clone());
                        let extras: Vec<String> = selected_artifacts
                            .iter()
                            .filter(|a| {
                                inst.as_deref() != Some(a.name.as_str())
                                    && port.as_deref() != Some(a.name.as_str())
                            })
                            .map(|a| a.name.clone())
                            .collect();

                        let custom_base = if prov == "github" || !repo_slug.contains('/') {
                            String::new()
                        } else {
                            let host = if !base_url.trim().is_empty() {
                                base_url.trim().trim_end_matches('/')
                            } else {
                                match prov.as_str() {
                                    "gitee" => "https://gitee.com",
                                    "gitlab" => "https://gitlab.com",
                                    "gitea" => "https://gitea.com",
                                    _ => "https://github.com",
                                }
                            };
                            match prov.as_str() {
                                "gitlab" => format!(
                                    "{}/api/v4/projects/{}/packages/generic/release/{}",
                                    host,
                                    packager::encode_uri_component(&repo_slug),
                                    target_ver
                                ),
                                _ => format!("{}/{}/releases/download/v{}", host, repo_slug, target_ver),
                            }
                        };

                        let mf_req = ManifestRequest {
                            project_id: project.id.clone(),
                            version: target_ver.clone(),
                            channel: version::resolve_release_channel(&req.channel, &target_ver).channel.to_string(),
                            url_mode: if !repo_slug.contains('/') {
                                "local".into()
                            } else if prov == "github" {
                                "github".into()
                            } else {
                                "custom".into()
                            },
                            github_repo: repo_slug.clone(),
                            base_url: custom_base,
                            local_port: default_local_port,
                            installer_file: inst,
                            portable_file: port,
                            extra_files: extras,
                            min_version: String::new(),
                            notes: effective_notes.clone(),
                        };

                        match packager::generate_manifest(&release_dir, &mf_req) {
                            Ok(_) => {
                                emit_log(&app, &pid, "system", "✓ 已生成/同步 latest.json 与 SHA256SUMS.txt");
                            }
                            Err(e) => {
                                let msg = format!("生成或同步更新清单失败：{}", e);
                                emit_log(&app, &pid, "stderr", &msg);
                                emit_step(i, name, "failed", &msg);
                                finish_deploy(false, false, msg);
                                return;
                            }
                        }
                    } else if release_dir.join("latest.json").is_file() {
                        sync_latest_json_notes_if_matching(&release_dir, &target_ver, &effective_notes);
                    }

                    // 3. Re-verify checksums strictly when requested
                    if req.verify_checksums && release_dir.join("SHA256SUMS.txt").is_file() {
                        match packager::verify_checksums(&release_dir) {
                            Ok(items) => {
                                let bad: Vec<String> = items
                                    .iter()
                                    .filter(|it| matches!(it.status.as_str(), "mismatch" | "missing" | "invalid"))
                                    .map(|it| format!("{} ({})", it.file, it.status))
                                    .collect();
                                if !bad.is_empty() {
                                    let msg = format!("SHA256SUMS.txt 复检失败：{}", bad.join("、"));
                                    emit_log(&app, &pid, "stderr", &msg);
                                    emit_step(i, name, "failed", &msg);
                                    finish_deploy(false, false, msg);
                                    return;
                                }
                            }
                            Err(e) => {
                                let msg = format!("校验表核验异常：{}", e);
                                emit_log(&app, &pid, "stderr", &msg);
                                emit_step(i, name, "failed", &msg);
                                finish_deploy(false, false, msg);
                                return;
                            }
                        }
                    }

                    for fixed in ["SHA256SUMS.txt", "latest.json"] {
                        if release_dir.join(fixed).is_file()
                            && !upload_files.iter().any(|x| x.eq_ignore_ascii_case(fixed))
                        {
                            upload_files.push(fixed.to_string());
                        }
                    }

                    let _ = artifacts::mark_artifacts_stage_with_dir(
                        &project.id,
                        Some(&release_dir),
                        &upload_files,
                        "ready",
                        None,
                        None,
                        false,
                    );
                    emit_step(
                        i,
                        name,
                        "ok",
                        &format!("{} 个成品哈希与清单核验通过", verified_count),
                    );
                }
                "git_deploy" => {
                    let tag = format!("v{}", target_ver);
                    let mut summary_parts = Vec::new();
                    if req.git_mode == "commit_tag_push" {
                        let commit_msg = if req.git_commit_message.trim().is_empty() {
                            format!("chore(release): {}", tag)
                        } else {
                            req.git_commit_message.trim().to_string()
                        };
                        match git_ops::git_commit_for_release(&root, &project.release_dir, &commit_msg) {
                            Ok(msg) => {
                                emit_log(&app, &pid, "stdout", &format!("✓ {}", msg));
                                summary_parts.push(msg);
                            }
                            Err(e) => {
                                let msg = format!("Git 提交失败：{}", e);
                                emit_log(&app, &pid, "stderr", &msg);
                                emit_step(i, name, "failed", &msg);
                                finish_deploy(false, false, msg);
                                return;
                            }
                        }
                    }

                    if !target_ver.is_empty() && target_ver != "0.0.0" {
                        match git_ops::git_ensure_tag(&root, &tag, &format!("Release {}", tag)) {
                            Ok(msg) => {
                                emit_log(&app, &pid, "stdout", &format!("✓ {}", msg));
                                summary_parts.push(msg);
                            }
                            Err(e) => {
                                let msg = format!("Git 打标失败：{}", e);
                                emit_log(&app, &pid, "stderr", &msg);
                                emit_step(i, name, "failed", &msg);
                                finish_deploy(false, false, msg);
                                return;
                            }
                        }
                    }

                    if cancel_flag.load(Ordering::SeqCst) {
                        emit_step(i, name, "failed", "已取消（跳过推送远端）");
                        finish_deploy(false, true, "部署已取消（未推送远端）".into());
                        return;
                    }

                    match git_ops::git_push_with_log(&root, true, |_pct, stage, line| {
                        if stage == "branch_pushed" {
                            if let Ok(mut snap) = tx_snap.lock() {
                                snap.mark_remote_pushed();
                            }
                        }
                        emit_log(&app, &pid, "stdout", line);
                    }) {
                        Ok(push_msg) => {
                            if let Ok(mut snap) = tx_snap.lock() {
                                snap.mark_remote_pushed();
                            }
                            summary_parts.push(push_msg);
                            emit_step(i, name, "ok", &summary_parts.join(" · "));
                        }
                        Err(e) => {
                            let msg = format!("Git 推送失败：{}", e);
                            emit_log(&app, &pid, "stderr", &msg);
                            emit_step(i, name, "failed", &msg);
                            finish_deploy(false, false, msg);
                            return;
                        }
                    }
                }
                "upload_release" => {
                    let tag = format!("v{}", target_ver);
                    let title = format!(
                        "{} {}",
                        if project.product_name.is_empty() {
                            &project.name
                        } else {
                            &project.product_name
                        },
                        tag
                    );
                    let ch_decision = version::resolve_release_channel(&req.channel, &target_ver);
                    if let Some(n) = &ch_decision.note {
                        emit_log(&app, &pid, "stderr", &format!("⚠ 通道裁决：{}", n));
                    }
                    emit_log(
                        &app,
                        &pid,
                        "stdout",
                        &format!(
                            "发布通道：{}（{}）",
                            ch_decision.channel,
                            if ch_decision.prerelease { "预发布" } else { "正式发布" }
                        ),
                    );
                    let mp_req = MultiPublishRequest {
                        project_id: project.id.clone(),
                        provider: prov.clone(),
                        base_url: base_url.clone(),
                        credential_id: cred_id.clone(),
                        repo: repo_slug.clone(),
                        tag: tag.clone(),
                        title,
                        notes: effective_notes.clone(),
                        files: upload_files.clone(),
                        prerelease: ch_decision.prerelease,
                        draft: req.draft,
                        overwrite_assets: req.overwrite_assets,
                    };
                    match repo_client::publish_release_multi(&release_dir, &mp_req, |pct, msg| {
                        let mapped = step_start
                            + (((step_end - step_start) as u16 * pct as u16) / 100) as u8;
                        emit_progress(&app, &pid, "部署推送流程", name, msg, mapped, false);
                    }) {
                        Ok(url) => {
                            let _ = artifacts::mark_artifacts_stage_with_dir(
                                &project.id,
                                Some(&release_dir),
                                &upload_files,
                                "published",
                                Some(&url),
                                Some(&prov),
                                false,
                            );
                            sync_version_bucket_snapshot(&project, &target_ver, &other_roots);
                            let mut b = storage::load_version_bucket(&project.id, &target_ver);
                            b.sealed = true;
                            if !version::is_bare_default_release_note(&effective_notes) {
                                b.notes_draft = effective_notes.clone();
                            }
                            let _ = storage::save_version_bucket(&project.id, &b);
                            emit_log(&app, &pid, "stdout", &format!("✓ Release 已发布：{}", url));
                            emit_step(i, name, "ok", &format!("已发布 {}", url));
                        }
                        Err(e) => {
                            let msg = format!("发布失败：{}", e);
                            emit_log(&app, &pid, "stderr", &msg);
                            emit_step(i, name, "failed", &msg);
                            finish_deploy(false, false, msg);
                            return;
                        }
                    }
                }
                _ => {}
            }
        }

        // If user only ran Git push (no Release upload), still sync bucket snapshot cleanly
        if !req.upload_release {
            sync_version_bucket_snapshot(&project, &target_ver, &other_roots);
        }

        finish_deploy(true, false, format!("部署推送全部完成（v{}）", target_ver));
    });

    Ok(pipeline_id)
}

#[tauri::command]
fn cancel_task(state: State<AppState>, task_id: String) -> Result<bool, String> {
    state.runner.cancel(&task_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_active_tasks(state: State<AppState>) -> Vec<String> {
    state.runner.active_ids()
}

// =========================================================================
// Packaging, Manifest, Server, Git & GitHub Commands
// =========================================================================

#[tauri::command]
async fn pack_portable_archive(
    app: AppHandle,
    state: State<'_, AppState>,
    req: PackageRequest,
) -> Result<PackageResult, String> {
    let project = find_project(&state, &req.project_id)?;
    ensure_root_exists(&project)?;
    let guardrail_settings = state
        .config
        .lock()
        .map(|c| c.guardrails.clone())
        .unwrap_or_default();
    let release_dir = config::resolve_release_dir(&project);
    guardrails::ensure_disk_headroom(&release_dir, 128 * 1024 * 1024, &guardrail_settings)
        .map_err(|e| e.to_string())?;

    let root = PathBuf::from(&project.root);
    let task_id = format!("pack-{}", chrono::Local::now().timestamp_millis());
    runner::register_task_project(&task_id, &project.id);
    let _lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &task_id)
        .map_err(|e| format!("当前项目正在执行其他任务，请等待完成后再打包：{}", e))?;
    let app_c = app.clone();
    let tid = task_id.clone();
    let proj_clone = project.clone();

    let res = tauri::async_runtime::spawn_blocking(move || {
        packager::pack_portable_with_progress(&root, &release_dir, &req, |pct, detail| {
            emit_progress(&app_c, &tid, "分包压缩", "打包压缩", detail, pct, false);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let _ = artifacts::mark_artifacts_stage(
        &proj_clone.id,
        &[res.file_name.clone()],
        "pending_verify",
        None,
        None,
    );

    let report = health_report::build_ephemeral_report(
        &proj_clone,
        "pack",
        "ok",
        res.duration_ms,
        format!(
            "打包完成：{}（{} 个文件{}）",
            res.file_name,
            res.file_count,
            if res.split_parts.is_empty() {
                String::new()
            } else {
                format!("，已切分 {} 卷", res.split_parts.len())
            }
        ),
        None,
        &guardrail_settings,
    );
    let _ = app.emit("health-report-once", &report);
    Ok(res)
}

#[tauri::command]
fn list_project_entries(state: State<AppState>, project_id: String) -> Result<Vec<String>, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let root = Path::new(&project.root);
    let other_roots = other_project_roots(&state, &project.id);
    let mut out = Vec::new();
    let skip = [".git", "node_modules", "target", ".build", "__pycache__"];
    if let Ok(rd) = std::fs::read_dir(root) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if skip.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                continue;
            }
            if storage::is_under_other_project(root, &name, &other_roots) {
                continue;
            }
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

#[tauri::command]
async fn compute_artifact_sha256(
    state: State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    let target = is_path_within_allowed_roots(&state, Path::new(&path))?;
    if !target.is_file() {
        return Err("目标路径不是普通文件".into());
    }
    tauri::async_runtime::spawn_blocking(move || packager::sha256_file(&target))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn generate_release_manifest(
    state: State<'_, AppState>,
    mut req: ManifestRequest,
) -> Result<ManifestResult, String> {
    let project = find_project(&state, &req.project_id)?;
    let task_id = format!("mf-{}", chrono::Local::now().timestamp_millis());
    let _lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &task_id)
        .map_err(|e| format!("当前项目正在执行其他任务，请等待完成后再生成清单：{}", e))?;
    if version::is_bare_default_release_note(&req.notes) {
        let other_roots = other_project_roots(&state, &project.id);
        req.notes = resolve_release_notes_for_project(&project, &req.version, &other_roots);
    } else {
        req.notes = version::sanitize_public_release_notes(&req.notes);
    }
    let release_dir = config::resolve_release_dir(&project);
    tauri::async_runtime::spawn_blocking(move || packager::generate_manifest(&release_dir, &req))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn verify_release_checksums(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<ChecksumVerifyItem>, String> {
    let project = find_project(&state, &project_id)?;
    let release_dir = config::resolve_release_dir(&project);
    tauri::async_runtime::spawn_blocking(move || packager::verify_checksums(&release_dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn archive_release_artifact(
    state: State<AppState>,
    project_id: String,
    file_name: String,
) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    let tid = format!("arch-{}", chrono::Local::now().timestamp_millis());
    let _lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &tid)
        .map_err(|e| format!("当前项目正在执行任务，请等待完成后再归档产物：{}", e))?;
    let release_dir = config::resolve_release_dir(&project);
    packager::archive_artifact(&release_dir, &file_name).map_err(|e| e.to_string())
}

#[tauri::command]
fn prune_old_release_artifacts(
    state: State<AppState>,
    project_id: String,
    keep_versions: usize,
) -> Result<Vec<String>, String> {
    let project = find_project(&state, &project_id)?;
    let tid = format!("prune-{}", chrono::Local::now().timestamp_millis());
    let _lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &tid)
        .map_err(|e| format!("当前项目正在执行任务，请等待完成后再清理旧产物：{}", e))?;
    let release_dir = config::resolve_release_dir(&project);
    packager::prune_old_releases(&release_dir, keep_versions).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_local_server_status(state: State<AppState>) -> Result<LocalServerStatus, String> {
    let guard = state.server.lock().map_err(|_| "服务锁异常".to_string())?;
    Ok(guard.status())
}

#[tauri::command]
fn start_local_server(
    app: AppHandle,
    state: State<AppState>,
    project_id: String,
    port: u16,
    rewrite_manifest: Option<bool>,
) -> Result<LocalServerStatus, String> {
    let project = find_project(&state, &project_id)?;
    let release_dir = config::resolve_release_dir(&project);
    let mut guard = state.server.lock().map_err(|_| "服务锁异常".to_string())?;
    guard
        .start_for_project(
            project.id.clone(),
            project.name.clone(),
            release_dir,
            port,
            rewrite_manifest.unwrap_or(true),
            Some(app),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn stop_local_server(state: State<AppState>) -> Result<LocalServerStatus, String> {
    let mut guard = state.server.lock().map_err(|_| "服务锁异常".to_string())?;
    guard.stop();
    Ok(guard.status())
}

#[tauri::command]
fn launch_project_exe(
    state: State<AppState>,
    project_id: String,
    update_feed_url: Option<String>,
) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    if project.launch_exe.trim().is_empty() {
        return Err("该项目未配置可启动程序路径".into());
    }
    let exe = guardrails::resolve_confined_path(Path::new(&project.root), &project.launch_exe)
        .map_err(|e| format!("可执行文件路径越界或非法：{}", e))?;
    if !exe.is_file() {
        return Err(format!("找不到可执行文件：{}", exe.display()));
    }
    let mut cmd = Command::new(&exe);
    cmd.current_dir(&project.root);
    if let Some(ref feed) = update_feed_url {
        let trimmed = feed.trim();
        if !trimmed.is_empty() {
            let parsed = repo_client::parse_http_url(trimmed)
                .map_err(|e| format!("本地联调更新源 URL 无效：{}", e))?;
            if parsed.https
                || (!parsed.host.eq_ignore_ascii_case("127.0.0.1")
                    && !parsed.host.eq_ignore_ascii_case("localhost"))
            {
                return Err("本地联调更新源仅允许 http://127.0.0.1 或 http://localhost 回环地址".into());
            }
            cmd.env("TIDY_UPDATE_FEED", trimmed);
            cmd.env("SHIPFORGE_UPDATE_FEED", trimmed);
            cmd.env("UPDATE_FEED_URL", trimmed);
        }
    }
    cmd.spawn().map_err(|e| format!("启动失败：{}", e))?;
    Ok(format!("已启动 {}", exe.display()))
}

#[tauri::command]
fn open_in_explorer(state: State<AppState>, path: String) -> Result<(), String> {
    let canon = is_path_within_allowed_roots(&state, Path::new(&path))?;
    // Strip Windows extended-length prefix `\\?\` because explorer.exe mishandles it
    let display_path = canon
        .to_string_lossy()
        .strip_prefix(r"\\?\")
        .map(|s| s.to_string())
        .unwrap_or_else(|| canon.to_string_lossy().to_string());
    #[cfg(windows)]
    {
        let mut cmd = Command::new("explorer");
        if canon.is_file() {
            cmd.arg("/select,").arg(&display_path);
        } else {
            cmd.arg(&display_path);
        }
        cmd.spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn validate_external_url(url: &str) -> Result<&str, String> {
    let u = url.trim();
    let forbidden = ['"', '\'', '&', '|', ';', '<', '>', '^', '`', '$', '\r', '\n', ' '];
    if u.chars().any(|c| c.is_control() || forbidden.contains(&c)) {
        return Err("链接包含不安全字符，已拦截".into());
    }
    let parsed = repo_client::parse_http_url(u)
        .map_err(|e| format!("链接格式非法，已拦截：{}", e))?;
    if !parsed.https && !parsed.host.eq_ignore_ascii_case("127.0.0.1") {
        return Err("仅允许打开 https:// 或本机 127.0.0.1 回环链接".into());
    }
    Ok(u)
}

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    let clean = validate_external_url(&url)?;
    #[cfg(windows)]
    {
        let mut cmd = Command::new("rundll32.exe");
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.args(["url.dll,FileProtocolHandler", clean])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn git_commit_changes(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    message: String,
) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let root = PathBuf::from(&project.root);
    let release_dir_rel = project.release_dir.clone();
    let task_id = format!("git-commit-{}", chrono::Local::now().timestamp_millis());
    let lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &task_id)
        .map_err(|e| format!("当前项目正在执行任务，请等待完成后再提交 Git：{}", e))?;
    runner::register_task_project(&task_id, &project.id);
    emit_log(&app, &task_id, "system", &format!("▶ 执行 Git 提交：{}", message.trim()));
    let res = tauri::async_runtime::spawn_blocking(move || {
        let _lock_guard = lock_guard;
        git_ops::git_commit_for_release(&root, &release_dir_rel, &message)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string());
    match &res {
        Ok(msg) => emit_log(&app, &task_id, "stdout", &format!("✓ {}", msg)),
        Err(e) => emit_log(&app, &task_id, "stderr", &format!("✗ {}", e)),
    }
    res
}

#[tauri::command]
async fn git_tag_release(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    tag: String,
    message: String,
) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let root = PathBuf::from(&project.root);
    let task_id = format!("git-tag-{}", chrono::Local::now().timestamp_millis());
    let lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &task_id)
        .map_err(|e| format!("当前项目正在执行任务，请等待完成后再打标签：{}", e))?;
    runner::register_task_project(&task_id, &project.id);
    emit_log(&app, &task_id, "system", &format!("▶ 创建 Git 标签：{}", tag.trim()));
    let res = tauri::async_runtime::spawn_blocking(move || {
        let _lock_guard = lock_guard;
        git_ops::git_create_tag(&root, &tag, &message)
    })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string());
    match &res {
        Ok(msg) => emit_log(&app, &task_id, "stdout", &format!("✓ {}", msg)),
        Err(e) => emit_log(&app, &task_id, "stderr", &format!("✗ {}", e)),
    }
    res
}

#[tauri::command]
async fn git_push_remote(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    push_tags: bool,
) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let root = PathBuf::from(&project.root);
    let task_id = format!("git-push-{}", chrono::Local::now().timestamp_millis());
    let lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &task_id)
        .map_err(|e| format!("当前项目正在执行任务，请等待完成后再推送远端：{}", e))?;
    runner::register_task_project(&task_id, &project.id);
    let label = if push_tags { "推送分支与标签" } else { "推送分支到远端" };
    let app_c = app.clone();
    let tid = task_id.clone();

    emit_progress(&app, &task_id, label, "准备推送", "正在连接远端仓库…", 8, false);
    let res = tauri::async_runtime::spawn_blocking(move || {
        let _lock_guard = lock_guard;
        git_ops::git_push_with_log(&root, push_tags, |pct, stage, line| {
            let stream = if line.starts_with('✗') {
                "stderr"
            } else if line.starts_with('▶') || line.starts_with('✓') {
                "system"
            } else {
                "stdout"
            };
            emit_log(&app_c, &tid, stream, line);
            emit_progress(&app_c, &tid, label, stage, line, pct, false);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string());

    if let Err(ref e) = res {
        emit_progress(&app, &task_id, label, "推送失败", e, 100, false);
    }
    res
}

#[tauri::command]
async fn git_undo_last_commit(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let root = PathBuf::from(&project.root);
    let task_id = format!("git-undo-{}", chrono::Local::now().timestamp_millis());
    let lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &task_id)
        .map_err(|e| format!("当前项目正在执行任务，请等待完成后再撤回提交：{}", e))?;
    runner::register_task_project(&task_id, &project.id);
    emit_log(&app, &task_id, "system", "▶ 安全撤回最新未推送本地提交 (git reset --soft HEAD~1)");
    let res = tauri::async_runtime::spawn_blocking(move || {
        let _lock_guard = lock_guard;
        git_ops::git_undo_last_commit(&root)
    })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string());
    match &res {
        Ok(msg) => emit_log(&app, &task_id, "stdout", &format!("✓ {}", msg)),
        Err(e) => emit_log(&app, &task_id, "stderr", &format!("✗ {}", e)),
    }
    res
}

#[tauri::command]
async fn git_delete_tag(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    tag: String,
    delete_remote: bool,
) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let root = PathBuf::from(&project.root);
    let task_id = format!("git-deltag-{}", chrono::Local::now().timestamp_millis());
    let lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &task_id)
        .map_err(|e| format!("当前项目正在执行任务，请等待完成后再删除标签：{}", e))?;
    runner::register_task_project(&task_id, &project.id);
    emit_log(
        &app,
        &task_id,
        "system",
        &format!(
            "▶ 删除 Git 标签：{}{}",
            tag.trim(),
            if delete_remote { "（含远端）" } else { "（仅本地）" }
        ),
    );
    let res = tauri::async_runtime::spawn_blocking(move || {
        let _lock_guard = lock_guard;
        git_ops::git_delete_tag(&root, &tag, delete_remote)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string());
    match &res {
        Ok(msg) => emit_log(&app, &task_id, "stdout", &format!("✓ {}", msg)),
        Err(e) => emit_log(&app, &task_id, "stderr", &format!("✗ {}", e)),
    }
    res
}

#[tauri::command]
fn clean_stale_build_lock(state: State<AppState>, project_id: String) -> Result<String, String> {
    let project = find_project(&state, &project_id)?;
    build_guard::clean_stale_lock(&project.lock_file).map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_gh_status() -> GhStatus {
    tauri::async_runtime::spawn_blocking(git_ops::check_gh_status)
        .await
        .unwrap_or(GhStatus {
            installed: false,
            authenticated: false,
            account: String::new(),
            detail: "检查失败".into(),
        })
}

#[tauri::command]
async fn get_gh_releases(repo: String) -> Result<Vec<GhReleaseItem>, String> {
    tauri::async_runtime::spawn_blocking(move || git_ops::list_gh_releases(&repo))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn publish_to_github(
    state: State<'_, AppState>,
    req: GhPublishRequest,
) -> Result<String, String> {
    let project = find_project(&state, &req.project_id)?;
    let task_id = format!("ghpub-{}", chrono::Local::now().timestamp_millis());
    let _lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &task_id)
        .map_err(|e| format!("当前项目正在执行其他任务，请等待完成后再发布：{}", e))?;
    let release_dir = config::resolve_release_dir(&project);
    let pid = project.id.clone();
    let tag_ver = req.tag.trim().trim_start_matches('v').to_string();
    let url = tauri::async_runtime::spawn_blocking(move || {
        git_ops::publish_gh_release(&release_dir, &req)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    // Seal the published version bucket so subsequent edits trigger isolation warning
    let mut b = storage::load_version_bucket(&pid, &tag_ver);
    b.sealed = true;
    let _ = storage::save_version_bucket(&pid, &b);
    Ok(url)
}

// =========================================================================
// ShipForge v2.0 Commands (Matrix, Preflight, Analyzer, Multi-Repo, Guardrails, Recovery)
// =========================================================================

#[tauri::command]
fn set_onboarding_completed(state: State<AppState>, completed: bool) -> Result<AppConfig, String> {
    let mut cfg = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    cfg.onboarding_completed = completed;
    config::save_config(&cfg).map_err(|e| e.to_string())?;
    Ok(cfg.clone())
}

#[tauri::command]
fn update_guardrail_settings(
    state: State<AppState>,
    mut settings: GuardrailSettings,
) -> Result<AppConfig, String> {
    settings.min_free_disk_mb = settings.min_free_disk_mb.clamp(256, 10_000_000);
    settings.disk_headroom_multiplier = settings.disk_headroom_multiplier.clamp(1, 10);
    settings.max_concurrency = settings.max_concurrency.min(64);
    settings.split_warning_mb = settings.split_warning_mb.clamp(20, 2000);
    let mut cfg = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    cfg.guardrails = settings;
    config::save_config(&cfg).map_err(|e| e.to_string())?;
    Ok(cfg.clone())
}

#[tauri::command]
async fn get_system_headroom(
    state: State<'_, AppState>,
    project_id: Option<String>,
) -> Result<HeadroomSnapshot, String> {
    let (target_path, settings) = {
        let cfg = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
        let p = project_id
            .as_deref()
            .and_then(|id| cfg.projects.iter().find(|x| x.id == id))
            .map(|proj| config::resolve_release_dir(proj))
            .unwrap_or_else(storage::active_data_root);
        (p, cfg.guardrails.clone())
    };
    tauri::async_runtime::spawn_blocking(move || {
        guardrails::compute_headroom(&target_path, 128 * 1024 * 1024, &settings)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn check_crash_recovery() -> Result<CrashRecoveryResult, String> {
    tauri::async_runtime::spawn_blocking(transaction::recover_interrupted_transactions)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_project_profile_matrix(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<ProjectProfileMatrix, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let root = PathBuf::from(&project.root);
    let pid = project.id.clone();
    tauri::async_runtime::spawn_blocking(move || detector::analyze_project_profiles(&pid, &root))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_framework_presets(
    state: State<AppState>,
    project_id: String,
) -> Result<Vec<presets::FrameworkPreset>, String> {
    let project = find_project(&state, &project_id)?;
    Ok(presets::list_presets_for_project(&project))
}

#[tauri::command]
fn apply_framework_preset(
    state: State<AppState>,
    project_id: String,
    preset_id: String,
) -> Result<ProjectConfig, String> {
    let mut cfg = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    let proj = cfg
        .projects
        .iter_mut()
        .find(|p| p.id == project_id)
        .ok_or_else(|| "未找到项目".to_string())?;
    presets::apply_preset_to_project(proj, &preset_id)?;
    let updated = proj.clone();
    config::save_config(&cfg).map_err(|e| e.to_string())?;
    Ok(updated)
}

#[tauri::command]
fn apply_project_profile(
    state: State<AppState>,
    project_id: String,
    language: String,
    shell: String,
) -> Result<ProjectConfig, String> {
    let mut cfg = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    let proj = cfg
        .projects
        .iter_mut()
        .find(|p| p.id == project_id)
        .ok_or_else(|| "未找到项目".to_string())?;
    detector::apply_profile_override(proj, &language, &shell);
    let updated = proj.clone();
    config::save_config(&cfg).map_err(|e| e.to_string())?;
    Ok(updated)
}

#[tauri::command]
async fn inspect_project_prerequisites(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<PrereqReport, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let settings = state
        .config
        .lock()
        .map(|c| c.guardrails.clone())
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        preflight::inspect_project_prerequisites(&project, &settings)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn plan_prerequisite_repair(
    state: State<'_, AppState>,
    project_id: String,
    selected_ids: Vec<String>,
) -> Result<RepairPlan, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let settings = state
        .config
        .lock()
        .map(|c| c.guardrails.clone())
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        preflight::plan_prerequisite_repair(&project, &selected_ids, &settings)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn execute_prerequisite_repair(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    approved_ids: Vec<String>,
) -> Result<RepairResult, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let (settings, roots) = {
        let cfg = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
        (
            cfg.guardrails.clone(),
            cfg.projects.iter().map(|p| p.root.clone()).collect::<Vec<_>>(),
        )
    };
    let task_id = format!("prereq-{}", chrono::Local::now().timestamp_millis());
    runner::register_task_project(&task_id, &project.id);
    let lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &task_id)
        .map_err(|e| e.to_string())?;
    let tid = task_id.clone();

    let res = tauri::async_runtime::spawn_blocking(move || {
        let _lock_guard = lock_guard;
        preflight::execute_prerequisite_repair(
            &app,
            &tid,
            &project,
            &approved_ids,
            &roots,
            &settings,
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    Ok(res)
}

#[tauri::command]
async fn analyze_project_files(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<ProjectFileAnalysis, String> {
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let root = PathBuf::from(&project.root);
    let pid = project.id.clone();
    let other_roots = other_project_roots(&state, &project.id);
    tauri::async_runtime::spawn_blocking(move || {
        analyzer::analyze_project_files_isolated(&pid, &root, &other_roots)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn save_package_profiles(
    state: State<AppState>,
    project_id: String,
    mut profiles: Vec<PackageProfile>,
) -> Result<ProjectConfig, String> {
    let mut cfg = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    let proj = cfg
        .projects
        .iter_mut()
        .find(|p| p.id == project_id)
        .ok_or_else(|| "未找到项目".to_string())?;
    let root = Path::new(&proj.root);
    for p in &mut profiles {
        p.name = p.name.trim().to_string();
        if p.name.is_empty() {
            return Err("分包预设名称不能为空".into());
        }
        if !matches!(p.format.as_str(), "zip" | "targz") {
            p.format = "zip".into();
        }
        if let Some(mb) = p.split_size_mb {
            if mb == 0 || mb > 4096 {
                return Err(format!("预设「{}」分卷大小须在 1 ~ 4096 MB 之间", p.name));
            }
        }
        for src in &p.sources {
            if !src.trim().is_empty() {
                guardrails::resolve_confined_path(root, src)
                    .map_err(|e| format!("预设「{}」包含非法或越界源路径 {}：{}", p.name, src, e))?;
            }
        }
    }
    proj.package_profiles = profiles;
    let updated = proj.clone();
    config::save_config(&cfg).map_err(|e| e.to_string())?;
    Ok(updated)
}

#[tauri::command]
async fn list_classified_artifacts(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<ClassifiedArtifact>, String> {
    let project = find_project(&state, &project_id)?;
    let release_dir = config::resolve_release_dir(&project);
    let root = PathBuf::from(&project.root);
    let vfiles = project.version_files.clone();
    let pid = project.id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ver = version::read_project_versions(&root, &vfiles).current;
        artifacts::classify_artifacts(&pid, &release_dir, &ver)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn mark_artifact_stage(
    state: State<'_, AppState>,
    project_id: String,
    file_names: Vec<String>,
    stage: String,
) -> Result<Vec<ClassifiedArtifact>, String> {
    if !matches!(
        stage.as_str(),
        "pending_verify" | "verified" | "ready" | "published" | "archived"
    ) {
        return Err(format!("不支持的产物阶段：{}", stage));
    }
    let project = find_project(&state, &project_id)?;
    let release_dir = config::resolve_release_dir(&project);
    let root = PathBuf::from(&project.root);
    let vfiles = project.version_files.clone();
    let pid = project.id.clone();
    let allow_downgrade = stage == "pending_verify" || stage == "archived";
    tauri::async_runtime::spawn_blocking(move || {
        artifacts::mark_artifacts_stage_with_dir(
            &pid,
            Some(&release_dir),
            &file_names,
            &stage,
            None,
            None,
            allow_downgrade,
        )?;
        let ver = version::read_project_versions(&root, &vfiles).current;
        Ok::<Vec<ClassifiedArtifact>, anyhow::Error>(artifacts::classify_artifacts(
            &pid,
            &release_dir,
            &ver,
        ))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_repo_credentials() -> Result<Vec<RepoCredentialMeta>, String> {
    credentials::list_credentials().map_err(|e| e.to_string())
}

#[tauri::command]
async fn verify_and_save_repo_credential(
    id: Option<String>,
    provider: String,
    base_url: String,
    label: String,
    token: String,
) -> Result<RepoCredentialMeta, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let username = repo_client::verify_token(&provider, &base_url, &token)?;
        credentials::upsert_credential(id, &provider, &base_url, &label, &username, &token)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_repo_credential(cred_id: String) -> Result<Vec<RepoCredentialMeta>, String> {
    credentials::delete_credential(&cred_id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_remote_repos(
    provider: String,
    base_url: String,
    credential_id: String,
) -> Result<Vec<RemoteRepoItem>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        repo_client::list_user_repos(&provider, &base_url, &credential_id)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn bind_project_repo(
    state: State<'_, AppState>,
    project_id: String,
    provider: String,
    base_url: String,
    repo_slug: String,
    credential_id: Option<String>,
    clone_url: Option<String>,
    init_git_if_needed: bool,
) -> Result<ProjectConfig, String> {
    let prov = credentials::normalize_provider(&provider).map_err(|e| e.to_string())?;
    let slug = repo_client::validate_repo_slug(&repo_slug).map_err(|e| e.to_string())?;
    let cred_id = credential_id.unwrap_or_default().trim().to_string();
    let project = find_project(&state, &project_id)?;
    ensure_root_exists(&project)?;
    let root = PathBuf::from(&project.root);
    let remote_url = clone_url
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| {
            let host = if base_url.trim().is_empty() {
                match prov.as_str() {
                    "gitee" => "https://gitee.com",
                    "gitlab" => "https://gitlab.com",
                    "gitea" => "https://gitea.com",
                    _ => "https://github.com",
                }
            } else {
                base_url.trim().trim_end_matches('/')
            };
            format!("{}/{}.git", host, slug)
        });
    repo_client::parse_http_url(&remote_url).map_err(|e| e.to_string())?;
    let rel_dir_name = if project.release_dir.trim().is_empty() {
        "release".to_string()
    } else {
        project.release_dir.trim().trim_end_matches(&['/', '\\'][..]).to_string()
    };

    if init_git_if_needed {
        let root_c = root.clone();
        let url_c = remote_url.clone();
        let prov_c = prov.clone();
        let slug_c = slug.clone();
        tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<()> {
            if !root_c.join(".git").exists() {
                let mut cmd = Command::new("git");
                cmd.current_dir(&root_c).args(["init"]);
                #[cfg(windows)]
                cmd.creation_flags(CREATE_NO_WINDOW);
                let out = cmd.output().map_err(|e| anyhow::anyhow!("无法启动 git init：{}", e))?;
                if !out.status.success() {
                    let err_txt = runner::decode_win_bytes(&out.stderr);
                    return Err(anyhow::anyhow!("git init 失败：{}", err_txt.trim()));
                }
            }
            // Ensure release/ is ignored to protect Git history
            let gi = root_c.join(".gitignore");
            let existing = std::fs::read_to_string(&gi).unwrap_or_default();
            if !existing.lines().any(|l| {
                l.trim()
                    .trim_matches('/')
                    .eq_ignore_ascii_case(&rel_dir_name)
            }) {
                let mut next = existing;
                if !next.is_empty() && !next.ends_with('\n') {
                    next.push('\n');
                }
                next.push_str(&format!("\n# ShipForge build artifacts\n{}/\n", rel_dir_name));
                let _ = guardrails::atomic_write_bytes(&gi, next.as_bytes());
            }
            // Bind or update origin remote — preserve existing SSH remote if it already targets the SAME provider & slug!
            let mut chk = Command::new("git");
            chk.current_dir(&root_c).args(["remote", "get-url", "origin"]);
            #[cfg(windows)]
            chk.creation_flags(CREATE_NO_WINDOW);
            let existing_origin = chk
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| runner::decode_win_bytes(&o.stdout).trim().to_string())
                .unwrap_or_default();
            let already_same_repo_ssh = existing_origin.starts_with("git@")
                && detector::parse_repo_binding_from_url(&existing_origin)
                    .map(|b| {
                        b.provider.eq_ignore_ascii_case(&prov_c)
                            && b.repo_slug.eq_ignore_ascii_case(&slug_c)
                    })
                    .unwrap_or(false);
            if !already_same_repo_ssh {
                let mut set = Command::new("git");
                set.current_dir(&root_c);
                if !existing_origin.is_empty() {
                    set.args(["remote", "set-url", "origin", &url_c]);
                } else {
                    set.args(["remote", "add", "origin", &url_c]);
                }
                #[cfg(windows)]
                set.creation_flags(CREATE_NO_WINDOW);
                let out = set
                    .output()
                    .map_err(|e| anyhow::anyhow!("无法执行 git remote 配置：{}", e))?;
                if !out.status.success() {
                    let err_txt = runner::decode_win_bytes(&out.stderr);
                    return Err(anyhow::anyhow!("配置 Git origin 远端失败：{}", err_txt.trim()));
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    }

    let mut cfg = state.config.lock().map_err(|_| "配置锁异常".to_string())?;
    let proj = cfg
        .projects
        .iter_mut()
        .find(|p| p.id == project_id)
        .ok_or_else(|| "未找到项目".to_string())?;
    proj.repo_binding = RepoBinding {
        provider: prov.clone(),
        base_url: base_url.trim().trim_end_matches('/').to_string(),
        repo_slug: slug.clone(),
        credential_id: cred_id,
    };
    if prov == "github" {
        proj.github_repo = slug;
    } else {
        proj.github_repo.clear();
    }
    let updated = proj.clone();
    config::save_config(&cfg).map_err(|e| e.to_string())?;
    Ok(updated)
}

#[tauri::command]
async fn list_releases_multi(
    provider: String,
    base_url: String,
    credential_id: String,
    repo: String,
) -> Result<Vec<GhReleaseItem>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        repo_client::list_releases_multi(&provider, &base_url, &credential_id, &repo)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn publish_release_multi(
    app: AppHandle,
    state: State<'_, AppState>,
    mut req: MultiPublishRequest,
) -> Result<String, String> {
    let project = find_project(&state, &req.project_id)?;
    let other_roots = other_project_roots(&state, &project.id);
    let task_id = format!("pub-{}", chrono::Local::now().timestamp_millis());
    runner::register_task_project(&task_id, &project.id);
    let _lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &task_id)
        .map_err(|e| format!("当前项目正在执行构建或发布任务，请等待完成后再上传：{}", e))?;
    let release_dir = config::resolve_release_dir(&project);
    let release_dir_mark = release_dir.clone();
    let settings = state
        .config
        .lock()
        .map(|c| c.guardrails.clone())
        .unwrap_or_default();
    let pid = project.id.clone();
    let prov = req.provider.clone();
    let pub_files = req.files.clone();
    let tag_ver = req
        .tag
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .to_string();
    // 统一渠道裁决：带预发布后缀的 tag（如 v1.5.0-beta.1）绝不允许按正式版推送
    let ch_decision = version::resolve_release_channel(
        if req.prerelease { "beta" } else { "stable" },
        &tag_ver,
    );
    req.prerelease = ch_decision.prerelease;
    if version::is_bare_default_release_note(&req.notes) {
        req.notes = resolve_release_notes_for_project(&project, &tag_ver, &other_roots);
    } else {
        req.notes = version::sanitize_public_release_notes(&req.notes);
    }
    let effective_notes = req.notes.clone();
    if pub_files.iter().any(|f| f.eq_ignore_ascii_case("latest.json")) {
        sync_latest_json_notes_if_matching(&release_dir, &tag_ver, &effective_notes);
    }
    let started = Instant::now();
    let app_up = app.clone();
    let tid = task_id.clone();

    let url = tauri::async_runtime::spawn_blocking(move || {
        repo_client::publish_release_multi(&release_dir, &req, |pct, msg| {
            emit_progress(&app_up, &tid, "上传发布产物", "同步至仓库", msg, pct, false);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let mut b = storage::load_version_bucket(&pid, &tag_ver);
    b.sealed = true;
    if !version::is_bare_default_release_note(&effective_notes) {
        b.notes_draft = effective_notes;
    }
    let _ = storage::save_version_bucket(&pid, &b);
    let _ = artifacts::mark_artifacts_stage_with_dir(
        &pid,
        Some(&release_dir_mark),
        &pub_files,
        "published",
        Some(&url),
        Some(&prov),
        false,
    );

    let report = health_report::build_ephemeral_report(
        &project,
        "publish",
        "ok",
        started.elapsed().as_millis() as u64,
        format!("已成功发布至 {}：{}", prov, url),
        None,
        &settings,
    );
    let _ = app.emit("health-report-once", &report);
    Ok(url)
}

#[tauri::command]
async fn update_release_multi(
    state: State<'_, AppState>,
    mut req: UpdateReleaseRequest,
) -> Result<GhReleaseItem, String> {
    let sanitized_notes = if req.notes.trim().is_empty() {
        String::new()
    } else {
        version::sanitize_public_release_notes(&req.notes)
    };
    req.notes = sanitized_notes.clone();

    let project_opt = if !req.project_id.trim().is_empty() {
        find_project(&state, &req.project_id).ok()
    } else {
        None
    };

    // Acquire project lock when syncing local/remote latest.json so it never races with an active build/publish
    let _lock_guard = if req.sync_latest_json {
        if let Some(ref proj) = project_opt {
            let tid = format!("sync-rel-{}", chrono::Local::now().timestamp_millis());
            runner::register_task_project(&tid, &proj.id);
            Some(
                state
                    .runner
                    .acquire_project_guard(&proj.id, &tid)
                    .map_err(|e| format!("当前项目正在执行构建或发布任务，请稍后再同步：{}", e))?,
            )
        } else {
            None
        }
    } else {
        None
    };

    let req_for_update = req.clone();
    let mut updated =
        tauri::async_runtime::spawn_blocking(move || repo_client::update_release_multi(&req_for_update))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;

    let tag_ver = updated
        .tag_name
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .to_string();

    if let Some(ref proj) = project_opt {
        if !tag_ver.is_empty() && !sanitized_notes.is_empty() && !version::is_bare_default_release_note(&sanitized_notes) {
            let mut b = storage::load_version_bucket(&proj.id, &tag_ver);
            b.notes_draft = sanitized_notes.clone();
            b.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let _ = storage::save_version_bucket(&proj.id, &b);
        }

        if req.sync_latest_json {
            let release_dir = config::resolve_release_dir(proj);
            match packager::read_manifest_version(&release_dir) {
                Some(local_ver) if local_ver.eq_ignore_ascii_case(&tag_ver) => {
                    sync_latest_json_notes_if_matching(&release_dir, &tag_ver, &sanitized_notes);
                    let mut sync_req = req.clone();
                    if sync_req.release_id.trim().is_empty() && !updated.id.trim().is_empty() {
                        sync_req.release_id = updated.id.clone();
                    }
                    let remote_asset_names: Vec<String> =
                        updated.assets.iter().map(|a| a.name.clone()).collect();
                    let rel_dir_up = release_dir.clone();
                    let uploaded_asset = tauri::async_runtime::spawn_blocking(move || {
                        repo_client::sync_release_manifest_only(
                            &rel_dir_up,
                            &sync_req,
                            &remote_asset_names,
                        )
                    })
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| format!("线上 Release 正文已更新，但覆盖线上 latest.json 失败：{}", e))?;

                    if let Some(new_asset) = uploaded_asset {
                        if !updated.assets.is_empty() {
                            updated
                                .assets
                                .retain(|a| !a.name.eq_ignore_ascii_case("latest.json"));
                        }
                        updated.assets.push(new_asset);
                    }
                    updated.manifest_synced = true;
                    updated.manifest_sync_message = "已同步更新本地与线上 latest.json（未改动安装包）".into();
                    let _ = artifacts::mark_artifacts_stage_with_dir(
                        &proj.id,
                        Some(&release_dir),
                        &["latest.json".into()],
                        "published",
                        Some(&updated.url),
                        Some(&req.provider),
                        false,
                    );
                }
                Some(local_ver) => {
                    updated.manifest_synced = false;
                    updated.manifest_sync_message = format!(
                        "线上 Release 正文已更新；本地 latest.json 版本为 v{}（与目标 {} 不同），未覆盖线上清单",
                        local_ver, updated.tag_name
                    );
                }
                None => {
                    updated.manifest_synced = false;
                    updated.manifest_sync_message =
                        "线上 Release 正文已更新；本地 release 目录未找到 latest.json，未覆盖线上清单".into();
                }
            }
        }
    }

    Ok(updated)
}

#[tauri::command]
async fn sync_changelog_to_online_release(
    state: State<'_, AppState>,
    req: SyncChangelogOnlineRequest,
) -> Result<SyncChangelogOnlineResult, String> {
    let project = find_project(&state, &req.project_id)?;
    ensure_root_exists(&project)?;

    let clean_ver = req
        .version_str
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .to_string();
    if clean_ver.is_empty() {
        return Err("请填写有效的版本号".into());
    }
    let clean_notes = version::sanitize_public_release_notes(&req.body);
    if clean_notes.trim().is_empty() || version::is_bare_default_release_note(&clean_notes) {
        return Err("当前更新内容为空或仅为默认兜底文案，请先补充实质内容后再同步线上".into());
    }

    // Resolve project's bound repo (multi-provider repo_binding preferred, fallback to github_repo)
    let repo_slug = if !project.repo_binding.repo_slug.trim().is_empty() {
        project.repo_binding.repo_slug.trim().to_string()
    } else {
        project.github_repo.trim().to_string()
    };
    if repo_slug.is_empty() {
        return Err("当前项目尚未绑定远程仓库，请先在「产物与仓库」绑定 GitHub / Gitee / GitLab / Gitea 仓库".into());
    }
    let provider = if !project.repo_binding.provider.trim().is_empty() {
        project.repo_binding.provider.trim().to_string()
    } else {
        "github".into()
    };
    let base_url = project.repo_binding.base_url.trim().to_string();
    let credential_id = project.repo_binding.credential_id.trim().to_string();

    let tid = format!("sync-cl-{}", chrono::Local::now().timestamp_millis());
    runner::register_task_project(&tid, &project.id);
    let _lock_guard = state
        .runner
        .acquire_project_guard(&project.id, &tid)
        .map_err(|e| format!("当前项目正在执行构建或发布任务，请等待完成后再同步：{}", e))?;

    let root = Path::new(&project.root);
    let mut changelog_written = false;
    if req.write_changelog {
        let written_rel = version::upsert_changelog_entry_with_summary(
            root,
            &project.changelog_path,
            &clean_ver,
            req.summary.trim(),
            &clean_notes,
        )
        .map_err(|e| format!("写入本地变更记录失败：{}", e))?;
        changelog_written = true;
        if project.changelog_path.trim().is_empty() {
            if let Ok(mut cfg) = state.config.lock() {
                if let Some(p) = cfg.projects.iter_mut().find(|p| p.id == project.id) {
                    p.changelog_path = written_rel;
                    let _ = config::save_config(&cfg);
                }
            }
        }
    }

    // Persist draft bucket & update local latest.json if matching version
    let mut bucket = storage::load_version_bucket(&project.id, &clean_ver);
    bucket.notes_draft = clean_notes.clone();
    bucket.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let _ = storage::save_version_bucket(&project.id, &bucket);

    let release_dir = config::resolve_release_dir(&project);
    sync_latest_json_notes_if_matching(&release_dir, &clean_ver, &clean_notes);

    // Fetch existing online releases to find the matching tag and preserve its title/draft/prerelease flags
    let prov_l = provider.clone();
    let base_l = base_url.clone();
    let cred_l = credential_id.clone();
    let repo_l = repo_slug.clone();
    let releases = tauri::async_runtime::spawn_blocking(move || {
        repo_client::list_releases_multi(&prov_l, &base_l, &cred_l, &repo_l)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("读取线上发行版列表失败：{}", e))?;

    let target_tag_v = format!("v{}", clean_ver);
    let matched = releases
        .into_iter()
        .find(|r| {
            let t = r.tag_name.trim();
            t.eq_ignore_ascii_case(&target_tag_v) || t.eq_ignore_ascii_case(&clean_ver)
        })
        .ok_or_else(|| {
            if changelog_written {
                format!(
                    "已写入本地变更记录并更新本地 latest.json，但线上仓库尚未发布 {}；请前往「产物与仓库」完成首次发版",
                    target_tag_v
                )
            } else {
                format!(
                    "线上仓库尚未找到 {} 的发行版，请先在「产物与仓库」发布该版本",
                    target_tag_v
                )
            }
        })?;

    let update_req = UpdateReleaseRequest {
        project_id: project.id.clone(),
        provider: provider.clone(),
        base_url: base_url.clone(),
        credential_id: credential_id.clone(),
        repo: repo_slug.clone(),
        release_id: matched.id.clone(),
        tag_name: matched.tag_name.clone(),
        title: if matched.name.trim().is_empty() {
            matched.tag_name.clone()
        } else {
            matched.name.clone()
        },
        notes: clean_notes.clone(),
        prerelease: matched.is_prerelease,
        draft: matched.is_draft,
        sync_latest_json: req.sync_latest_json,
    };

    let req_patch = update_req.clone();
    let updated =
        tauri::async_runtime::spawn_blocking(move || repo_client::update_release_multi(&req_patch))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("更新线上 Release 正文失败：{}", e))?;

    let mut manifest_synced = false;
    let mut detail_msg = format!("已同步更新线上 Release {} 正文说明", updated.tag_name);

    if req.sync_latest_json {
        match packager::read_manifest_version(&release_dir) {
            Some(local_ver) if local_ver.eq_ignore_ascii_case(&clean_ver) => {
                let mut remote_assets: Vec<String> =
                    updated.assets.iter().map(|a| a.name.clone()).collect();
                if remote_assets.is_empty() {
                    remote_assets = matched.assets.iter().map(|a| a.name.clone()).collect();
                }
                let mut sync_req = update_req.clone();
                if sync_req.release_id.trim().is_empty() && !updated.id.trim().is_empty() {
                    sync_req.release_id = updated.id.clone();
                }
                let rel_dir_up = release_dir.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    repo_client::sync_release_manifest_only(&rel_dir_up, &sync_req, &remote_assets)
                })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| format!("线上 Release 正文已更新，但覆盖线上 latest.json 失败：{}", e))?;

                manifest_synced = true;
                detail_msg = format!(
                    "已同步更新线上 Release {} 说明并覆盖 latest.json（未改动安装包）",
                    updated.tag_name
                );
                let _ = artifacts::mark_artifacts_stage_with_dir(
                    &project.id,
                    Some(&release_dir),
                    &["latest.json".into()],
                    "published",
                    Some(&updated.url),
                    Some(&provider),
                    false,
                );
            }
            Some(local_ver) => {
                detail_msg = format!(
                    "已更新线上 Release {} 正文；本地 latest.json 为 v{}（与目标 {} 不同），未覆盖线上清单",
                    updated.tag_name, local_ver, updated.tag_name
                );
            }
            None => {
                detail_msg = format!(
                    "已更新线上 Release {} 正文；本地 release 目录未找到 latest.json，未覆盖线上清单",
                    updated.tag_name
                );
            }
        }
    }

    Ok(SyncChangelogOnlineResult {
        tag_name: updated.tag_name,
        url: updated.url,
        changelog_written,
        manifest_synced,
        message: detail_msg,
    })
}

#[tauri::command]
async fn delete_release_multi(
    app: AppHandle,
    state: State<'_, AppState>,
    req: DeleteReleaseRequest,
) -> Result<DeleteReleaseResult, String> {
    let project = find_project(&state, &req.project_id)?;
    let root = PathBuf::from(&project.root);
    let pid = project.id.clone();
    let tag_name = req.tag_name.clone();
    let tag_ver = tag_name.trim().trim_start_matches('v').trim_start_matches('V').to_string();
    let task_id = format!("recall-{}", chrono::Local::now().timestamp_millis());
    let lock_guard = state
        .runner
        .acquire_project_guard(&pid, &task_id)
        .map_err(|e| format!("当前项目正在执行任务，请等待完成后再撤回发布：{}", e))?;
    runner::register_task_project(&task_id, &pid);
    emit_log(
        &app,
        &task_id,
        "system",
        &format!("▶ 撤回线上 Release：{} ({})", tag_name.trim(), req.repo.trim()),
    );

    let res = tauri::async_runtime::spawn_blocking(move || {
        let _lock_guard = lock_guard;
        repo_client::delete_release_multi(&req, Some(&root))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    // Unseal local version bucket and revert published artifacts back to ready
    if !tag_ver.is_empty() {
        let mut b = storage::load_version_bucket(&pid, &tag_ver);
        if b.sealed {
            b.sealed = false;
            let _ = storage::save_version_bucket(&pid, &b);
        }
    }
    let _ = artifacts::reset_published_artifacts_for_tag(&pid, &tag_name);
    emit_log(&app, &task_id, "stdout", &format!("✓ {}", res.summary));
    for w in &res.warnings {
        emit_log(&app, &task_id, "stderr", &format!("⚠ {}", w));
    }
    Ok(res)
}

#[tauri::command]
async fn delete_release_asset_multi(req: DeleteReleaseAssetRequest) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || repo_client::delete_release_asset_multi(&req))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_app_info() -> serde_json::Value {
    serde_json::json!({
        "name": "ShipForge",
        "version": env!("CARGO_PKG_VERSION"),
        "configDir": config::app_data_dir().to_string_lossy().to_string(),
    })
}

fn main() {
    if let Some(cli) = updater_host::parse_updater_host_cli() {
        updater_host::run_updater_host_app(cli);
        return;
    }

    // 异步清理上次自动更新遗留的独立更新弹窗宿主目录
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(2500));
        updater_host::cleanup_updater_host_artifacts();
    });

    let initial_cfg = config::load_config();
    // Backfill workspace owner metadata for existing projects so their isolated workspaces are bound to their roots
    for p in &initial_cfg.projects {
        let r = Path::new(&p.root);
        if r.is_dir() {
            storage::bind_workspace_owner(&p.id, r);
        }
    }
    let runner = Arc::new(TaskRunner::default());
    let server = Arc::new(Mutex::new(LocalFeedServer::default()));
    let runner_for_exit = runner.clone();
    let server_for_exit = server.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .manage(AppState {
            config: Mutex::new(initial_cfg),
            runner,
            server,
            watch_state: Arc::new(Mutex::new(HashMap::new())),
        })
        .manage(updater::UpdaterState::default())
        .on_window_event(move |_window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                runner_for_exit.cancel_all();
                if let Ok(mut srv) = server_for_exit.lock() {
                    srv.stop();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_app_state,
            set_auto_watch,
            set_scan_roots,
            discover_workspace_projects,
            import_projects_batch,
            detect_project_github_repo,
            launch_gh_auth,
            set_active_project,
            pick_folder,
            add_project_by_path,
            rescan_project,
            update_project_config,
            remove_project,
            get_project_overview,
            detect_project_changes,
            get_version_buckets,
            save_version_draft,
            seal_version_bucket,
            get_storage_info,
            migrate_storage_root,
            consolidate_tools_and_caches,
            scan_toolchains,
            repair_toolchain,
            repair_all_essential_toolchains,
            preview_bump,
            apply_version,
            get_changelog,
            add_changelog_entry,
            upsert_changelog_entry,
            resolve_release_notes_detail,
            extract_release_notes,
            draft_changelog_from_git,
            run_project_action,
            run_custom_command,
            run_pipeline,
            deploy_existing_release,
            cancel_task,
            get_active_tasks,
            pack_portable_archive,
            list_project_entries,
            compute_artifact_sha256,
            generate_release_manifest,
            verify_release_checksums,
            archive_release_artifact,
            prune_old_release_artifacts,
            get_local_server_status,
            start_local_server,
            stop_local_server,
            launch_project_exe,
            open_in_explorer,
            open_external_url,
            git_commit_changes,
            git_tag_release,
            git_push_remote,
            git_undo_last_commit,
            git_delete_tag,
            clean_stale_build_lock,
            get_gh_status,
            get_gh_releases,
            publish_to_github,
            set_onboarding_completed,
            update_guardrail_settings,
            get_system_headroom,
            check_crash_recovery,
            get_project_profile_matrix,
            list_framework_presets,
            apply_framework_preset,
            apply_project_profile,
            inspect_project_prerequisites,
            plan_prerequisite_repair,
            execute_prerequisite_repair,
            analyze_project_files,
            save_package_profiles,
            list_classified_artifacts,
            mark_artifact_stage,
            list_repo_credentials,
            verify_and_save_repo_credential,
            delete_repo_credential,
            list_remote_repos,
            bind_project_repo,
            list_releases_multi,
            publish_release_multi,
            update_release_multi,
            sync_changelog_to_online_release,
            delete_release_multi,
            delete_release_asset_multi,
            get_app_info,
            updater::updater_status,
            updater::updater_config_get,
            updater::updater_config_set,
            updater::updater_check,
            updater::updater_download,
            updater::updater_cancel,
            updater::updater_clear_cache,
            updater::updater_auto_update,
            updater::updater_install,
            updater::updater_changelog,
        ])
        .run(tauri::generate_context!())
        .expect("ShipForge 启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_external_url_blocks_shell_injection() {
        assert!(validate_external_url("https://github.com/acme/demo-app/releases/tag/v1.2.0").is_ok());
        assert!(validate_external_url("http://127.0.0.1:18765/latest.json").is_ok());
        assert!(validate_external_url("https://github.com/a/b&calc.exe").is_err());
        assert!(validate_external_url("https://github.com/a/b|whoami").is_err());
        assert!(validate_external_url("file:///C:/Windows/win.ini").is_err());
        assert!(validate_external_url("javascript:alert(1)").is_err());
        // Userinfo loopback spoofing must be blocked!
        assert!(validate_external_url("http://127.0.0.1:@evil.com/phish").is_err());
        assert!(validate_external_url("http://evil.com/test").is_err());
    }

    #[test]
    fn test_rescan_preserves_user_binding_profiles_env_and_custom_actions() {
        let mut existing = ProjectConfig {
            id: "demo".into(),
            name: "Demo".into(),
            root: "D:\\Demo".into(),
            kind: "rust".into(),
            adapter_label: "Rust".into(),
            primary_language: "rust".into(),
            detected_languages: vec!["rust".into()],
            shell_type: "native".into(),
            product_name: "Demo".into(),
            artifact_prefix: "Demo".into(),
            release_dir: "release".into(),
            changelog_path: "CHANGELOG.md".into(),
            github_repo: "acme/demo".into(),
            repo_binding: RepoBinding {
                provider: "gitee".into(),
                base_url: "https://gitee.com".into(),
                repo_slug: "acme/demo-cn".into(),
                credential_id: "cred-gitee-1".into(),
            },
            keep_releases: 5,
            launch_exe: String::new(),
            lock_file: String::new(),
            journal_file: String::new(),
            has_build_ps1: false,
            has_publish_ps1: false,
            engine_src_dir: String::new(),
            engine_exe_path: String::new(),
            version_files: vec![],
            env: HashMap::from([("CUSTOM_FLAG".into(), "1".into())]),
            actions: vec![ActionSpec {
                id: "custom-smoke".into(),
                label: "冒烟".into(),
                category: "custom".into(),
                command: "echo".into(),
                args: vec!["ok".into()],
                cwd: String::new(),
                description: "smoke".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            }],
            package_profiles: vec![PackageProfile {
                id: "gitee-95".into(),
                name: "Gitee 95MB".into(),
                format: "zip".into(),
                name_template: "Demo_{version}.zip".into(),
                sources: vec!["dist".into()],
                excludes: vec![],
                split_size_mb: Some(95),
                include_readme: false,
                readme_content: String::new(),
            }],
        };

        let detected = ProjectConfig {
            id: "auto-id".into(),
            github_repo: String::new(),
            repo_binding: RepoBinding::default(),
            keep_releases: 2,
            env: HashMap::new(),
            actions: vec![],
            package_profiles: vec![],
            ..existing.clone()
        };

        let merged = merge_rescanned_project(&existing, detected);
        assert_eq!(merged.id, "demo");
        assert_eq!(merged.repo_binding.provider, "gitee");
        assert_eq!(merged.repo_binding.repo_slug, "acme/demo-cn");
        assert_eq!(merged.repo_binding.credential_id, "cred-gitee-1");
        assert_eq!(merged.keep_releases, 5);
        assert_eq!(merged.env.get("CUSTOM_FLAG").map(String::as_str), Some("1"));
        assert_eq!(merged.package_profiles.len(), 1);
        assert!(merged.actions.iter().any(|a| a.id == "custom-smoke"));

        // validate_project_config must reject traversal/non-lock/bad regex and clamp keep_releases
        existing.keep_releases = 999;
        assert!(validate_project_config(&mut existing).is_ok());
        assert_eq!(existing.keep_releases, 50);

        let mut bad_rel = existing.clone();
        bad_rel.release_dir = "../../escape".into();
        assert!(validate_project_config(&mut bad_rel).is_err());

        let mut bad_lock = existing.clone();
        bad_lock.lock_file = "C:\\Windows\\win.ini".into();
        assert!(validate_project_config(&mut bad_lock).is_err());

        // Round 2: bad action cwd must be rejected, and config::resolve_release_dir must fall back safely
        let mut bad_cwd = existing.clone();
        bad_cwd.actions[0].cwd = "../../Windows".into();
        assert!(validate_project_config(&mut bad_cwd).is_err());

        let escaped_rel = ProjectConfig {
            release_dir: "../../outside".into(),
            ..existing.clone()
        };
        assert_eq!(
            config::resolve_release_dir(&escaped_rel),
            PathBuf::from(&escaped_rel.root).join("release")
        );
    }

    #[test]
    fn test_preflight_pipeline_fails_fast_before_mutation() {
        let proj = ProjectConfig {
            id: "demo".into(),
            name: "Demo".into(),
            root: ".".into(),
            kind: "custom".into(),
            adapter_label: "Custom".into(),
            primary_language: "custom".into(),
            detected_languages: vec![],
            shell_type: "portable".into(),
            product_name: "Demo".into(),
            artifact_prefix: "Demo".into(),
            release_dir: "release".into(),
            changelog_path: "CHANGELOG.md".into(),
            github_repo: String::new(),
            repo_binding: RepoBinding::default(),
            keep_releases: 2,
            launch_exe: String::new(),
            lock_file: String::new(),
            journal_file: String::new(),
            has_build_ps1: false,
            has_publish_ps1: false,
            engine_src_dir: String::new(),
            engine_exe_path: String::new(),
            version_files: vec![],
            env: HashMap::new(),
            actions: vec![],
            package_profiles: vec![],
        };
        let bad_gh = PipelineRequest {
            project_id: "demo".into(),
            bump_kind: "patch".into(),
            custom_version: None,
            channel: "stable".into(),
            run_tests: false,
            run_build: false,
            build_action_id: String::new(),
            generate_manifest: false,
            upload_github: true,
            github_repo: String::new(),
            release_notes: String::new(),
            draft: false,
            git_mode: "none".into(),
            git_commit_message: String::new(),
            repo_provider: String::new(),
            repo_base_url: String::new(),
            credential_id: String::new(),
        };
        assert!(preflight_pipeline(&proj, &bad_gh).is_err());

        let bad_custom = PipelineRequest {
            bump_kind: "custom".into(),
            custom_version: Some("not-a-semver".into()),
            upload_github: false,
            ..bad_gh.clone()
        };
        assert!(preflight_pipeline(&proj, &bad_custom).is_err());

        // Git auto-commit on a non-git temp dir must fail fast before bumping version!
        let non_git = std::env::temp_dir().join(format!(
            "shipforge_nongit_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(11)
        ));
        std::fs::create_dir_all(&non_git).unwrap();
        let proj_nongit = ProjectConfig {
            root: non_git.to_string_lossy().to_string(),
            ..proj
        };
        let bad_git = PipelineRequest {
            bump_kind: "none".into(),
            upload_github: false,
            git_mode: "commit_tag".into(),
            ..bad_gh
        };
        assert!(preflight_pipeline(&proj_nongit, &bad_git).is_err());
        let _ = std::fs::remove_dir_all(&non_git);
    }

    #[test]
    fn test_e2e_full_developer_workflow() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_e2e_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(9)
        ));
        let tauri_dir = base.join("src-tauri");
        std::fs::create_dir_all(&tauri_dir).unwrap();
        std::fs::write(
            tauri_dir.join("tauri.conf.json"),
            "{\n  \"productName\": \"E2EApp\",\n  \"version\": \"1.2.3\"\n}\n",
        )
        .unwrap();
        std::fs::write(
            tauri_dir.join("Cargo.toml"),
            "[package]\nname = \"e2e-app\"\nversion = \"1.2.3\"\nedition = \"2021\"\n\n[dependencies]\nserde = { version = \"1.0\" }\n",
        )
        .unwrap();
        std::fs::write(base.join("app.bin"), b"binary payload 1234567890").unwrap();
        std::fs::write(base.join("skip.PDB"), b"debug symbols").unwrap();

        let cfg = detector::detect_project(&base).unwrap();
        assert_eq!(cfg.kind, "tauri");
        assert_eq!(cfg.version_files.len(), 2);

        let vs0 = version::read_project_versions(&base, &cfg.version_files);
        assert!(vs0.synced);
        let next = version::bump_semver(&vs0.current, "minor").unwrap();
        let vs1 = version::write_project_versions(&base, &cfg.version_files, &next).unwrap();
        assert_eq!(vs1.current, "1.3.0");

        version::prepend_changelog_entry(&base, "CHANGELOG.md", "1.3.0", "E2E 验证", "- 完成全链路测试")
            .unwrap();
        let notes = version::extract_notes_for_version(&base, "CHANGELOG.md", "1.3.0", "E2EApp");
        assert!(notes.contains("完成全链路测试"));

        let rel = base.join("release");
        let mut saw_100 = false;
        let zip_res = packager::pack_portable_with_progress(
            &base,
            &rel,
            &PackageRequest {
                project_id: cfg.id.clone(),
                format: "zip".into(),
                output_name: "E2EApp_1.3.0_x64-portable".into(),
                sources: vec!["app.bin".into(), "app.bin".into(), "skip.PDB".into()],
                excludes: vec!["*.pdb".into()],
                readme_name: Some("使用说明.txt".into()),
                readme_content: Some("E2E 便携版".into()),
                split_size_mb: None,
                allow_sensitive: false,
            },
            |p, _| {
                if p == 100 {
                    saw_100 = true;
                }
            },
        )
        .unwrap();
        assert!(saw_100);
        assert_eq!(zip_res.file_name, "E2EApp_1.3.0_x64-portable.zip");
        assert_eq!(zip_res.file_count, 2);

        let mf = packager::generate_manifest(
            &rel,
            &ManifestRequest {
                project_id: cfg.id.clone(),
                version: "1.3.0".into(),
                channel: "stable".into(),
                url_mode: "github".into(),
                github_repo: "tester/e2eapp".into(),
                base_url: String::new(),
                local_port: 18765,
                installer_file: None,
                portable_file: Some(zip_res.file_name.clone()),
                extra_files: vec![],
                min_version: String::new(),
                notes,
            },
        )
        .unwrap();
        assert!(mf.latest_json.contains("https://github.com/tester/e2eapp/releases/download/v1.3.0/"));
        assert_eq!(packager::verify_checksums(&rel).unwrap()[0].status, "ok");

        let mut srv = LocalFeedServer::default();
        srv.start(rel.clone(), 19888, true, None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(60));
        {
            use std::io::{Read, Write};
            let mut sock = std::net::TcpStream::connect(("127.0.0.1", 19888)).unwrap();
            sock.write_all(b"GET /latest.json HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                .unwrap();
            let mut body = String::new();
            sock.read_to_string(&mut body).unwrap();
            assert!(body.contains("http://127.0.0.1:19888/E2EApp_1.3.0_x64-portable.zip"));
        }
        srv.stop();

        packager::archive_artifact(&rel, &zip_res.file_name).unwrap();
        std::fs::write(rel.join(&zip_res.file_name), b"second copy").unwrap();
        packager::archive_artifact(&rel, &zip_res.file_name).unwrap();

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_pipeline_post_build_sync_prevents_false_consecutive_warning() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_pipe_sync_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(23)
        ));
        std::fs::create_dir_all(base.join("release")).unwrap();
        std::fs::write(base.join("Cargo.toml"), "[package]\nname=\"p\"\nversion=\"2.0.0\"\n").unwrap();
        let mut cfg = detector::detect_project(&base).unwrap();
        cfg.id = format!("pipe-sync-{}", chrono::Local::now().timestamp_nanos_opt().unwrap_or(29));

        // Step 1: simulate build recording
        record_build_into_version_bucket(&cfg, "2.0.0", &[]);
        assert_eq!(storage::load_version_bucket(&cfg.id, "2.0.0").build_count, 1);

        // Step 2: simulate post-build manifest generation inside pipeline, then sync snapshot
        std::fs::write(base.join("release").join("latest.json"), r#"{"version":"2.0.0"}"#).unwrap();
        sync_version_bucket_snapshot(&cfg, "2.0.0", &[]);
        assert_eq!(storage::load_version_bucket(&cfg.id, "2.0.0").build_count, 1);

        let (report_clean, _) = compute_change_report_for_project(&cfg, None, &[]);
        assert!(
            report_clean.consecutive_warning.is_empty(),
            "Expected no false consecutive warning right after pipeline sync, got: {}",
            report_clean.consecutive_warning
        );

        // Step 3: genuine subsequent artifact/source modification AFTER pipeline MUST trigger warning!
        std::fs::write(base.join("release").join("extra.zip"), b"modified-after").unwrap();
        let (report_dirty, _) = compute_change_report_for_project(&cfg, None, &[]);
        assert!(!report_dirty.consecutive_warning.is_empty());

        let _ = std::fs::remove_dir_all(storage::workspaces_dir().join(&cfg.id));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_should_delegate_bump_blocks_prerelease_versions() {
        // Stable versions with build.ps1 may delegate patch/minor/major cleanly
        assert!(should_delegate_bump_to_script(true, true, "1.46.8", "patch"));
        assert!(should_delegate_bump_to_script(true, true, "1.46.8", "minor"));
        assert!(should_delegate_bump_to_script(true, true, "1.46.8", "major"));
        // Pre-release versions MUST NOT delegate to external scripts (prevents `[int]"7-beta"` crash!)
        assert!(!should_delegate_bump_to_script(true, true, "1.46.7-beta.1", "patch"));
        assert!(!should_delegate_bump_to_script(true, true, "1.46.7-beta.1", "minor"));
        assert!(!should_delegate_bump_to_script(true, true, "1.46.7-rc.2", "major"));
        // Beta/custom/non-build.ps1 never delegate
        assert!(!should_delegate_bump_to_script(true, true, "1.46.8", "beta"));
        assert!(!should_delegate_bump_to_script(true, true, "1.46.8", "custom"));
        assert!(!should_delegate_bump_to_script(false, true, "1.46.8", "patch"));
        assert!(!should_delegate_bump_to_script(true, false, "1.46.8", "patch"));
    }

    #[test]
    fn test_ensure_project_idle_blocks_version_write_while_busy() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_ver_mutex_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(31)
        ));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("Cargo.toml"), "[package]\nname=\"m\"\nversion=\"1.46.9\"\n").unwrap();
        let lock_file = base.join("test.build.lock");

        let mut proj = detector::detect_project(&base).unwrap();
        proj.id = "proj-mutex-1".into();
        proj.lock_file = lock_file.to_string_lossy().to_string();
        let runner = TaskRunner::default();

        // Idle: allowed
        assert!(ensure_project_idle_for_version_write(&runner, &proj).is_ok());

        // Active ShipForge task on this project: blocked
        runner.try_acquire_project(&proj.id, "pipe-999").unwrap();
        let err = ensure_project_idle_for_version_write(&runner, &proj).unwrap_err();
        assert!(err.contains("pipe-999"), "unexpected err: {}", err);
        runner.release_project(&proj.id, "pipe-999");

        // Storage migration lock active: blocked
        runner.try_acquire_project("__storage__", "mig-1").unwrap();
        assert!(ensure_project_idle_for_version_write(&runner, &proj).is_err());
        runner.release_project("__storage__", "mig-1");

        // Live external build lock (current test PID): blocked
        std::fs::write(
            &lock_file,
            format!(r#"{{"pid":{},"startTicks":0,"version":"1.46.9"}}"#, std::process::id()),
        )
        .unwrap();
        let ext_err = ensure_project_idle_for_version_write(&runner, &proj).unwrap_err();
        assert!(ext_err.contains("外部构建进程"), "unexpected ext err: {}", ext_err);

        // Stale/dead external build lock (PID 0): allowed
        std::fs::write(&lock_file, r#"{"pid":0,"startTicks":0,"version":"1.46.9"}"#).unwrap();
        assert!(ensure_project_idle_for_version_write(&runner, &proj).is_ok());

        proj.lock_file.clear();
        assert!(ensure_project_idle_for_version_write(&runner, &proj).is_ok());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_allocate_unique_project_id_handles_case_whitespace_and_workspace_owner() {
        let ts = chrono::Local::now().timestamp_nanos_opt().unwrap_or(88);
        let root_a = std::env::temp_dir().join(format!("sf_alloc_a_{}", ts));
        let root_b = std::env::temp_dir().join(format!("sf_alloc_b_{}", ts));
        std::fs::create_dir_all(&root_a).unwrap();
        std::fs::create_dir_all(&root_b).unwrap();

        let mut p1 = ProjectConfig::default();
        p1.id = "MyApp".into();
        p1.root = root_a.to_string_lossy().to_string();

        // Case-insensitive collision avoided
        let id2 = allocate_unique_project_id(&[p1.clone()], "myapp", &root_b);
        assert_eq!(id2, "myapp-2");

        // Empty/whitespace base_id never produces "-2"
        let mut p_empty = ProjectConfig::default();
        p_empty.id = "project".into();
        let id3 = allocate_unique_project_id(&[p_empty], "   ", &root_b);
        assert_eq!(id3, "project-2");

        // Orphaned workspace owned by root_a forces a new ID for root_b, but allows root_a to recover
        let orphan_id = format!("orphan-alloc-{}", ts);
        storage::bind_workspace_owner(&orphan_id, &root_a);
        assert_eq!(allocate_unique_project_id(&[], &orphan_id, &root_a), orphan_id);
        assert_eq!(
            allocate_unique_project_id(&[], &orphan_id, &root_b),
            format!("{}-2", orphan_id)
        );

        let _ = std::fs::remove_dir_all(storage::project_workspace_dir(&orphan_id));
        let _ = std::fs::remove_dir_all(&root_a);
        let _ = std::fs::remove_dir_all(&root_b);
    }

    #[test]
    fn test_resolve_release_notes_priority_cascade_and_latest_json_sync() {
        let ts = chrono::Local::now().timestamp_nanos_opt().unwrap_or(97);
        let dir = std::env::temp_dir().join(format!("shipforge_notes_cascade_{}", ts));
        let rel = dir.join("release");
        std::fs::create_dir_all(&rel).unwrap();

        let mut proj = ProjectConfig::default();
        proj.id = format!("notes-proj-{}", ts);
        proj.name = "NotesDemo".into();
        proj.product_name = "NotesDemo".into();
        proj.root = dir.to_string_lossy().to_string();
        proj.release_dir = "release".into();
        proj.changelog_path = "CHANGELOG.md".into();

        // Tier 6: empty project -> clean fallback
        let r_fb = resolve_release_notes_detail_for_project(&proj, "1.0.0", &[]);
        assert_eq!(r_fb.source, "fallback");
        assert!(r_fb.is_fallback);

        // Tier 5: workspace bucket changed files -> workspace_diff
        let mut b = storage::load_version_bucket(&proj.id, "1.0.1");
        b.changed_files = vec!["src/main.rs".into(), ".env".into()];
        storage::save_version_bucket(&proj.id, &b).unwrap();
        let r_ws = resolve_release_notes_detail_for_project(&proj, "1.0.1", &[]);
        assert_eq!(r_ws.source, "workspace_diff");
        assert!(!r_ws.is_fallback);
        assert!(r_ws.notes.contains("main.rs"));
        assert!(!r_ws.notes.contains(".env"));

        // Tier 3: Unreleased section overrides workspace diff
        std::fs::write(
            dir.join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n- 新增自动更新日志填写\n\n## 1.0.2\n- 精确版本变更条目\n",
        )
        .unwrap();
        let r_unrel = resolve_release_notes_detail_for_project(&proj, "1.0.1", &[]);
        assert_eq!(r_unrel.source, "unreleased");
        assert!(r_unrel.notes.contains("新增自动更新日志填写"));

        // Tier 2: User-saved draft overrides Unreleased
        b.notes_draft = "- 用户手写草稿内容".into();
        storage::save_version_bucket(&proj.id, &b).unwrap();
        let r_draft = resolve_release_notes_detail_for_project(&proj, "1.0.1", &[]);
        assert_eq!(r_draft.source, "draft");
        assert!(r_draft.notes.contains("用户手写草稿内容"));

        // Tier 1: Exact changelog entry wins for 1.0.2
        let r_exact = resolve_release_notes_detail_for_project(&proj, "1.0.2", &[]);
        assert_eq!(r_exact.source, "changelog");
        assert!(r_exact.notes.contains("精确版本变更条目"));

        // sync_latest_json_notes_if_matching updates matching version ONLY and ignores mismatched version
        std::fs::write(
            rel.join("latest.json"),
            r#"{"version":"1.0.2","notes":"NotesDemo v1.0.2 正式发布。"}"#,
        )
        .unwrap();
        sync_latest_json_notes_if_matching(&rel, "1.0.1", "wrong version notes");
        let after_mismatch = std::fs::read_to_string(rel.join("latest.json")).unwrap();
        assert!(after_mismatch.contains("正式发布。"), "mismatched version must not overwrite latest.json");

        sync_latest_json_notes_if_matching(&rel, "v1.0.2", "- 同步后的完整更新说明");
        let after_match = std::fs::read_to_string(rel.join("latest.json")).unwrap();
        assert!(after_match.contains("同步后的完整更新说明"));

        let _ = std::fs::remove_dir_all(storage::project_workspace_dir(&proj.id));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validate_project_config_blocks_unc_journal_and_engine_traversal() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_val_cfg_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(113)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let make_base = || {
            let mut p = ProjectConfig::default();
            p.id = "demo".into();
            p.name = "Demo".into();
            p.root = dir.to_string_lossy().to_string();
            p
        };

        let mut p_unc = make_base();
        p_unc.journal_file = r"\\evil\share\builds.jsonl".into();
        assert!(validate_project_config(&mut p_unc).is_err(), "UNC journal_file must be blocked");

        let mut p_ext = make_base();
        p_ext.journal_file = r"D:\Builds\not_jsonl.txt".into();
        assert!(validate_project_config(&mut p_ext).is_err(), "non-.jsonl journal_file must be blocked");

        let mut p_eng = make_base();
        p_eng.engine_src_dir = r"..\..\outside".into();
        assert!(validate_project_config(&mut p_eng).is_err(), "engine_src_dir traversal must be blocked");

        let mut p_ok = make_base();
        p_ok.journal_file = r"D:\Builds\demo-builds.jsonl".into();
        p_ok.engine_src_dir = r"src\engine".into();
        p_ok.engine_exe_path = r"引擎\demo.exe".into();
        assert!(validate_project_config(&mut p_ok).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore]
    fn probe_live_real_workspaces_readonly() {
        let settings = GuardrailSettings::default();
        println!("=== REAL TOOLCHAIN SCAN ===");
        for t in toolchain::scan_toolchains() {
            println!(
                "  tool {:16} | status={:10} | ess={} | rep={} | ver={} | path={} | detail={}",
                t.id, t.status, t.essential, t.repairable, t.version, t.path, t.detail
            );
        }

        for dir_str in [r"D:\Tools\TidyAssistant", r"D:\Tools\ShipForge"] {
            let root = Path::new(dir_str);
            if !root.is_dir() {
                continue;
            }
            println!("\n==================================================");
            println!("=== REAL WORKSPACE PROBE: {} ===", dir_str);
            let mut proj = detector::detect_project(root).expect("detect_project");
            if dir_str.ends_with("TidyAssistant") {
                proj.engine_src_dir = r"src\engine".into();
                proj.engine_exe_path = r"引擎\整理助手引擎.exe".into();
            }
            println!(
                "  detected: id={} kind={} adapter={} lang={}/{} has_build_ps1={} launch_exe={} lock={} journal={} cl={} rel_dir={} repo={}/{:?}",
                proj.id, proj.kind, proj.adapter_label, proj.primary_language, proj.shell_type,
                proj.has_build_ps1, proj.launch_exe, proj.lock_file, proj.journal_file,
                proj.changelog_path, proj.release_dir, proj.github_repo, proj.repo_binding
            );
            println!("  version_files ({}): {:?}", proj.version_files.len(), proj.version_files.iter().map(|v| format!("{}[{}]", v.path, v.format)).collect::<Vec<_>>());
            println!("  actions ({}): {:?}", proj.actions.len(), proj.actions.iter().map(|a| format!("{}({})", a.id, a.category)).collect::<Vec<_>>());

            let vs = version::read_project_versions(root, &proj.version_files);
            println!("  versions: current={} synced={} readings={:?}", vs.current, vs.synced, vs.readings.iter().map(|r| format!("{}={}({:?})", r.path, r.version, r.error)).collect::<Vec<_>>());

            let cl = version::read_changelog(root, &proj.changelog_path);
            println!(
                "  changelog: exists={} path={} entries={} first={:?} last={:?}",
                cl.exists,
                cl.path,
                cl.entries.len(),
                cl.entries.first().map(|e| (&e.version, &e.heading)),
                cl.entries.last().map(|e| (&e.version, &e.heading))
            );

            let git = git_ops::inspect_git_for_project(root, &proj.release_dir);
            println!(
                "  git: is_repo={} branch={} sha={} dirty={} ahead={} behind={} remote={} tags={} last_tag={:?}",
                git.is_repo, git.branch, git.head_sha, git.dirty_count, git.ahead_count, git.behind_count, git.remote_url, git.tags.len(), git.last_tag
            );

            let eng = build_guard::check_engine_stale(root, &proj.engine_src_dir, &proj.engine_exe_path);
            println!("  engine_stale: {:?}", eng);

            let rdir = config::resolve_release_dir(&proj);
            let arts = packager::scan_artifacts(&rdir);
            let active_arts: Vec<_> = arts.iter().filter(|a| !a.is_archived).collect();
            println!("  artifacts: total={} active={} latest_5={:?}", arts.len(), active_arts.len(), active_arts.iter().take(5).map(|a| (&a.name, &a.kind, &a.version)).collect::<Vec<_>>());

            let sums = packager::verify_checksums(&rdir);
            println!("  verify_checksums: {:?}", sums.as_ref().map(|v| v.iter().map(|i| (&i.file, &i.status)).collect::<Vec<_>>()));

            let prereq = preflight::inspect_project_prerequisites(&proj, &settings);
            println!("  prereq: ready={} missing_req={} items={}", prereq.ready, prereq.missing_required, prereq.items.len());
            for it in &prereq.items {
                println!("    [{}] {:22} req={} rep={} | {}", it.status, it.id, it.required, it.repairable, it.detail);
            }

            let analysis = analyzer::analyze_project_files_isolated(&proj.id, root, &[]);
            println!(
                "  analyzer: files={} bytes={} truncated={} sensitive={} skipped={:?}",
                analysis.total_files, analysis.total_bytes, analysis.truncated,
                analysis.sensitive_warnings.len(), analysis.skipped_dirs
            );
            for sw in analysis.sensitive_warnings.iter().take(8) {
                println!("    SENSITIVE: {} ({})", sw.rel_path, sw.reason);
            }

            let (cr, _) = build_change_report_from_parts(
                &proj,
                root,
                &vs,
                &git,
                eng.as_ref().map(|e| e.stale).unwrap_or(false),
                &arts,
                None,
                &[],
            );
            let dr = cr.deploy_readiness.as_ref().expect("deploy_readiness must be populated");
            // Verify zero drift between overview snapshot and subsequent detect_project_changes!
            let snap = ProjectWatchSnapshot {
                fingerprint: cr.fingerprint.clone(),
                version: cr.current_version.clone(),
                artifact_fp: cr.artifact_fingerprint.clone(),
                fs_snapshot: None,
            };
            let (cr2, _) = compute_change_report_for_project(&proj, Some(&snap), &[]);
            assert!(!cr2.has_external_change, "back-to-back detect_project_changes must have zero drift!");
            assert_eq!(cr.fingerprint, cr2.fingerprint, "overview & detect fingerprints must match");
            assert_eq!(
                cr.deploy_readiness.as_ref().map(|d| (&d.status, d.newest_artifact_unix)),
                cr2.deploy_readiness.as_ref().map(|d| (&d.status, d.newest_artifact_unix)),
                "overview & detect deploy_readiness must match identically!"
            );
            println!(
                "  deploy_readiness: status={} ver={} ready={} stale={} headline={} | detail={} | artifacts={:?} | pending={:?} | blockers={:?} | zero_drift=true",
                dr.status, dr.version, dr.artifacts_ready, dr.source_newer_than_artifacts,
                dr.headline, dr.detail, dr.artifact_files, dr.pending_actions, dr.blockers
            );

            let n_cur = resolve_release_notes_detail_for_project(&proj, &vs.current, &[]);
            let next_v = version::bump_semver(&vs.current, "patch").unwrap_or_else(|_| "0.0.1".into());
            let n_next = resolve_release_notes_detail_for_project(&proj, &next_v, &[]);
            println!(
                "  notes(cur={}): src={} fallback={} label={} latest_cl={:?} | head={:?}",
                vs.current, n_cur.source, n_cur.is_fallback, n_cur.source_label, n_cur.latest_changelog_version, n_cur.notes.lines().next()
            );
            println!(
                "  notes(next={}): src={} fallback={} label={} latest_cl={:?} | head={:?}",
                next_v, n_next.source, n_next.is_fallback, n_next.source_label, n_next.latest_changelog_version, n_next.notes.lines().next()
            );
            if proj.id == "tidyassistant" {
                let n_1472 = resolve_release_notes_detail_for_project(&proj, "1.47.2", &[]);
                println!(
                    "  notes(1.47.2): src={} fallback={} label={} | head={:?}",
                    n_1472.source, n_1472.is_fallback, n_1472.source_label, n_1472.notes.lines().next()
                );
            }
        }
    }

    #[test]
    fn test_overview_and_detect_zero_drift_and_changelog_range_resolution() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_zero_drift_notes_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(101)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::create_dir_all(dir.join("release")).unwrap();

        let git_run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .expect("git");
            assert!(out.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr));
        };
        git_run(&["init"]);
        git_run(&["config", "user.name", "Test"]);
        git_run(&["config", "user.email", "test@example.com"]);

        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"1.46.16\"\n").unwrap();
        std::fs::write(dir.join("docs/变更记录.md"), "# 变更记录\n\n## 1.46.16\n- 初始版本\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        git_run(&["add", "."]);
        git_run(&["commit", "-m", "initial 1.46.16"]);
        git_run(&["tag", "v1.46.16"]);

        // Now simulate pre-build automated commits for 1.47.0 & 1.47.1 with real changelog notes, then bump to 1.47.2
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"1.47.2\"\n").unwrap();
        std::fs::write(
            dir.join("docs/变更记录.md"),
            "# 变更记录\n\n## 1.46.16\n- 初始版本\n\n## 1.47.0（minor）—— 界面重构\n- 全面升级 Apple 风格界面\n\n## 1.47.1（patch）—— 视觉精修\n- 移除输入框默认蓝框\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() { println!(\"v1.47\"); }\n").unwrap();
        git_run(&["add", "."]);
        git_run(&["commit", "-m", "pre-build 1.47.2 (full)"]);

        let mut proj = ProjectConfig::default();
        proj.id = "demo_zero_drift".into();
        proj.name = "DemoApp".into();
        proj.product_name = "DemoApp".into();
        proj.root = dir.to_string_lossy().to_string();
        proj.changelog_path = "docs/变更记录.md".into();
        proj.release_dir = "release".into();
        proj.version_files = vec![VersionFileSpec {
            path: "Cargo.toml".into(),
            format: "cargo_toml".into(),
            pattern: String::new(),
        }];

        // 1. Overview change report MUST populate deploy_readiness and match detect_project_changes 100% (zero 8s banner loop!)
        let vs = version::read_project_versions(&dir, &proj.version_files);
        let git = git_ops::inspect_git_for_project(&dir, &proj.release_dir);
        let arts = packager::scan_artifacts(&dir.join("release"));
        let (cr_ov, _) = build_change_report_from_parts(&proj, &dir, &vs, &git, false, &arts, None, &[]);
        assert!(cr_ov.deploy_readiness.is_some(), "overview change_report must include deploy_readiness");
        let snap = ProjectWatchSnapshot {
            fingerprint: cr_ov.fingerprint.clone(),
            version: cr_ov.current_version.clone(),
            artifact_fp: cr_ov.artifact_fingerprint.clone(),
            fs_snapshot: None,
        };
        let (cr_det, _) = compute_change_report_for_project(&proj, Some(&snap), &[]);
        assert!(!cr_det.has_external_change, "detect after overview sync must NOT flag external change");
        assert_eq!(cr_ov.fingerprint, cr_det.fingerprint);
        assert_eq!(
            cr_ov.deploy_readiness.as_ref().map(|d| (&d.status, d.newest_artifact_unix)),
            cr_det.deploy_readiness.as_ref().map(|d| (&d.status, d.newest_artifact_unix))
        );

        // 2. Resolving 1.47.2 MUST aggregate unpublished 1.47.1 + 1.47.0 human changelog notes instead of dumping file paths!
        let r1472 = resolve_release_notes_detail_for_project(&proj, "1.47.2", &[]);
        assert_eq!(r1472.source, "changelog_range", "got: {:?}", r1472);
        assert!(!r1472.is_fallback);
        assert!(r1472.notes.contains("全面升级 Apple 风格界面"));
        assert!(r1472.notes.contains("移除输入框默认蓝框"));
        assert!(!r1472.notes.contains("src/main.rs"));

        // 3. Tag v1.47.2, bump locally to 1.47.3 with ONLY a bare stub in 变更记录.md -> MUST reject stub & NOT dump `- docs/变更记录.md`!
        git_run(&["tag", "v1.47.2"]);
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"1.47.3\"\n").unwrap();
        std::fs::write(
            dir.join("docs/变更记录.md"),
            "# 变更记录\n\n## 1.46.16\n- 初始版本\n\n## 1.47.0（minor）—— 界面重构\n- 全面升级 Apple 风格界面\n\n## 1.47.1（patch）—— 视觉精修\n- 移除输入框默认蓝框\n\n## 1.47.3 (2026-11-26)\n\nDemoApp v1.47.3 正式发布。\n",
        )
        .unwrap();
        let r1473 = resolve_release_notes_detail_for_project(&proj, "1.47.3", &[]);
        assert_eq!(r1473.source, "changelog_carryover", "no-change rebuild bump must carry over latest real entry, not placeholder/paths: {:?}", r1473);
        assert!(!r1473.is_fallback);
        assert!(r1473.notes.contains("移除输入框默认蓝框"));
        assert!(!r1473.notes.contains("正式发布"));
        assert!(!r1473.notes.starts_with('#'));
        assert_eq!(r1473.latest_changelog_version.as_deref(), Some("1.47.1"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
