use crate::artifacts;
use crate::config;
use crate::credentials;
use crate::guardrails;
use crate::models::{
    ArtifactInfo, ChangeReport, DeployPushRequest, DeployReadiness, GitStatus, ProjectConfig,
    VersionState,
};
use crate::packager;
use crate::storage;
use anyhow::{anyhow, Result};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::UNIX_EPOCH;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn run_git_quick(root: &Path, args: &[&str]) -> Option<String> {
    if !root.join(".git").exists() {
        return None;
    }
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd.output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn file_mtime_unix(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn is_binary_deploy_artifact(a: &ArtifactInfo) -> bool {
    !a.is_archived && matches!(a.kind.as_str(), "installer" | "portable")
}

fn manifest_referenced_files_for_version(release_dir: &Path, expected_ver: &str) -> (bool, bool, Vec<String>) {
    let mf_path = release_dir.join("latest.json");
    if !mf_path.is_file() {
        return (false, false, vec![]);
    }
    let Ok(raw) = fs::read_to_string(&mf_path) else {
        return (true, false, vec![]);
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return (true, false, vec![]);
    };
    let mf_ver = val
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
    if exp_clean.is_empty() || !mf_ver.eq_ignore_ascii_case(exp_clean) {
        return (true, false, vec![]);
    }

    let mut files: Vec<String> = Vec::new();
    let mut all_valid = true;
    let mut checked_any = false;

    for key in ["installer", "portable"] {
        if let Some(obj) = val.get(key).and_then(|v| v.as_object()) {
            let fname = obj.get("file").and_then(|v| v.as_str()).unwrap_or("").trim();
            if fname.is_empty() {
                continue;
            }
            checked_any = true;
            let sha = obj.get("sha256").and_then(|v| v.as_str()).unwrap_or("").trim();
            let sha_ok = sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit());
            let expected_size = obj.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            let p_res = guardrails::resolve_confined_path(release_dir, fname);
            match p_res {
                Ok(p) if p.is_file() => {
                    let actual_size = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                    if !sha_ok || expected_size == 0 || actual_size != expected_size {
                        all_valid = false;
                    }
                    if !files.iter().any(|x| x.eq_ignore_ascii_case(fname)) {
                        files.push(fname.to_string());
                    }
                }
                _ => {
                    all_valid = false;
                }
            }
        }
    }

    // Also inspect `extras` array if present (multi-framework extra archives / wheels / jars)
    if let Some(arr) = val.get("extras").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(fname) = item.get("file").and_then(|v| v.as_str()).map(str::trim) {
                if fname.is_empty() {
                    continue;
                }
                checked_any = true;
                let sha = item.get("sha256").and_then(|v| v.as_str()).unwrap_or("").trim();
                let sha_ok = sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit());
                let expected_size = item.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                match guardrails::resolve_confined_path(release_dir, fname) {
                    Ok(p) if p.is_file() => {
                        let actual_size = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                        if !sha_ok || expected_size == 0 || actual_size != expected_size {
                            all_valid = false;
                        }
                        if !files.iter().any(|x| x.eq_ignore_ascii_case(fname)) {
                            files.push(fname.to_string());
                        }
                    }
                    _ => all_valid = false,
                }
            }
        }
    }

    (true, checked_any && all_valid, files)
}

fn check_checksums_cover_files(release_dir: &Path, target_files: &[String]) -> (bool, bool) {
    let sums_path = release_dir.join("SHA256SUMS.txt");
    if !sums_path.is_file() {
        return (false, false);
    }
    if target_files.is_empty() {
        return (true, false);
    }
    let Ok(raw) = fs::read_to_string(&sums_path) else {
        return (true, false);
    };
    let mut listed: HashSet<String> = HashSet::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.len() < 66 {
            continue;
        }
        let mut parts = t.splitn(2, char::is_whitespace);
        let hash = parts.next().unwrap_or("");
        let rest = parts
            .next()
            .unwrap_or("")
            .trim()
            .trim_start_matches('*')
            .trim();
        if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) && !rest.is_empty() {
            listed.insert(rest.to_ascii_lowercase());
        }
    }
    let all_covered = target_files
        .iter()
        .all(|f| listed.contains(&f.to_ascii_lowercase()));
    (true, all_covered)
}

/// Discover active deployable binary artifacts belonging to `target_version` across ANY framework:
/// 1) Version-matched installer/portable/package files (`a.version == target_version`),
/// 2) Unversioned active binaries explicitly referenced by a version-aligned `latest.json`
///    (supports Go/C++/Python/Node/generic tools whose output filenames don't embed SemVer),
/// 3) Strictly excludes artifacts belonging to any OTHER version!
pub fn discover_version_deploy_artifacts(
    release_dir: &Path,
    artifacts: &[ArtifactInfo],
    target_version: &str,
) -> Vec<ArtifactInfo> {
    let clean_ver = target_version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    let (_, mf_aligned, mf_files) = manifest_referenced_files_for_version(release_dir, clean_ver);
    let mf_set: HashSet<String> = mf_files.iter().map(|s| s.to_ascii_lowercase()).collect();

    let mut out: Vec<ArtifactInfo> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for a in artifacts {
        if !is_binary_deploy_artifact(a) || a.size == 0 {
            continue;
        }
        let matches_ver = a
            .version
            .as_deref()
            .map(|v| v.eq_ignore_ascii_case(clean_ver))
            .unwrap_or(false);
        let referenced_unversioned = a.version.is_none()
            && mf_aligned
            && mf_set.contains(&a.name.to_ascii_lowercase());

        if (matches_ver || referenced_unversioned) && seen.insert(a.name.to_ascii_lowercase()) {
            out.push(a.clone());
        }
    }

    // Fallback ONLY for unversioned generic projects where version is "0.0.0" or empty
    if out.is_empty() && (clean_ver.is_empty() || clean_ver == "0.0.0") {
        for a in artifacts {
            if is_binary_deploy_artifact(a)
                && a.size > 0
                && a.version.is_none()
                && seen.insert(a.name.to_ascii_lowercase())
            {
                out.push(a.clone());
            }
        }
    }

    out
}

fn git_head_changed_substantive_source(
    root: &Path,
    project: &ProjectConfig,
    newest_artifact_unix: u64,
) -> Option<String> {
    let ts_raw = run_git_quick(root, &["log", "-1", "--format=%ct", "HEAD"])?;
    let commit_ts = ts_raw.trim().parse::<u64>().ok()?;
    let subject = run_git_quick(root, &["log", "-1", "--format=%s", "HEAD"]).unwrap_or_default();
    let low_subj = subject.trim().to_ascii_lowercase();
    // Automated release/pre-build commits are created by build scripts right before/during packaging
    if low_subj.starts_with("pre-build ") || low_subj.starts_with("chore(release):") {
        return None;
    }
    if commit_ts <= newest_artifact_unix.saturating_add(5) {
        return None;
    }
    let files_raw = run_git_quick(
        root,
        &[
            "-c",
            "core.quotepath=false",
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "HEAD",
        ],
    )?;
    for f in files_raw.lines().map(str::trim).filter(|s| !s.is_empty()) {
        let cat = storage::classify_change_path(f, project);
        if matches!(cat, "source" | "engine") {
            // Confirm disk mtime of the touched source file is actually newer than the built artifact!
            // Prevents false-stale when user built code first and ran `git commit` 1 minute later without editing!
            let f_mtime = file_mtime_unix(&root.join(f));
            if f_mtime > newest_artifact_unix.saturating_add(2) {
                return Some(f.to_string());
            }
        }
    }
    None
}

/// Check release tag alignment against `HEAD`:
/// returns `(tag_exists_on_head, tag_exists_on_other_commit)`
fn inspect_release_tag_alignment(root: &Path, git: &GitStatus, clean_ver: &str) -> (bool, bool) {
    if !git.is_repo || clean_ver.is_empty() || clean_ver == "0.0.0" {
        return (false, false);
    }
    let matched_tag = git.tags.iter().find(|t| {
        let low = t.trim().to_ascii_lowercase();
        !low.starts_with("build-")
            && t.trim()
                .trim_start_matches('v')
                .trim_start_matches('V')
                .eq_ignore_ascii_case(clean_ver)
    });
    let Some(tag_name) = matched_tag else {
        return (false, false);
    };
    let head_sha = run_git_quick(root, &["rev-parse", "HEAD"]).unwrap_or_default();
    let tag_sha = run_git_quick(
        root,
        &["rev-parse", "--verify", "--quiet", &format!("refs/tags/{}^{{commit}}", tag_name)],
    )
    .unwrap_or_default();
    if !head_sha.is_empty() && !tag_sha.is_empty() {
        if head_sha == tag_sha {
            (true, false)
        } else {
            (false, true)
        }
    } else {
        (true, false)
    }
}

/// Framework-agnostic readiness inspector that accurately distinguishes:
/// - `ready_to_deploy`: Fresh built & packaged artifacts exist for current version, awaiting tag/push/release upload.
/// - `needs_build`: No current-version artifacts, or source/engine changed after artifacts were built.
/// - `partial_artifacts`: Current-version artifacts exist but version files are unsynced or blocked.
/// - `published`: Current version is sealed, published, and Git remote is synced.
pub fn inspect_deploy_readiness(
    project: &ProjectConfig,
    versions: &VersionState,
    git: &GitStatus,
    artifacts_list: &[ArtifactInfo],
    change_report: &ChangeReport,
    engine_stale: bool,
) -> DeployReadiness {
    let root = Path::new(&project.root);
    let release_dir = config::resolve_release_dir(project);
    let ver = versions.current.trim().to_string();

    let matching = discover_version_deploy_artifacts(&release_dir, artifacts_list, &ver);
    let artifact_files: Vec<String> = matching.iter().map(|a| a.name.clone()).collect();
    let artifact_total_bytes: u64 = matching.iter().map(|a| a.size).sum();
    let newest_artifact_unix = matching
        .iter()
        .map(|a| file_mtime_unix(Path::new(&a.path)))
        .max()
        .unwrap_or(0);
    let artifacts_ready = !matching.is_empty();

    let (manifest_exists, manifest_aligned, _) =
        manifest_referenced_files_for_version(&release_dir, &ver);
    let (checksums_exists, checksums_aligned) =
        check_checksums_cover_files(&release_dir, &artifact_files);

    // Check whether source/engine/config changed AFTER artifacts were built (docs-only changes do NOT stale binaries!)
    let mut source_newer = false;
    let mut stale_reason = String::new();

    if engine_stale {
        source_newer = true;
        stale_reason = "子引擎源码比已打包引擎更新，需先重新构建".into();
    } else {
        let substantive_dirty: Vec<&str> = change_report
            .classified_changes
            .iter()
            .filter(|c| matches!(c.category.as_str(), "source" | "engine" | "config"))
            .map(|c| c.path.as_str())
            .collect();
        if !substantive_dirty.is_empty() {
            source_newer = true;
            let sample = substantive_dirty
                .iter()
                .take(2)
                .copied()
                .collect::<Vec<_>>()
                .join("、");
            stale_reason = format!(
                "工作区存在 {} 项未编译源码/配置改动（{}），需先重新构建",
                substantive_dirty.len(),
                sample
            );
        } else if artifacts_ready && git.is_repo && newest_artifact_unix > 0 {
            if let Some(sample_file) =
                git_head_changed_substantive_source(root, project, newest_artifact_unix)
            {
                source_newer = true;
                stale_reason = format!(
                    "最新提交修改了源码（{}）且文件时间晚于当前产物，需重新构建",
                    sample_file
                );
            }
        }
    }

    let clean_ver = ver.trim_start_matches('v').trim_start_matches('V');
    let (git_has_release_tag, git_tag_misaligned) =
        inspect_release_tag_alignment(root, git, clean_ver);

    let has_git_remote = git.is_repo && !git.remote_url.trim().is_empty();
    let git_needs_push = has_git_remote
        && (git.ahead_count > 0 || !git_has_release_tag || change_report.dirty_count > 0);

    let repo_slug = if !project.repo_binding.repo_slug.trim().is_empty() {
        project.repo_binding.repo_slug.trim()
    } else {
        project.github_repo.trim()
    };
    let repo_bound = repo_slug.contains('/');

    // Check actual per-artifact published stage (automatically demoted by hash mismatch if rebuilt externally!)
    let classified = artifacts::classify_artifacts(&project.id, &release_dir, clean_ver);
    let all_matching_published = artifacts_ready
        && matching.iter().all(|a| {
            classified
                .iter()
                .find(|c| c.artifact.name.eq_ignore_ascii_case(&a.name))
                .map(|c| c.stage == "published")
                .unwrap_or(false)
        });
    let already_published = artifacts_ready
        && change_report.bucket.sealed
        && all_matching_published
        && !source_newer
        && !git_tag_misaligned
        && (!has_git_remote || (git.ahead_count == 0 && git_has_release_tag && change_report.dirty_count == 0));

    let mut blockers: Vec<String> = Vec::new();
    if !versions.readings.is_empty() && !versions.synced {
        blockers.push("多个版本配置文件尚未对齐".into());
    }
    if git_tag_misaligned {
        blockers.push(format!(
            "标签 v{} 已存在但指向旧提交（与当前 HEAD 不一致），请递增版本号或先删除旧标签",
            clean_ver
        ));
    }
    if source_newer && !stale_reason.is_empty() {
        blockers.push(stale_reason.clone());
    }
    if !artifacts_ready {
        blockers.push(format!("release/ 中尚未找到 v{} 的可分发产物", clean_ver));
    }
    if artifacts_ready && !repo_bound && !has_git_remote {
        blockers.push("尚未绑定目标发布仓库或 Git 远端（前往「产物与仓库」绑定后即可一键部署推送）".into());
    }

    let mut pending_actions: Vec<String> = Vec::new();
    if artifacts_ready && !source_newer {
        if !manifest_aligned || !checksums_aligned {
            pending_actions.push("补齐并校验 latest.json / SHA256SUMS.txt".into());
        }
        if git.is_repo {
            if change_report.dirty_count > 0 {
                pending_actions.push(format!("提交 {} 项变更记录/文档", change_report.dirty_count));
            }
            if !git_has_release_tag && !clean_ver.is_empty() && clean_ver != "0.0.0" {
                pending_actions.push(format!("创建发布标签 v{}", clean_ver));
            }
            if has_git_remote && (git.ahead_count > 0 || !git_has_release_tag || change_report.dirty_count > 0) {
                if git.ahead_count > 0 {
                    pending_actions.push(format!("推送 {} 个提交与标签至远端", git.ahead_count));
                } else {
                    pending_actions.push("推送标签至远端仓库".into());
                }
            }
        }
        if repo_bound && !all_matching_published {
            let prov_label = match project.repo_binding.provider.as_str() {
                "gitee" => "Gitee",
                "gitlab" => "GitLab",
                "gitea" => "Gitea",
                _ => "GitHub",
            };
            pending_actions.push(format!(
                "上传 {} 个产物至 {} Release",
                artifact_files.len(),
                prov_label
            ));
        }
    }

    let (status, headline, detail) = if already_published {
        (
            "published".to_string(),
            format!("v{} 已完成发布与远端同步", clean_ver),
            format!(
                "已发布 {} 个成品，源码与远端状态一致",
                artifact_files.len()
            ),
        )
    } else if artifacts_ready && blockers.is_empty() && (!pending_actions.is_empty() || !all_matching_published) {
        let pending_txt = if pending_actions.is_empty() {
            "可直接执行部署推送或绑定仓库后上传".to_string()
        } else {
            format!("待完成：{}", pending_actions.join(" → "))
        };
        (
            "ready_to_deploy".to_string(),
            format!(
                "v{} 已完成构建打包（{} 个成品就绪），仅差部署推送",
                clean_ver,
                artifact_files.len()
            ),
            pending_txt,
        )
    } else if artifacts_ready && !blockers.is_empty() {
        (
            "partial_artifacts".to_string(),
            format!("检测到 v{} 已有本地产物，但存在发布阻断项", clean_ver),
            blockers.join("；"),
        )
    } else {
        (
            "needs_build".to_string(),
            if source_newer {
                format!("v{} 存在未构建改动，需先编译打包", clean_ver)
            } else {
                format!("v{} 尚未生成本地发布产物", clean_ver)
            },
            if !stale_reason.is_empty() {
                stale_reason.clone()
            } else {
                "点击「开始构建并发布」或通过常用工序生成产物".into()
            },
        )
    };

    DeployReadiness {
        status,
        version: ver,
        headline,
        detail,
        artifacts_ready,
        artifact_files,
        artifact_total_bytes,
        newest_artifact_unix,
        manifest_exists,
        manifest_aligned,
        checksums_exists,
        checksums_aligned,
        source_newer_than_artifacts: source_newer,
        stale_reason,
        git_is_repo: git.is_repo,
        git_dirty_count: change_report.dirty_count,
        git_ahead_count: git.ahead_count,
        git_has_release_tag,
        git_needs_push,
        repo_bound,
        already_published,
        pending_actions,
        blockers,
    }
}

/// Resolve and validate explicit or auto-discovered artifact files confined inside `release_dir`.
pub fn select_deploy_artifacts(
    release_dir: &Path,
    artifacts_list: &[ArtifactInfo],
    target_version: &str,
    explicit_files: &[String],
) -> Result<Vec<ArtifactInfo>> {
    if explicit_files.is_empty() {
        let discovered = discover_version_deploy_artifacts(release_dir, artifacts_list, target_version);
        if discovered.is_empty() {
            return Err(anyhow!(
                "在 release/ 中未找到 v{} 的可部署成品，请先构建打包或核对版本号",
                target_version.trim()
            ));
        }
        return Ok(discovered);
    }

    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for raw in explicit_files {
        let p = guardrails::resolve_confined_path(release_dir, raw)
            .map_err(|e| anyhow!("产物路径非法或越界：{} ({})", raw, e))?;
        if !p.is_file() {
            return Err(anyhow!("指定的部署产物不存在：{}", raw));
        }
        let fname = p
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("无效文件名：{}", raw))?;
        if let Some(info) = artifacts_list
            .iter()
            .find(|a| !a.is_archived && a.name.eq_ignore_ascii_case(fname))
        {
            if !is_binary_deploy_artifact(info) {
                continue;
            }
            if info.size == 0 {
                return Err(anyhow!("产物文件体积为 0 字节，已拦截发布：{}", info.name));
            }
            if seen.insert(info.name.to_ascii_lowercase()) {
                out.push(info.clone());
            }
        } else {
            return Err(anyhow!("指定文件不在活跃产物目录中：{}", raw));
        }
    }
    if out.is_empty() {
        return Err(anyhow!("请至少选择一个有效的可分发产物文件"));
    }
    Ok(out)
}

/// Strict fail-fast preflight BEFORE `deploy_existing_release` mutates manifests, Git, or remote releases.
pub fn preflight_deploy_push(
    project: &ProjectConfig,
    req: &DeployPushRequest,
    readiness: &DeployReadiness,
    versions: &VersionState,
    git: &GitStatus,
    release_dir: &Path,
    selected: &[ArtifactInfo],
) -> Result<()> {
    if !release_dir.is_dir() {
        return Err(anyhow!("找不到产物目录：{}", release_dir.display()));
    }
    let clean_req_ver = req
        .version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    if (req.upload_release || req.git_mode.trim() != "none")
        && (clean_req_ver.is_empty() || clean_req_ver == "0.0.0")
    {
        return Err(anyhow!(
            "当前项目尚未配置有效语义版本号（检测到 v0.0.0），请先在「版本与源码」配置版本文件后再执行远端推送或发布"
        ));
    }
    if !clean_req_ver.is_empty() && clean_req_ver != "0.0.0" {
        crate::version::validate_semver(clean_req_ver)?;
    }

    let (_, tag_misaligned) = inspect_release_tag_alignment(Path::new(&project.root), git, clean_req_ver);
    if tag_misaligned && (req.git_mode.trim() != "none" || req.upload_release) {
        return Err(anyhow!(
            "本地标签 v{} 已存在但指向旧提交（与当前 HEAD 不一致），已拦截推送以防错发；请递增版本号或在「版本与源码」删除旧标签",
            clean_req_ver
        ));
    }

    if !versions.readings.is_empty() && !versions.synced {
        return Err(anyhow!(
            "当前项目存在多个未对齐的版本配置文件（当前检测为 v{}），请先在「版本与源码」对齐版本后再部署",
            versions.current
        ));
    }

    if !versions.current.is_empty()
        && versions.current != "0.0.0"
        && !clean_req_ver.is_empty()
        && !clean_req_ver.eq_ignore_ascii_case(&versions.current)
    {
        return Err(anyhow!(
            "待部署版本 v{} 与当前项目配置版本 v{} 不一致，已拦截以防错发版本",
            clean_req_ver,
            versions.current
        ));
    }

    if selected.is_empty() {
        return Err(anyhow!("未检测到可部署的当前版本成品"));
    }

    // Check every selected artifact belongs to target version (if it encodes a version)
    for a in selected {
        if let Some(ref av) = a.version {
            if !clean_req_ver.is_empty() && !av.eq_ignore_ascii_case(clean_req_ver) {
                return Err(anyhow!(
                    "产物 {} 属于 v{}，与目标发布版本 v{} 不一致，已拦截发布",
                    a.name,
                    av,
                    clean_req_ver
                ));
            }
        }
    }

    if readiness.source_newer_than_artifacts && !req.force_stale_source {
        return Err(anyhow!(
            "源码或子引擎在打包后有新改动（{}），请先重新构建打包；如确认仍要发布现有成品请显式确认覆盖",
            readiness.stale_reason
        ));
    }
    if readiness.source_newer_than_artifacts
        && req.force_stale_source
        && req.git_mode.trim() == "commit_tag_push"
    {
        return Err(anyhow!(
            "已勾选忽略源码较新强推，禁止同时将未编译的新源码提交并打上发布标签；请将 Git 动作改为「不操作 Git」或先重新构建"
        ));
    }

    let git_mode = req.git_mode.trim();
    if !matches!(git_mode, "none" | "tag_push" | "commit_tag_push") {
        return Err(anyhow!("不支持的 Git 部署模式：{}", git_mode));
    }
    if git_mode != "none" {
        if !git.is_repo {
            return Err(anyhow!("当前项目未初始化 Git 仓库，请选择「仅上传发布（不操作 Git）」或先绑定仓库"));
        }
        if git.remote_url.trim().is_empty() {
            return Err(anyhow!("当前 Git 仓库尚未配置远端地址 (origin)，请先在「产物与仓库」一键绑定"));
        }
        if git_mode == "tag_push" && readiness.git_dirty_count > 0 && readiness.source_newer_than_artifacts {
            return Err(anyhow!(
                "工作区存在未提交源码改动，请先构建并提交，或选择「提交改动 + 打标推送」"
            ));
        }
    }

    if !req.upload_release && git_mode == "none" && !req.ensure_manifest {
        return Err(anyhow!("未启用任何部署动作（请至少启用清单校验、Git 推送或线上 Release 上传之一）"));
    }

    if req.upload_release {
        let repo = if !project.repo_binding.repo_slug.trim().is_empty() {
            project.repo_binding.repo_slug.trim()
        } else {
            project.github_repo.trim()
        };
        if !repo.contains('/') {
            return Err(anyhow!("尚未绑定目标发布仓库 (owner/repo)，请先在「产物与仓库」完成绑定"));
        }
        let prov = if !project.repo_binding.provider.trim().is_empty() {
            project.repo_binding.provider.as_str()
        } else {
            "github"
        };
        credentials::resolve_token(
            prov,
            &project.repo_binding.base_url,
            &project.repo_binding.credential_id,
        )
        .map_err(|e| anyhow!("发布前预检失败（远端凭据不可用）：{}", e))?;
    }

    // Verify existing checksums/manifest won't carry corrupted data if ensure_manifest is false
    if !req.ensure_manifest && req.upload_release && release_dir.join("latest.json").is_file() {
        let mut check_names: Vec<String> = selected.iter().map(|a| a.name.clone()).collect();
        check_names.push("latest.json".into());
        packager::validate_manifest_for_publish(release_dir, &format!("v{}", clean_req_ver), &check_names)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ClassifiedChangeItem, RepoBinding, VersionBucket, VersionReading};
    use std::collections::HashMap;

    fn make_proj(root: &Path, kind: &str) -> ProjectConfig {
        let mut p = ProjectConfig::default();
        p.id = format!("dep-test-{}", chrono::Local::now().timestamp_nanos_opt().unwrap_or(1));
        p.name = "DemoApp".into();
        p.product_name = "DemoApp".into();
        p.root = root.to_string_lossy().to_string();
        p.kind = kind.into();
        p.release_dir = "release".into();
        p.repo_binding = RepoBinding {
            provider: "github".into(),
            base_url: String::new(),
            repo_slug: "owner/demo".into(),
            credential_id: String::new(),
        };
        p
    }

    fn make_versions(ver: &str, synced: bool) -> VersionState {
        VersionState {
            current: ver.into(),
            synced,
            readings: vec![VersionReading {
                path: "package.json".into(),
                format: "package_json".into(),
                version: ver.into(),
                exists: true,
                error: None,
            }],
        }
    }

    fn make_clean_report(pid: &str, ver: &str) -> ChangeReport {
        ChangeReport {
            project_id: pid.into(),
            project_name: "DemoApp".into(),
            scan_mode: "git".into(),
            current_version: ver.into(),
            head_sha: "abc1234".into(),
            dirty_count: 0,
            dirty_files: vec![],
            classified_changes: vec![],
            category_counts: HashMap::new(),
            change_summary: String::new(),
            artifact_changed: false,
            source_fingerprint: "fp1".into(),
            artifact_fingerprint: "afp1".into(),
            engine_stale: false,
            artifact_count: 2,
            fingerprint: "fp".into(),
            version_changed: false,
            has_external_change: false,
            changed_since_last_build: false,
            consecutive_warning: String::new(),
            bucket: VersionBucket {
                version: ver.into(),
                ..Default::default()
            },
            deploy_readiness: None,
        }
    }

    #[test]
    fn test_detects_ready_to_deploy_after_external_cli_build_even_when_git_clean() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_dep_ready_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(101)
        ));
        let rel = dir.join("release");
        fs::create_dir_all(&rel).unwrap();

        // Simulate external build.ps1 produced setup + portable + aligned latest.json & SHA256SUMS
        let setup_bytes = b"fake-setup-installer-binary";
        let port_bytes = b"fake-portable-zip-binary";
        fs::write(rel.join("QDisk_1.47.1_x64-setup.exe"), setup_bytes).unwrap();
        fs::write(rel.join("QDisk_1.47.1_x64-portable.zip"), port_bytes).unwrap();
        let sha_s = packager::sha256_file(&rel.join("QDisk_1.47.1_x64-setup.exe")).unwrap();
        let sha_p = packager::sha256_file(&rel.join("QDisk_1.47.1_x64-portable.zip")).unwrap();
        fs::write(
            rel.join("SHA256SUMS.txt"),
            format!("{}  QDisk_1.47.1_x64-setup.exe\n{}  QDisk_1.47.1_x64-portable.zip\n", sha_s, sha_p),
        )
        .unwrap();
        fs::write(
            rel.join("latest.json"),
            format!(
                r#"{{"version":"1.47.1","installer":{{"file":"QDisk_1.47.1_x64-setup.exe","size":{},"sha256":"{}"}},"portable":{{"file":"QDisk_1.47.1_x64-portable.zip","size":{},"sha256":"{}"}}}}"#,
                setup_bytes.len(),
                sha_s,
                port_bytes.len(),
                sha_p
            ),
        )
        .unwrap();

        let proj = make_proj(&dir, "tauri");
        let versions = make_versions("1.47.1", true);
        let git = GitStatus {
            is_repo: true,
            branch: "main".into(),
            head_sha: "e4cb247".into(),
            remote_url: "https://github.com/owner/demo.git".into(),
            dirty_count: 0,
            ahead_count: 2,
            behind_count: 0,
            has_upstream: true,
            changes: vec![],
            recent_commits: vec![],
            tags: vec!["build-1.47.1".into(), "v1.46.16".into()],
            last_tag: Some("v1.46.16".into()),
        };
        let arts = packager::scan_artifacts(&rel);
        let rep = make_clean_report(&proj.id, "1.47.1");

        let r = inspect_deploy_readiness(&proj, &versions, &git, &arts, &rep, false);
        assert_eq!(r.status, "ready_to_deploy", "got: {:?}", r);
        assert!(r.artifacts_ready);
        assert_eq!(r.artifact_files.len(), 2);
        assert!(r.manifest_aligned);
        assert!(r.checksums_aligned);
        assert!(!r.git_has_release_tag, "build-1.47.1 internal tag must NOT count as release tag v1.47.1");
        assert!(r.git_needs_push);
        assert!(r.pending_actions.iter().any(|s| s.contains("v1.47.1")));
        assert!(r.pending_actions.iter().any(|s| s.contains("2 个提交")));

        // Docs-only dirty change (e.g. writing 变更记录.md after build) MUST NOT mark compiled binary stale!
        let mut rep_docs = rep.clone();
        rep_docs.dirty_count = 1;
        rep_docs.classified_changes.push(ClassifiedChangeItem {
            path: "docs/01-当前版/变更记录.md".into(),
            status: "M".into(),
            category: "docs".into(),
        });
        let r_docs = inspect_deploy_readiness(&proj, &versions, &git, &arts, &rep_docs, false);
        assert_eq!(r_docs.status, "ready_to_deploy");
        assert!(!r_docs.source_newer_than_artifacts);

        // Source code dirty change MUST block ready_to_deploy and flag stale!
        let mut rep_src = rep.clone();
        rep_src.dirty_count = 1;
        rep_src.classified_changes.push(ClassifiedChangeItem {
            path: "src/main.rs".into(),
            status: "M".into(),
            category: "source".into(),
        });
        let r_src = inspect_deploy_readiness(&proj, &versions, &git, &arts, &rep_src, false);
        assert_eq!(r_src.status, "partial_artifacts");
        assert!(r_src.source_newer_than_artifacts);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_multi_framework_unversioned_manifest_artifact_and_stale_version_exclusion() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_dep_multi_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(103)
        ));
        let rel = dir.join("release");
        fs::create_dir_all(&rel).unwrap();

        // Go/CLI tool with unversioned binary referenced by latest.json + an old 1.0.0 archive that MUST be excluded
        fs::write(rel.join("mycli-win64.zip"), b"go-cli-binary-v2").unwrap();
        fs::write(rel.join("mycli_1.0.0_win64.zip"), b"old-v1-binary").unwrap();
        let sha = packager::sha256_file(&rel.join("mycli-win64.zip")).unwrap();
        fs::write(
            rel.join("latest.json"),
            format!(
                r#"{{"version":"2.0.0","portable":{{"file":"mycli-win64.zip","size":16,"sha256":"{}"}}}}"#,
                sha
            ),
        )
        .unwrap();

        let arts = packager::scan_artifacts(&rel);
        let picked = discover_version_deploy_artifacts(&rel, &arts, "2.0.0");
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].name, "mycli-win64.zip");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_redteam_round1_misaligned_tag_and_unversioned_0_0_0_blocked() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_dep_rt1_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(107)
        ));
        let rel = dir.join("release");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(&rel).unwrap();

        let mut init_cmd = Command::new("git");
        init_cmd.arg("-C").arg(&dir).arg("init");
        #[cfg(windows)]
        init_cmd.creation_flags(CREATE_NO_WINDOW);
        assert!(init_cmd.output().map(|o| o.status.success()).unwrap_or(false));
        assert!(run_git_quick(&dir, &["config", "user.email", "t@t.com"]).is_some());
        assert!(run_git_quick(&dir, &["config", "user.name", "Tester"]).is_some());
        fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        assert!(run_git_quick(&dir, &["add", "."]).is_some());
        assert!(run_git_quick(&dir, &["commit", "-m", "v1.0.0"]).is_some());
        assert!(run_git_quick(&dir, &["tag", "v1.0.0"]).is_some());

        // Make a second commit on HEAD while v1.0.0 tag still points to the OLD commit!
        fs::write(dir.join("README.md"), "# Updated\n").unwrap();
        assert!(run_git_quick(&dir, &["add", "README.md"]).is_some());
        assert!(run_git_quick(&dir, &["commit", "-m", "docs: update"]).is_some());
        fs::write(rel.join("DemoApp_1.0.0_x64-setup.exe"), b"binary-v1").unwrap();

        let proj = make_proj(&dir, "tauri");
        let versions = make_versions("1.0.0", true);
        let git = crate::git_ops::inspect_git_for_project(&dir, "release");
        let arts = packager::scan_artifacts(&rel);
        let rep = make_clean_report(&proj.id, "1.0.0");

        let readiness = inspect_deploy_readiness(&proj, &versions, &git, &arts, &rep, false);
        assert_eq!(readiness.status, "partial_artifacts", "misaligned tag must block ready_to_deploy: {:?}", readiness);
        assert!(readiness.blockers.iter().any(|b| b.contains("指向旧提交")));

        // Preflight MUST reject misaligned tag push
        let req_mis = DeployPushRequest {
            project_id: proj.id.clone(),
            version: "1.0.0".into(),
            channel: "stable".into(),
            ensure_manifest: true,
            verify_checksums: true,
            git_mode: "tag_push".into(),
            git_commit_message: String::new(),
            upload_release: false,
            draft: false,
            overwrite_assets: true,
            release_notes: String::new(),
            artifact_files: vec![],
            force_stale_source: false,
        };
        let err_mis = preflight_deploy_push(&proj, &req_mis, &readiness, &versions, &git, &rel, &arts)
            .unwrap_err()
            .to_string();
        assert!(err_mis.contains("指向旧提交"), "err={}", err_mis);

        // Preflight MUST reject unversioned 0.0.0 remote publish
        let req_zero = DeployPushRequest {
            version: "0.0.0".into(),
            git_mode: "none".into(),
            upload_release: true,
            ..req_mis
        };
        let err_zero = preflight_deploy_push(&proj, &req_zero, &readiness, &make_versions("0.0.0", true), &git, &rel, &arts)
            .unwrap_err()
            .to_string();
        assert!(err_zero.contains("v0.0.0"), "err={}", err_zero);

        // Red-Team Round 2: force_stale_source + commit_tag_push MUST be blocked (never tag unbuilt code!)
        let mut stale_readiness = readiness.clone();
        stale_readiness.source_newer_than_artifacts = true;
        stale_readiness.stale_reason = "src/main.rs modified".into();
        let req_stale_commit = DeployPushRequest {
            version: "2.0.0".into(),
            git_mode: "commit_tag_push".into(),
            upload_release: false,
            force_stale_source: true,
            ..req_zero
        };
        let mut arts_v2 = arts.clone();
        arts_v2[0].version = Some("2.0.0".into());
        let err_stale_commit = preflight_deploy_push(
            &proj,
            &req_stale_commit,
            &stale_readiness,
            &make_versions("2.0.0", true),
            &git,
            &rel,
            &arts_v2,
        )
        .unwrap_err()
        .to_string();
        assert!(err_stale_commit.contains("禁止同时将未编译的新源码提交"), "err={}", err_stale_commit);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_unbound_non_git_project_blocks_ready_to_deploy_cleanly() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_dep_unbound_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(109)
        ));
        let rel = dir.join("release");
        fs::create_dir_all(&rel).unwrap();
        fs::write(rel.join("LocalApp_1.0.0_x64-setup.exe"), b"setup-binary").unwrap();

        let mut proj = make_proj(&dir, "tauri");
        proj.github_repo.clear();
        proj.repo_binding = RepoBinding::default();
        let git = GitStatus {
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
        };
        let arts = packager::scan_artifacts(&rel);
        let rep = make_clean_report(&proj.id, "1.0.0");
        let r = inspect_deploy_readiness(&proj, &make_versions("1.0.0", true), &git, &arts, &rep, false);
        assert_eq!(r.status, "partial_artifacts", "unbound non-git project must not pretend ready_to_deploy: {:?}", r);
        assert!(r.blockers.iter().any(|b| b.contains("尚未绑定目标发布仓库")));

        let _ = fs::remove_dir_all(&dir);
    }
}
