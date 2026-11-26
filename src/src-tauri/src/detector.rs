use crate::models::{
    ActionSpec, DetectedLanguage, DetectedShell, DiscoveredProject, PackageProfile, ProjectConfig,
    ProjectProfileMatrix, ProjectSubmodule, RepoBinding, VersionFileSpec,
};
use crate::version;
use anyhow::{anyhow, Result};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn gh_repo_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"github\.com[:/]([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+?)(?:\.git)?(?:/|"|'|\s|$)"#).unwrap()
    })
}

fn lock_path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)([A-Za-z]:\\[^'"\r\n)\s]+?\.build\.lock)"#).unwrap()
    })
}

fn journal_path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)['"]([A-Za-z]:\\[^'"\r\n]+?builds?\.jsonl)['"]"#).unwrap()
    })
}

fn cargo_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?m)^\s*name\s*=\s*"([^"]+)""#).unwrap())
}

fn hidden_cmd(prog: &str) -> Command {
    let mut cmd = Command::new(prog);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

pub fn preferred_pwsh() -> String {
    crate::toolchain::preferred_pwsh_native()
}

fn slugify_id(root: &Path) -> String {
    let name = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = clean.trim_matches('-');
    if trimmed.is_empty() {
        format!("proj-{}", chrono::Local::now().timestamp())
    } else {
        trimmed.to_string()
    }
}

pub fn detect_github_repo(root: &Path) -> String {
    // 1. Primary source: git remote origin
    if root.join(".git").exists() {
        if let Ok(o) = hidden_cmd("git")
            .args(["-C", &root.to_string_lossy(), "remote", "get-url", "origin"])
            .output()
        {
            if o.status.success() {
                let url = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let parsed = parse_github_owner_repo(&url);
                if !parsed.is_empty() {
                    return parsed;
                }
            }
        }
    }
    // 2. Fallback: tauri.conf.json plugins.updater.endpoints (structured JSON only — never match unrelated strings)
    if let Some(conf) = find_tauri_conf(root) {
        if let Ok(raw) = fs::read_to_string(&conf) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim_start_matches('\u{feff}')) {
                if let Some(eps) = v
                    .get("plugins")
                    .and_then(|p| p.get("updater"))
                    .and_then(|u| u.get("endpoints"))
                    .and_then(|e| e.as_array())
                {
                    for ep in eps {
                        if let Some(s) = ep.as_str() {
                            let parsed = parse_github_owner_repo(s);
                            if !parsed.is_empty() {
                                return parsed;
                            }
                        }
                    }
                }
            }
        }
    }
    // 3. Fallback: package.json top-level `repository` field only
    if let Ok(raw) = fs::read_to_string(root.join("package.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim_start_matches('\u{feff}')) {
            let repo_url = v
                .get("repository")
                .and_then(|r| r.as_str().or_else(|| r.get("url").and_then(|u| u.as_str())))
                .unwrap_or("");
            let parsed = parse_github_owner_repo(repo_url);
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }
    // 4. Fallback: Cargo.toml `[package]` `repository = "..."` field only (NEVER scan `[dependencies]`!)
    for rel in ["Cargo.toml", "src/src-tauri/Cargo.toml", "src-tauri/Cargo.toml"] {
        if let Ok(raw) = fs::read_to_string(root.join(rel)) {
            let mut in_pkg = false;
            for line in raw.lines() {
                let t = line.trim();
                if t.starts_with('[') {
                    in_pkg = t.eq_ignore_ascii_case("[package]");
                    continue;
                }
                if in_pkg && t.starts_with("repository") && t.contains('=') {
                    let parsed = parse_github_owner_repo(t);
                    if !parsed.is_empty() {
                        return parsed;
                    }
                }
            }
        }
    }
    String::new()
}

fn detect_lock_file_from_scripts(root: &Path) -> String {
    let dir_var_re = Regex::new(r#"(?im)\$(?:script:)?([A-Za-z0-9_]+)\s*=\s*['"]([A-Za-z]:\\[^'"\r\n]+?)['"]"#).ok();
    let join_lock_re = Regex::new(r#"(?i)Join-Path\s+\$(?:script:)?([A-Za-z0-9_]+)\s+['"]([^'"\r\n\\/:*?"<>|]+?\.build\.lock)['"]"#).ok();

    let is_safe_lock = |p: &str| -> bool {
        !p.starts_with("\\\\") && !p.contains("..") && !p.chars().any(|c| c.is_control())
    };

    for rel in ["tools/build-manager.ps1", "scripts/build-manager.ps1", "build.ps1"] {
        let p = root.join(rel);
        if let Ok(raw) = fs::read_to_string(&p) {
            if let Some(caps) = lock_path_re().captures(&raw) {
                let cand = caps[1].to_string();
                if is_safe_lock(&cand) {
                    return cand;
                }
            }
            if let (Some(dv), Some(jl)) = (&dir_var_re, &join_lock_re) {
                let mut vars: HashMap<String, String> = HashMap::new();
                for c in dv.captures_iter(&raw) {
                    vars.insert(c[1].to_ascii_lowercase(), c[2].trim_end_matches('\\').to_string());
                }
                if let Some(jc) = jl.captures(&raw) {
                    if let Some(base) = vars.get(&jc[1].to_ascii_lowercase()) {
                        let cand = format!("{}\\{}", base, &jc[2]);
                        if is_safe_lock(&cand) {
                            return cand;
                        }
                    }
                }
            }
        }
    }
    String::new()
}

fn detect_journal_file_from_scripts(root: &Path) -> String {
    for rel in ["tools/build-manager.ps1", "scripts/build-manager.ps1", "build.ps1"] {
        let p = root.join(rel);
        if let Ok(raw) = fs::read_to_string(&p) {
            if let Some(caps) = journal_path_re().captures(&raw) {
                let cand = caps[1].to_string();
                if !cand.starts_with("\\\\") && !cand.contains("..") && !cand.chars().any(|c| c.is_control()) {
                    return cand;
                }
            }
        }
    }
    String::new()
}

pub fn parse_github_owner_repo(url: &str) -> String {
    if let Some(caps) = gh_repo_re().captures(url.trim()) {
        return caps[1].to_string();
    }
    String::new()
}

/// Parse Git remote URL into a multi-platform `RepoBinding` (GitHub / Gitee / GitLab / Gitea).
pub fn parse_repo_binding_from_url(url: &str) -> Option<RepoBinding> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Support both HTTPS (https://host/owner/repo.git) and SSH (git@host:owner/repo.git)
    let (host, path_part) = if let Some(rest) = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
    {
        let (h, p) = rest.split_once('/')?;
        let h_clean = h.rsplit('@').next().unwrap_or(h);
        (h_clean.to_string(), p.to_string())
    } else if let Some(rest) = trimmed.strip_prefix("git@") {
        let (h, p) = rest.split_once(':')?;
        (h.to_string(), p.to_string())
    } else {
        return None;
    };

    let slug = path_part
        .trim_matches('/')
        .trim_end_matches(".git")
        .to_string();
    if !slug.contains('/') {
        return None;
    }
    let lower_host = host.to_ascii_lowercase();
    if lower_host == "github.com" {
        Some(RepoBinding {
            provider: "github".into(),
            base_url: String::new(),
            repo_slug: slug,
            credential_id: String::new(),
        })
    } else if lower_host == "gitee.com" {
        Some(RepoBinding {
            provider: "gitee".into(),
            base_url: String::new(),
            repo_slug: slug,
            credential_id: String::new(),
        })
    } else if lower_host == "gitlab.com" || lower_host.contains("gitlab") {
        Some(RepoBinding {
            provider: "gitlab".into(),
            base_url: if lower_host == "gitlab.com" {
                String::new()
            } else {
                format!("https://{}", host)
            },
            repo_slug: slug,
            credential_id: String::new(),
        })
    } else {
        Some(RepoBinding {
            provider: "gitea".into(),
            base_url: format!("https://{}", host),
            repo_slug: slug,
            credential_id: String::new(),
        })
    }
}

pub fn detect_repo_binding(root: &Path) -> RepoBinding {
    if root.join(".git").exists() {
        if let Ok(o) = hidden_cmd("git")
            .args(["-C", &root.to_string_lossy(), "remote", "get-url", "origin"])
            .output()
        {
            if o.status.success() {
                let url = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if let Some(binding) = parse_repo_binding_from_url(&url) {
                    return binding;
                }
            }
        }
    }
    let gh = detect_github_repo(root);
    RepoBinding {
        provider: "github".into(),
        base_url: String::new(),
        repo_slug: gh,
        credential_id: String::new(),
    }
}

fn find_changelog(root: &Path) -> String {
    let candidates = [
        "docs/01-当前版/变更记录.md",
        "CHANGELOG.md",
        "changelog.md",
        "docs/CHANGELOG.md",
        "HISTORY.md",
        "RELEASE_NOTES.md",
    ];
    for c in candidates {
        if root.join(c).exists() {
            return c.replace('/', "\\");
        }
    }
    "CHANGELOG.md".to_string()
}

fn find_release_dir(root: &Path) -> String {
    for c in ["release", "releases", "dist", "out", "build/release"] {
        if root.join(c).is_dir() {
            return c.replace('/', "\\");
        }
    }
    "release".to_string()
}

fn find_tauri_conf(root: &Path) -> Option<PathBuf> {
    for c in [
        "src/src-tauri/tauri.conf.json",
        "src-tauri/tauri.conf.json",
        "tauri.conf.json",
    ] {
        let p = root.join(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn detect_pkg_manager(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if root.join("yarn.lock").exists() {
        "yarn"
    } else if root.join("bun.lockb").exists() || root.join("bun.lock").exists() {
        "bun"
    } else {
        "npm"
    }
}

fn find_first_ext(root: &Path, ext: &str) -> Option<String> {
    let Ok(rd) = fs::read_dir(root) else {
        return None;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_file() {
            if let Some(e) = p.extension().and_then(|s| s.to_str()) {
                if e.eq_ignore_ascii_case(ext) {
                    return p
                        .file_name()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string());
                }
            }
        }
    }
    None
}

struct BuildPs1Capabilities {
    has_bump: bool,
    has_channel: bool,
    has_skip_tests: bool,
    has_tests_only: bool,
    has_app_only: bool,
    has_engine: bool,
    has_portable: bool,
    has_status: bool,
    has_clean: bool,
}

/// Introspect a project's `build.ps1` param(...) block so we only generate actions the script actually supports!
fn inspect_build_ps1(path: &Path) -> BuildPs1Capabilities {
    let raw = fs::read_to_string(path).unwrap_or_default();
    let head: String = raw.chars().take(4000).collect();
    let has = |param: &str| -> bool {
        let needle = format!("${}", param);
        head.to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    };
    BuildPs1Capabilities {
        has_bump: has("Bump"),
        has_channel: has("Channel"),
        has_skip_tests: has("SkipTests"),
        has_tests_only: has("TestsOnly"),
        has_app_only: has("AppOnly"),
        has_engine: has("Engine"),
        has_portable: has("Portable"),
        has_status: has("Status"),
        has_clean: has("Clean"),
    }
}

/// Generate accurate `build.ps1` actions by introspecting the script's real `param(...)` switches.
/// Shared by both `detector::detect_project` and `presets::apply_preset_to_project`.
pub(crate) fn build_ps1_actions(root: &Path, ps_bin: &str) -> Vec<ActionSpec> {
    let build_ps1_path = root.join("build.ps1");
    let caps = inspect_build_ps1(&build_ps1_path);
    let mut actions = Vec::new();
    actions.push(ActionSpec {
        id: "build-full".into(),
        label: "全量构建与打包".into(),
        category: "build".into(),
        command: ps_bin.to_string(),
        args: vec!["-NoProfile".into(), "-File".into(), "build.ps1".into()],
        cwd: String::new(),
        description: "测试门禁 + 构建外壳 + NSIS 安装包 + 便携 ZIP".into(),
        dangerous: false,
        supports_bump: caps.has_bump,
        supports_channel: caps.has_channel,
        supports_skip_tests: caps.has_skip_tests,
    });
    if caps.has_tests_only {
        actions.push(ActionSpec {
            id: "test-gate".into(),
            label: "运行测试门禁".into(),
            category: "test".into(),
            command: ps_bin.to_string(),
            args: vec![
                "-NoProfile".into(),
                "-File".into(),
                "build.ps1".into(),
                "-TestsOnly".into(),
            ],
            cwd: String::new(),
            description: "仅执行全量测试门禁，不改版本与产物".into(),
            dangerous: false,
            supports_bump: false,
            supports_channel: false,
            supports_skip_tests: false,
        });
    }
    if caps.has_app_only {
        actions.push(ActionSpec {
            id: "build-app-only".into(),
            label: "仅构建主程序".into(),
            category: "build".into(),
            command: ps_bin.to_string(),
            args: vec![
                "-NoProfile".into(),
                "-File".into(),
                "build.ps1".into(),
                "-AppOnly".into(),
            ],
            cwd: String::new(),
            description: "快速增量编译主程序 EXE，跳过安装包与压缩".into(),
            dangerous: false,
            supports_bump: caps.has_bump,
            supports_channel: false,
            supports_skip_tests: caps.has_skip_tests,
        });
    }
    if caps.has_engine {
        actions.push(ActionSpec {
            id: "build-engine".into(),
            label: "重建子引擎".into(),
            category: "build".into(),
            command: ps_bin.to_string(),
            args: vec![
                "-NoProfile".into(),
                "-File".into(),
                "build.ps1".into(),
                "-Engine".into(),
            ],
            cwd: String::new(),
            description: "执行 build.ps1 -Engine 重建子引擎组件".into(),
            dangerous: false,
            supports_bump: false,
            supports_channel: false,
            supports_skip_tests: false,
        });
    }
    if caps.has_portable {
        actions.push(ActionSpec {
            id: "pack-portable".into(),
            label: "补打便携版 ZIP".into(),
            category: "package".into(),
            command: ps_bin.to_string(),
            args: vec![
                "-NoProfile".into(),
                "-File".into(),
                "build.ps1".into(),
                "-Portable".into(),
            ],
            cwd: String::new(),
            description: "使用当前已构建程序生成便携压缩包".into(),
            dangerous: false,
            supports_bump: false,
            supports_channel: false,
            supports_skip_tests: false,
        });
    }
    if caps.has_status {
        actions.push(ActionSpec {
            id: "build-status".into(),
            label: "构建一致性自检".into(),
            category: "maintenance".into(),
            command: ps_bin.to_string(),
            args: vec![
                "-NoProfile".into(),
                "-File".into(),
                "build.ps1".into(),
                "-Status".into(),
            ],
            cwd: String::new(),
            description: "检查版本对齐、锁占用、运行副本与 release 状态".into(),
            dangerous: false,
            supports_bump: false,
            supports_channel: false,
            supports_skip_tests: false,
        });
    }
    if caps.has_clean {
        actions.push(ActionSpec {
            id: "build-clean".into(),
            label: "清理缓存并全量重建".into(),
            category: "maintenance".into(),
            command: ps_bin.to_string(),
            args: vec![
                "-NoProfile".into(),
                "-File".into(),
                "build.ps1".into(),
                "-Clean".into(),
            ],
            cwd: String::new(),
            description: "清空 target 缓存后重新执行完整构建".into(),
            dangerous: true,
            supports_bump: caps.has_bump,
            supports_channel: caps.has_channel,
            supports_skip_tests: caps.has_skip_tests,
        });
    }
    actions
}

pub fn detect_project(root_path: &Path) -> Result<ProjectConfig> {
    if !root_path.exists() || !root_path.is_dir() {
        return Err(anyhow!("项目目录不存在：{}", root_path.display()));
    }
    let root_str = root_path.to_string_lossy().to_string();
    let folder_name = root_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Project")
        .to_string();
    let id = slugify_id(root_path);
    let build_ps1_path = root_path.join("build.ps1");
    let has_build_ps1 = build_ps1_path.exists();
    let has_publish_ps1 = root_path.join("tools/publish-release.ps1").exists();
    let has_build_manager = root_path.join("tools/build-manager.ps1").exists()
        || root_path.join("scripts/build-manager.ps1").exists();
    let changelog_path = find_changelog(root_path);
    let release_dir = find_release_dir(root_path);
    let github_repo = detect_github_repo(root_path);

    let mut kind = "custom".to_string();
    let mut adapter_label = "通用脚本项目".to_string();
    let mut product_name = folder_name.clone();
    let mut artifact_prefix = folder_name.clone();
    let mut version_files: Vec<VersionFileSpec> = Vec::new();
    let mut actions: Vec<ActionSpec> = Vec::new();
    let env: HashMap<String, String> = HashMap::new();
    let mut launch_exe = String::new();
    let mut lock_file = String::new();
    let mut journal_file = String::new();
    let engine_src_dir = String::new();
    let engine_exe_path = String::new();

    let ps_bin = preferred_pwsh();

    // 1) Tauri (v1 / v2)
    if let Some(tauri_conf) = find_tauri_conf(root_path) {
        kind = "tauri".to_string();
        let tauri_dir = tauri_conf.parent().unwrap_or(root_path);
        let rel_conf = tauri_conf
            .strip_prefix(root_path)
            .unwrap_or(&tauri_conf)
            .to_string_lossy()
            .replace('/', "\\");
        let rel_tauri_dir = tauri_dir
            .strip_prefix(root_path)
            .unwrap_or(tauri_dir)
            .to_string_lossy()
            .replace('/', "\\");

        if let Ok(raw) = fs::read_to_string(&tauri_conf) {
            if let Ok(v) =
                serde_json::from_str::<serde_json::Value>(raw.trim_start_matches('\u{feff}'))
            {
                if let Some(pn) = v
                    .get("productName")
                    .or_else(|| v.get("package").and_then(|p| p.get("productName")))
                    .and_then(|x| x.as_str())
                {
                    if !pn.trim().is_empty() {
                        product_name = pn.to_string();
                        artifact_prefix = pn.to_string();
                    }
                }
            }
        }

        version_files.push(VersionFileSpec {
            path: rel_conf,
            format: "tauri_json".into(),
            pattern: String::new(),
        });

        let cargo_toml = tauri_dir.join("Cargo.toml");
        if cargo_toml.exists() {
            let rel_cargo = cargo_toml
                .strip_prefix(root_path)
                .unwrap_or(&cargo_toml)
                .to_string_lossy()
                .replace('/', "\\");
            version_files.push(VersionFileSpec {
                path: rel_cargo,
                format: "cargo_toml".into(),
                pattern: String::new(),
            });
            if let Ok(raw) = fs::read_to_string(&cargo_toml) {
                if let Some(c) = cargo_name_re().captures(&raw) {
                    let bin_exe = format!("{}.exe", &c[1]);
                    if root_path.join(&bin_exe).exists() {
                        launch_exe = bin_exe;
                    }
                }
            }
        }
        if root_path.join("package.json").exists() {
            version_files.push(VersionFileSpec {
                path: "package.json".into(),
                format: "package_json".into(),
                pattern: String::new(),
            });
        }

        // Generic optional sidecar engine detection removed hardcoded personal names;
        // users can still configure custom engineSrcDir / engineExePath in project settings.
        if has_build_manager {
            adapter_label = "Tauri 2 · 构建管线 (build.ps1)".into();
            let detected_lock = detect_lock_file_from_scripts(root_path);
            if !detected_lock.is_empty() {
                lock_file = detected_lock;
            }
            let detected_journal = detect_journal_file_from_scripts(root_path);
            if !detected_journal.is_empty() {
                journal_file = detected_journal;
            } else if root_path.join(".build").is_dir() || root_path.join("scripts/build-manager.ps1").exists() {
                journal_file = root_path
                    .join(".build\\builds.jsonl")
                    .to_string_lossy()
                    .to_string();
            }
        } else if has_build_ps1 {
            adapter_label = "Tauri 2 · 脚本构建 (build.ps1)".into();
        } else {
            adapter_label = "Tauri Desktop".into();
        }

        if has_build_ps1 {
            actions.extend(build_ps1_actions(root_path, &ps_bin));
        } else {
            let local_tauri_cli = root_path.join("tools/cargo-tauri.exe");
            let tauri_cmd = if local_tauri_cli.exists() {
                local_tauri_cli.to_string_lossy().to_string()
            } else {
                "cargo-tauri".into()
            };
            actions.extend([
                ActionSpec {
                    id: "tauri-build-nsis".into(),
                    label: "构建 Tauri + NSIS 安装包".into(),
                    category: "build".into(),
                    command: tauri_cmd,
                    args: vec!["build".into(), "--bundles".into(), "nsis".into()],
                    cwd: rel_tauri_dir.clone(),
                    description: "使用 Tauri CLI 编译发布版并生成 NSIS 安装包".into(),
                    dangerous: false,
                    supports_bump: false,
                    supports_channel: false,
                    supports_skip_tests: false,
                },
                ActionSpec {
                    id: "cargo-test".into(),
                    label: "运行 Rust 单元测试".into(),
                    category: "test".into(),
                    command: "cargo".into(),
                    args: vec!["test".into()],
                    cwd: rel_tauri_dir.clone(),
                    description: "执行 src-tauri 下全部 Rust 单元测试".into(),
                    dangerous: false,
                    supports_bump: false,
                    supports_channel: false,
                    supports_skip_tests: false,
                },
                ActionSpec {
                    id: "cargo-check".into(),
                    label: "Cargo Release 检查".into(),
                    category: "test".into(),
                    command: "cargo".into(),
                    args: vec!["check".into(), "--release".into()],
                    cwd: rel_tauri_dir,
                    description: "快速验证发布配置编译与零警告基线".into(),
                    dangerous: false,
                    supports_bump: false,
                    supports_channel: false,
                    supports_skip_tests: false,
                },
            ]);
        }
    }
    // 2) Electron / Node.js / Web
    else if root_path.join("package.json").exists() {
        let pm = detect_pkg_manager(root_path);
        let raw = fs::read_to_string(root_path.join("package.json")).unwrap_or_default();
        let pkg: serde_json::Value =
            serde_json::from_str(raw.trim_start_matches('\u{feff}')).unwrap_or_default();
        let has_electron = pkg.get("dependencies").and_then(|d| d.get("electron")).is_some()
            || pkg
                .get("devDependencies")
                .and_then(|d| d.get("electron"))
                .is_some()
            || pkg
                .get("devDependencies")
                .and_then(|d| d.get("electron-builder"))
                .is_some()
            || pkg
                .get("devDependencies")
                .and_then(|d| d.get("@electron-forge/cli"))
                .is_some();

        if let Some(n) = pkg.get("name").and_then(|x| x.as_str()) {
            product_name = n.to_string();
            artifact_prefix = n.replace(['@', '/'], "_");
        }
        kind = if has_electron { "electron" } else { "node" }.into();
        adapter_label = if has_electron {
            format!("Electron ({})", pm)
        } else {
            format!("Node.js / Web ({})", pm)
        };
        version_files.push(VersionFileSpec {
            path: "package.json".into(),
            format: "package_json".into(),
            pattern: String::new(),
        });

        if let Some(scripts) = pkg.get("scripts").and_then(|s| s.as_object()) {
            let preferred = [
                ("test", "运行测试", "test"),
                ("lint", "规范检查", "test"),
                ("typecheck", "类型检查", "test"),
                ("build", "构建产物", "build"),
                ("dist", "打包分发包", "package"),
                ("pack", "本地打包", "package"),
                ("make", "Electron Forge 打包", "package"),
                ("package", "打包应用", "package"),
                ("release", "发布脚本", "publish"),
            ];
            let mut added = Vec::new();
            for (k, label, cat) in preferred {
                if scripts.contains_key(k) {
                    added.push(k.to_string());
                    actions.push(ActionSpec {
                        id: format!("script-{}", k),
                        label: format!("{} ({})", label, k),
                        category: cat.into(),
                        command: pm.into(),
                        args: vec!["run".into(), k.into()],
                        cwd: String::new(),
                        description: scripts
                            .get(k)
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        dangerous: cat == "publish",
                        supports_bump: false,
                        supports_channel: false,
                        supports_skip_tests: false,
                    });
                }
            }
            for (k, v) in scripts {
                if added.contains(k) || actions.len() >= 12 {
                    continue;
                }
                actions.push(ActionSpec {
                    id: format!("script-{}", k),
                    label: format!("脚本：{}", k),
                    category: "custom".into(),
                    command: pm.into(),
                    args: vec!["run".into(), k.into()],
                    cwd: String::new(),
                    description: v.as_str().unwrap_or("").to_string(),
                    dangerous: false,
                    supports_bump: false,
                    supports_channel: false,
                    supports_skip_tests: false,
                });
            }
        }
    }
    // 3) Rust / Cargo
    else if root_path.join("Cargo.toml").exists() {
        kind = "rust".into();
        adapter_label = "Rust / Cargo".into();
        version_files.push(VersionFileSpec {
            path: "Cargo.toml".into(),
            format: "cargo_toml".into(),
            pattern: String::new(),
        });
        if let Ok(raw) = fs::read_to_string(root_path.join("Cargo.toml")) {
            if let Some(c) = cargo_name_re().captures(&raw) {
                product_name = c[1].to_string();
                artifact_prefix = c[1].to_string();
                launch_exe = format!("target\\release\\{}.exe", &c[1]);
            }
        }
        actions.extend([
            ActionSpec {
                id: "cargo-build-release".into(),
                label: "编译 Release 二进制".into(),
                category: "build".into(),
                command: "cargo".into(),
                args: vec!["build".into(), "--release".into()],
                cwd: String::new(),
                description: "编译优化后的 Rust 发布二进制".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
            ActionSpec {
                id: "cargo-test".into(),
                label: "运行 Cargo 测试".into(),
                category: "test".into(),
                command: "cargo".into(),
                args: vec!["test".into()],
                cwd: String::new(),
                description: "运行全部单元测试与集成测试".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
            ActionSpec {
                id: "cargo-clippy".into(),
                label: "Clippy 静态审计".into(),
                category: "test".into(),
                command: "cargo".into(),
                args: vec!["clippy".into(), "--all-targets".into()],
                cwd: String::new(),
                description: "检查潜在缺陷与冗余代码".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
            ActionSpec {
                id: "cargo-clean".into(),
                label: "清理 Target 缓存".into(),
                category: "maintenance".into(),
                command: "cargo".into(),
                args: vec!["clean".into()],
                cwd: String::new(),
                description: "清理 Rust 编译缓存目录".into(),
                dangerous: true,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
        ]);
    }
    // 4) Go
    else if root_path.join("go.mod").exists() {
        kind = "go".into();
        adapter_label = "Go Module".into();
        let bin = format!("release\\{}.exe", folder_name);
        launch_exe = bin.clone();
        actions.extend([
            ActionSpec {
                id: "go-build".into(),
                label: "编译精简 Release 二进制".into(),
                category: "build".into(),
                command: "go".into(),
                args: vec![
                    "build".into(),
                    "-trimpath".into(),
                    "-ldflags".into(),
                    "-s -w".into(),
                    "-o".into(),
                    bin,
                    ".".into(),
                ],
                cwd: String::new(),
                description: "去除符号表与调试信息以减小体积".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
            ActionSpec {
                id: "go-test".into(),
                label: "运行 Go 测试".into(),
                category: "test".into(),
                command: "go".into(),
                args: vec!["test".into(), "./...".into()],
                cwd: String::new(),
                description: "运行全模块单元测试".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
            ActionSpec {
                id: "go-vet".into(),
                label: "Go Vet 静态检查".into(),
                category: "test".into(),
                command: "go".into(),
                args: vec!["vet".into(), "./...".into()],
                cwd: String::new(),
                description: "静态代码分析".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
        ]);
    }
    // 5) Python / PyInstaller / Wheel
    else if root_path.join("pyproject.toml").exists()
        || root_path.join("setup.py").exists()
        || find_first_ext(root_path, "spec").is_some()
        || root_path.join("requirements.txt").exists()
    {
        kind = "python".into();
        adapter_label = "Python / PyInstaller".into();
        if root_path.join("pyproject.toml").exists() {
            version_files.push(VersionFileSpec {
                path: "pyproject.toml".into(),
                format: "pyproject_toml".into(),
                pattern: String::new(),
            });
        }
        let py = if root_path.join(".venv/Scripts/python.exe").exists() {
            ".venv\\Scripts\\python.exe".to_string()
        } else if root_path.join("venv/Scripts/python.exe").exists() {
            "venv\\Scripts\\python.exe".to_string()
        } else {
            "python".to_string()
        };
        if let Some(spec) = find_first_ext(root_path, "spec") {
            actions.push(ActionSpec {
                id: "pyinstaller-spec".into(),
                label: format!("PyInstaller 打包 ({})", spec),
                category: "package".into(),
                command: py.clone(),
                args: vec!["-m".into(), "PyInstaller".into(), "--noconfirm".into(), spec],
                cwd: String::new(),
                description: "使用 .spec 配置构建独立可执行包".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            });
        }
        if root_path.join("pyproject.toml").exists() || root_path.join("setup.py").exists() {
            actions.push(ActionSpec {
                id: "py-build-wheel".into(),
                label: "构建 Wheel & sdist 包".into(),
                category: "package".into(),
                command: py.clone(),
                args: vec!["-m".into(), "build".into()],
                cwd: String::new(),
                description: "生成标准 Python 分发包到 dist/".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            });
        }
        actions.push(ActionSpec {
            id: "py-test".into(),
            label: "运行 Python 测试".into(),
            category: "test".into(),
            command: py,
            args: vec!["-m".into(), "unittest".into(), "discover".into()],
            cwd: String::new(),
            description: "发现并运行 unittest 套件".into(),
            dangerous: false,
            supports_bump: false,
            supports_channel: false,
            supports_skip_tests: false,
        });
    }
    // 6) .NET / C#
    else if let Some(csproj) = find_first_ext(root_path, "csproj") {
        kind = "dotnet".into();
        adapter_label = ".NET / C#".into();
        version_files.push(VersionFileSpec {
            path: csproj,
            format: "csproj".into(),
            pattern: String::new(),
        });
        actions.extend([
            ActionSpec {
                id: "dotnet-publish".into(),
                label: "发布 Win-x64 Release".into(),
                category: "package".into(),
                command: "dotnet".into(),
                args: vec![
                    "publish".into(),
                    "-c".into(),
                    "Release".into(),
                    "-r".into(),
                    "win-x64".into(),
                    "-o".into(),
                    "release\\win-x64".into(),
                ],
                cwd: String::new(),
                description: "编译并发布到 release\\win-x64".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
            ActionSpec {
                id: "dotnet-test".into(),
                label: "运行 Dotnet 测试".into(),
                category: "test".into(),
                command: "dotnet".into(),
                args: vec!["test".into()],
                cwd: String::new(),
                description: "运行项目测试集".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
        ]);
    }
    // 7) Flutter / Dart
    else if root_path.join("pubspec.yaml").exists() {
        kind = "flutter".into();
        adapter_label = "Flutter / Dart".into();
        version_files.push(VersionFileSpec {
            path: "pubspec.yaml".into(),
            format: "pubspec_yaml".into(),
            pattern: String::new(),
        });
        actions.extend([
            ActionSpec {
                id: "flutter-build-win".into(),
                label: "构建 Windows Release".into(),
                category: "build".into(),
                command: "flutter".into(),
                args: vec!["build".into(), "windows".into(), "--release".into()],
                cwd: String::new(),
                description: "编译 Windows 桌面发布版".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
            ActionSpec {
                id: "flutter-test".into(),
                label: "运行 Flutter 测试".into(),
                category: "test".into(),
                command: "flutter".into(),
                args: vec!["test".into()],
                cwd: String::new(),
                description: "执行单元与组件测试".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
        ]);
    }
    // 7.5) Wails (Go + Web Desktop)
    else if root_path.join("wails.json").exists() {
        kind = "wails".into();
        adapter_label = "Wails Desktop (Go)".into();
        actions.extend([
            ActionSpec {
                id: "wails-build".into(),
                label: "构建 Wails 桌面程序".into(),
                category: "build".into(),
                command: "wails".into(),
                args: vec!["build".into()],
                cwd: String::new(),
                description: "编译 Go 后端与前端界面并输出桌面可执行程序".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
            ActionSpec {
                id: "wails-build-nsis".into(),
                label: "构建 Wails + NSIS 安装包".into(),
                category: "package".into(),
                command: "wails".into(),
                args: vec!["build".into(), "-nsis".into()],
                cwd: String::new(),
                description: "编译并打包 Windows NSIS 安装程序".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
            ActionSpec {
                id: "go-test".into(),
                label: "运行 Go 测试".into(),
                category: "test".into(),
                command: "go".into(),
                args: vec!["test".into(), "./...".into()],
                cwd: String::new(),
                description: "运行后端单元测试".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
        ]);
    }
    // 7.6) C/C++ (CMake)
    else if root_path.join("CMakeLists.txt").exists() {
        kind = "cpp".into();
        adapter_label = "C/C++ (CMake)".into();
        actions.extend([
            ActionSpec {
                id: "cmake-configure".into(),
                label: "配置 CMake (Release)".into(),
                category: "build".into(),
                command: "cmake".into(),
                args: vec![
                    "-B".into(),
                    "build".into(),
                    "-DCMAKE_BUILD_TYPE=Release".into(),
                ],
                cwd: String::new(),
                description: "生成 CMake Release 构建工程".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
            ActionSpec {
                id: "cmake-build".into(),
                label: "编译 CMake Release".into(),
                category: "build".into(),
                command: "cmake".into(),
                args: vec![
                    "--build".into(),
                    "build".into(),
                    "--config".into(),
                    "Release".into(),
                ],
                cwd: String::new(),
                description: "编译 C/C++ 发布目标".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
            ActionSpec {
                id: "ctest-run".into(),
                label: "运行 CTest 测试".into(),
                category: "test".into(),
                command: "ctest".into(),
                args: vec!["--test-dir".into(), "build".into(), "--output-on-failure".into()],
                cwd: String::new(),
                description: "执行 CMake 注册的测试套件".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            },
        ]);
    }
    // 7.7) Java / Kotlin (Maven / Gradle)
    else if root_path.join("pom.xml").exists()
        || root_path.join("build.gradle").exists()
        || root_path.join("build.gradle.kts").exists()
    {
        kind = "java".into();
        let is_maven = root_path.join("pom.xml").exists();
        adapter_label = if is_maven {
            "Java / Maven".into()
        } else {
            "Java / Gradle".into()
        };
        if is_maven {
            let mvn_bin: String = if root_path.join("mvnw.cmd").exists() {
                "mvnw.cmd".into()
            } else {
                "mvn.cmd".into()
            };
            actions.extend([
                ActionSpec {
                    id: "mvn-package".into(),
                    label: "Maven 打包 (package)".into(),
                    category: "package".into(),
                    command: mvn_bin.clone(),
                    args: vec!["package".into()],
                    cwd: String::new(),
                    description: "运行测试并打包生成 JAR/WAR 产物".into(),
                    dangerous: false,
                    supports_bump: false,
                    supports_channel: false,
                    supports_skip_tests: false,
                },
                ActionSpec {
                    id: "mvn-test".into(),
                    label: "运行 Maven 测试".into(),
                    category: "test".into(),
                    command: mvn_bin,
                    args: vec!["test".into()],
                    cwd: String::new(),
                    description: "执行单元测试".into(),
                    dangerous: false,
                    supports_bump: false,
                    supports_channel: false,
                    supports_skip_tests: false,
                },
            ]);
        } else {
            let gr_bin: String = if root_path.join("gradlew.bat").exists() {
                "gradlew.bat".into()
            } else {
                "gradle.bat".into()
            };
            actions.extend([
                ActionSpec {
                    id: "gradle-build".into(),
                    label: "Gradle 构建与打包".into(),
                    category: "package".into(),
                    command: gr_bin.clone(),
                    args: vec!["build".into()],
                    cwd: String::new(),
                    description: "编译并生成分发产物".into(),
                    dangerous: false,
                    supports_bump: false,
                    supports_channel: false,
                    supports_skip_tests: false,
                },
                ActionSpec {
                    id: "gradle-test".into(),
                    label: "运行 Gradle 测试".into(),
                    category: "test".into(),
                    command: gr_bin,
                    args: vec!["test".into()],
                    cwd: String::new(),
                    description: "执行单元测试".into(),
                    dangerous: false,
                    supports_bump: false,
                    supports_channel: false,
                    supports_skip_tests: false,
                },
            ]);
        }
    }
    // 8) NSIS / Inno / Generic script
    else {
        if let Some(nsi) = find_first_ext(root_path, "nsi") {
            kind = "nsis".into();
            adapter_label = "NSIS 安装脚本".into();
            actions.push(ActionSpec {
                id: "nsis-build".into(),
                label: format!("编译 NSIS ({})", nsi),
                category: "package".into(),
                command: find_makensis_path().unwrap_or_else(|| "makensis".into()),
                args: vec![nsi],
                cwd: String::new(),
                description: "编译生成 Windows NSIS 安装包".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            });
        }
        if let Some(iss) = find_first_ext(root_path, "iss") {
            kind = "nsis".into();
            adapter_label = "Inno Setup 安装脚本".into();
            actions.push(ActionSpec {
                id: "inno-build".into(),
                label: format!("编译 Inno Setup ({})", iss),
                category: "package".into(),
                command: "iscc".into(),
                args: vec![iss],
                cwd: String::new(),
                description: "编译生成 Inno Setup 安装包".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            });
        }
        if has_build_ps1 {
            let caps = inspect_build_ps1(&build_ps1_path);
            actions.push(ActionSpec {
                id: "ps1-build".into(),
                label: "执行 build.ps1".into(),
                category: "build".into(),
                command: ps_bin,
                args: vec!["-NoProfile".into(), "-File".into(), "build.ps1".into()],
                cwd: String::new(),
                description: "执行项目根目录构建脚本".into(),
                dangerous: false,
                supports_bump: caps.has_bump,
                supports_channel: caps.has_channel,
                supports_skip_tests: caps.has_skip_tests,
            });
        }
    }

    if kind != "nsis" {
        if let Some(nsi) = find_first_ext(root_path, "nsi") {
            actions.push(ActionSpec {
                id: "nsis-script".into(),
                label: format!("编译 NSIS ({})", nsi),
                category: "package".into(),
                command: find_makensis_path().unwrap_or_else(|| "makensis".into()),
                args: vec![nsi],
                cwd: String::new(),
                description: "编译根目录 NSIS 安装包脚本".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            });
        }
        if let Some(iss) = find_first_ext(root_path, "iss") {
            actions.push(ActionSpec {
                id: "inno-script".into(),
                label: format!("编译 Inno ({})", iss),
                category: "package".into(),
                command: "iscc".into(),
                args: vec![iss],
                cwd: String::new(),
                description: "编译根目录 Inno Setup 脚本".into(),
                dangerous: false,
                supports_bump: false,
                supports_channel: false,
                supports_skip_tests: false,
            });
        }
    }

    if launch_exe.is_empty() {
        if let Some(exe) = find_first_ext(root_path, "exe") {
            launch_exe = exe;
        }
    }

    let matrix = analyze_project_profiles(&id, root_path);
    let primary_language = if !matrix.primary_language.is_empty() {
        matrix.primary_language.clone()
    } else {
        match kind.as_str() {
            "tauri" | "rust" => "rust",
            "electron" | "node" => "node",
            "python" => "python",
            "go" | "wails" => "go",
            "dotnet" => "dotnet",
            "flutter" => "dart",
            "cpp" => "cpp",
            "java" => "java",
            _ => "custom",
        }
        .to_string()
    };
    let detected_languages: Vec<String> = matrix.languages.iter().map(|l| l.id.clone()).collect();
    let shell_type = if !matrix.primary_shell.is_empty() {
        matrix.primary_shell.clone()
    } else {
        match kind.as_str() {
            "tauri" => "tauri",
            "electron" => "electron",
            "wails" => "wails",
            "flutter" => "flutter",
            "nsis" => "nsis",
            "node" => "web",
            _ => "portable",
        }
        .to_string()
    };
    let repo_binding = detect_repo_binding(root_path);
    let package_profiles = default_package_profiles(&artifact_prefix, &launch_exe);

    Ok(ProjectConfig {
        id,
        name: if product_name != folder_name && !product_name.is_empty() {
            format!("{} ({})", product_name, folder_name)
        } else {
            folder_name
        },
        root: root_str,
        kind,
        adapter_label,
        primary_language,
        detected_languages,
        shell_type,
        product_name,
        artifact_prefix,
        release_dir,
        changelog_path,
        github_repo,
        repo_binding,
        keep_releases: 2,
        launch_exe,
        lock_file,
        journal_file,
        has_build_ps1,
        has_publish_ps1,
        engine_src_dir,
        engine_exe_path,
        version_files,
        env,
        actions,
        package_profiles,
    })
}

pub fn default_package_profiles(prefix: &str, launch_exe: &str) -> Vec<PackageProfile> {
    default_package_profiles_for_project(prefix, launch_exe, "")
}

pub fn default_package_profiles_for_project(
    prefix: &str,
    launch_exe: &str,
    engine_exe_path: &str,
) -> Vec<PackageProfile> {
    let mut core_sources = Vec::new();
    if !launch_exe.trim().is_empty() {
        core_sources.push(launch_exe.to_string());
    }
    if let Some(top_dir) = engine_exe_path
        .trim()
        .split(['\\', '/'])
        .find(|seg| !seg.is_empty() && *seg != ".")
    {
        if !core_sources.iter().any(|s| s.eq_ignore_ascii_case(top_dir)) {
            core_sources.push(top_dir.to_string());
        }
    }
    for f in ["WebView2Loader.dll", "README.md", "LICENSE", "THIRD_PARTY.md", "dist"] {
        if !core_sources.iter().any(|s| s.eq_ignore_ascii_case(f)) {
            core_sources.push(f.to_string());
        }
    }
    vec![
        PackageProfile {
            id: "portable-core".into(),
            name: "精简便携包".into(),
            format: "zip".into(),
            name_template: format!("{}_{{version}}_x64-portable.zip", prefix),
            sources: core_sources,
            excludes: vec![
                "*.pdb".into(),
                "*.map".into(),
                "*.log".into(),
                ".env*".into(),
                "__pycache__".into(),
                ".git".into(),
                "node_modules".into(),
                "target".into(),
            ],
            split_size_mb: None,
            include_readme: false,
            readme_content: String::new(),
        },
        PackageProfile {
            id: "split-95mb".into(),
            name: "Gitee 兼容分卷包 (95 MB)".into(),
            format: "zip".into(),
            name_template: format!("{}_{{version}}_split.zip", prefix),
            sources: vec![],
            excludes: vec![
                "*.pdb".into(),
                "*.log".into(),
                ".env*".into(),
                ".git".into(),
                "node_modules".into(),
                "target".into(),
            ],
            split_size_mb: Some(95),
            include_readme: false,
            readme_content: String::new(),
        },
        PackageProfile {
            id: "source-archive".into(),
            name: "纯净源码归档 (TAR.GZ)".into(),
            format: "targz".into(),
            name_template: format!("{}_{{version}}_src.tar.gz", prefix),
            sources: vec!["src".into(), "README.md".into(), "LICENSE".into()],
            excludes: vec![
                "target".into(),
                "node_modules".into(),
                ".venv".into(),
                "dist".into(),
                "release".into(),
                ".git".into(),
                ".env*".into(),
            ],
            split_size_mb: None,
            include_readme: false,
            readme_content: String::new(),
        },
    ]
}

/// Multi-signal Language / Shell / Packager matrix analyzer.
pub fn analyze_project_profiles(project_id: &str, root: &Path) -> ProjectProfileMatrix {
    let mut languages: Vec<DetectedLanguage> = Vec::new();
    let mut shells: Vec<DetectedShell> = Vec::new();
    let mut submodules: Vec<ProjectSubmodule> = Vec::new();

    // --- Language signals ---
    let mut rust_ev = Vec::new();
    if root.join("Cargo.toml").exists() {
        rust_ev.push("Cargo.toml".into());
    }
    if root.join("src-tauri/Cargo.toml").exists() || root.join("src/src-tauri/Cargo.toml").exists() {
        rust_ev.push("src-tauri/Cargo.toml".into());
    }
    if !rust_ev.is_empty() {
        languages.push(DetectedLanguage {
            id: "rust".into(),
            name: "Rust".into(),
            confidence: 95,
            evidence: rust_ev,
        });
    }

    let mut node_ev = Vec::new();
    if root.join("package.json").exists() {
        node_ev.push("package.json".into());
    }
    if root.join("tsconfig.json").exists() {
        node_ev.push("tsconfig.json".into());
    }
    if !node_ev.is_empty() {
        languages.push(DetectedLanguage {
            id: "node".into(),
            name: "Node.js / TypeScript".into(),
            confidence: 90,
            evidence: node_ev,
        });
    }

    let mut py_ev = Vec::new();
    for f in ["pyproject.toml", "setup.py", "requirements.txt"] {
        if root.join(f).exists() {
            py_ev.push(f.into());
        }
    }
    if let Some(sp) = find_first_ext(root, "spec") {
        py_ev.push(sp);
    }
    // Also detect embedded Python sub-engines in `src/engine/` or `engine/` (e.g., hybrid Tauri + Python apps)
    for eng_rel in ["src/engine", "engine"] {
        let eng_dir = root.join(eng_rel);
        if eng_dir.is_dir() {
            let has_py = eng_dir.join("requirements.txt").exists()
                || eng_dir.join("pyproject.toml").exists()
                || find_first_ext(&eng_dir, "py").is_some()
                || find_first_ext(&eng_dir, "spec").is_some();
            if has_py {
                py_ev.push(format!("{}/ (Python 子引擎)", eng_rel));
            }
        }
    }
    if !py_ev.is_empty() {
        languages.push(DetectedLanguage {
            id: "python".into(),
            name: "Python".into(),
            confidence: if py_ev.iter().all(|e| e.contains("子引擎")) { 80 } else { 88 },
            evidence: py_ev,
        });
    }

    if root.join("go.mod").exists() {
        languages.push(DetectedLanguage {
            id: "go".into(),
            name: "Go".into(),
            confidence: 95,
            evidence: vec!["go.mod".into()],
        });
    }
    if let Some(cs) = find_first_ext(root, "csproj").or_else(|| find_first_ext(root, "sln")) {
        languages.push(DetectedLanguage {
            id: "dotnet".into(),
            name: "C# / .NET".into(),
            confidence: 92,
            evidence: vec![cs],
        });
    }
    if root.join("pubspec.yaml").exists() {
        languages.push(DetectedLanguage {
            id: "dart".into(),
            name: "Dart / Flutter".into(),
            confidence: 92,
            evidence: vec!["pubspec.yaml".into()],
        });
    }
    if root.join("CMakeLists.txt").exists() {
        languages.push(DetectedLanguage {
            id: "cpp".into(),
            name: "C / C++".into(),
            confidence: 90,
            evidence: vec!["CMakeLists.txt".into()],
        });
    }
    if root.join("pom.xml").exists()
        || root.join("build.gradle").exists()
        || root.join("build.gradle.kts").exists()
    {
        let mut ev = Vec::new();
        for f in ["pom.xml", "build.gradle", "build.gradle.kts"] {
            if root.join(f).exists() {
                ev.push(f.into());
            }
        }
        languages.push(DetectedLanguage {
            id: "java".into(),
            name: "Java / Kotlin".into(),
            confidence: 90,
            evidence: ev,
        });
    }

    // --- Shell / Packager signals ---
    if let Some(tc) = find_tauri_conf(root) {
        let rel = tc
            .strip_prefix(root)
            .unwrap_or(&tc)
            .to_string_lossy()
            .replace('\\', "/");
        shells.push(DetectedShell {
            id: "tauri".into(),
            name: "Tauri 桌面外壳".into(),
            kind: "tauri".into(),
            confidence: 98,
            output_formats: vec!["EXE".into(), "NSIS 安装包".into(), "MSI".into()],
            evidence: vec![rel],
            recommended: false,
        });
    }
    if root.join("package.json").exists() {
        let raw = fs::read_to_string(root.join("package.json")).unwrap_or_default();
        if raw.contains("\"electron\"") || raw.contains("electron-builder") || raw.contains("@electron-forge") {
            shells.push(DetectedShell {
                id: "electron".into(),
                name: "Electron 桌面外壳".into(),
                kind: "electron".into(),
                confidence: 95,
                output_formats: vec!["NSIS".into(), "便携目录".into(), "ASAR".into()],
                evidence: vec!["package.json (electron)".into()],
                recommended: false,
            });
        } else {
            shells.push(DetectedShell {
                id: "web".into(),
                name: "Node / Web 静态构建".into(),
                kind: "node".into(),
                confidence: 80,
                output_formats: vec!["dist/".into(), "ZIP".into(), "TAR.GZ".into()],
                evidence: vec!["package.json".into()],
                recommended: false,
            });
        }
    }
    if root.join("wails.json").exists() {
        shells.push(DetectedShell {
            id: "wails".into(),
            name: "Wails 桌面外壳".into(),
            kind: "wails".into(),
            confidence: 96,
            output_formats: vec!["EXE".into(), "NSIS".into()],
            evidence: vec!["wails.json".into()],
            recommended: false,
        });
    }
    if root.join("pubspec.yaml").exists() {
        shells.push(DetectedShell {
            id: "flutter".into(),
            name: "Flutter Desktop".into(),
            kind: "flutter".into(),
            confidence: 94,
            output_formats: vec!["Windows Release".into(), "ZIP".into()],
            evidence: vec!["pubspec.yaml".into()],
            recommended: false,
        });
    }
    if let Some(spec) = find_first_ext(root, "spec") {
        shells.push(DetectedShell {
            id: "pyinstaller".into(),
            name: "PyInstaller 独立打包".into(),
            kind: "python".into(),
            confidence: 92,
            output_formats: vec!["EXE".into(), "便携 ZIP".into()],
            evidence: vec![spec],
            recommended: false,
        });
    }
    if let Some(nsi) = find_first_ext(root, "nsi") {
        shells.push(DetectedShell {
            id: "nsis".into(),
            name: "NSIS 安装包脚本".into(),
            kind: "nsis".into(),
            confidence: 86,
            output_formats: vec!["Setup EXE".into()],
            evidence: vec![nsi],
            recommended: false,
        });
    }
    if let Some(iss) = find_first_ext(root, "iss") {
        shells.push(DetectedShell {
            id: "inno".into(),
            name: "Inno Setup 安装脚本".into(),
            kind: "nsis".into(),
            confidence: 86,
            output_formats: vec!["Setup EXE".into()],
            evidence: vec![iss],
            recommended: false,
        });
    }
    // Always offer Universal Native Portable Packager
    shells.push(DetectedShell {
        id: "portable".into(),
        name: "通用原生二进制 / 便携分包".into(),
        kind: languages
            .first()
            .map(|l| match l.id.as_str() {
                "dart" => "flutter",
                other => other,
            })
            .unwrap_or("custom")
            .into(),
        confidence: 65,
        output_formats: vec!["ZIP".into(), "TAR.GZ".into(), "分卷包".into()],
        evidence: vec!["内置零依赖压缩引擎".into()],
        recommended: false,
    });

    languages.sort_by(|a, b| b.confidence.cmp(&a.confidence));
    shells.sort_by(|a, b| b.confidence.cmp(&a.confidence));
    if let Some(first) = shells.first_mut() {
        first.recommended = true;
    }

    // Scan immediate child directories for hybrid / monorepo sub-modules
    if let Ok(rd) = fs::read_dir(root) {
        for e in rd.flatten().take(40) {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if should_skip_scan_dir(&name) {
                continue;
            }
            let (lang, sh) = if p.join("Cargo.toml").exists() {
                ("rust", "portable")
            } else if p.join("pyproject.toml").exists() || p.join("requirements.txt").exists() {
                ("python", "pyinstaller")
            } else if p.join("package.json").exists() {
                ("node", "web")
            } else if p.join("go.mod").exists() {
                ("go", "portable")
            } else if p.join("CMakeLists.txt").exists() {
                ("cpp", "portable")
            } else if p.join("pom.xml").exists() || p.join("build.gradle").exists() {
                ("java", "portable")
            } else {
                continue;
            };
            submodules.push(ProjectSubmodule {
                name: name.clone(),
                rel_path: name,
                language: lang.into(),
                shell: sh.into(),
            });
        }
    }

    let primary_language = languages
        .first()
        .map(|l| l.id.clone())
        .unwrap_or_else(|| "custom".into());
    let primary_shell = shells
        .first()
        .map(|s| s.id.clone())
        .unwrap_or_else(|| "portable".into());

    let supported_languages = vec![
        DetectedLanguage { id: "rust".into(), name: "Rust".into(), confidence: 0, evidence: vec!["Cargo.toml".into()] },
        DetectedLanguage { id: "node".into(), name: "Node.js / TS".into(), confidence: 0, evidence: vec!["package.json".into()] },
        DetectedLanguage { id: "python".into(), name: "Python".into(), confidence: 0, evidence: vec!["pyproject.toml".into()] },
        DetectedLanguage { id: "go".into(), name: "Go".into(), confidence: 0, evidence: vec!["go.mod".into()] },
        DetectedLanguage { id: "dotnet".into(), name: "C# / .NET".into(), confidence: 0, evidence: vec!["*.csproj".into()] },
        DetectedLanguage { id: "dart".into(), name: "Dart / Flutter".into(), confidence: 0, evidence: vec!["pubspec.yaml".into()] },
        DetectedLanguage { id: "cpp".into(), name: "C / C++".into(), confidence: 0, evidence: vec!["CMakeLists.txt".into()] },
        DetectedLanguage { id: "java".into(), name: "Java / Kotlin".into(), confidence: 0, evidence: vec!["pom.xml / gradle".into()] },
    ];
    let supported_shells = vec![
        DetectedShell { id: "tauri".into(), name: "Tauri 桌面外壳".into(), kind: "tauri".into(), confidence: 0, output_formats: vec!["EXE".into(), "NSIS".into()], evidence: vec![], recommended: false },
        DetectedShell { id: "electron".into(), name: "Electron 桌面外壳".into(), kind: "electron".into(), confidence: 0, output_formats: vec!["NSIS".into(), "便携包".into()], evidence: vec![], recommended: false },
        DetectedShell { id: "wails".into(), name: "Wails 桌面外壳".into(), kind: "wails".into(), confidence: 0, output_formats: vec!["EXE".into(), "NSIS".into()], evidence: vec![], recommended: false },
        DetectedShell { id: "flutter".into(), name: "Flutter Desktop".into(), kind: "flutter".into(), confidence: 0, output_formats: vec!["Windows Release".into()], evidence: vec![], recommended: false },
        DetectedShell { id: "pyinstaller".into(), name: "PyInstaller 打包".into(), kind: "python".into(), confidence: 0, output_formats: vec!["EXE".into()], evidence: vec![], recommended: false },
        DetectedShell { id: "web".into(), name: "Web / Node 构建".into(), kind: "node".into(), confidence: 0, output_formats: vec!["dist/".into(), "ZIP".into()], evidence: vec![], recommended: false },
        DetectedShell { id: "nsis".into(), name: "NSIS 安装脚本".into(), kind: "nsis".into(), confidence: 0, output_formats: vec!["Setup EXE".into()], evidence: vec![], recommended: false },
        DetectedShell { id: "inno".into(), name: "Inno Setup 脚本".into(), kind: "nsis".into(), confidence: 0, output_formats: vec!["Setup EXE".into()], evidence: vec![], recommended: false },
        DetectedShell { id: "portable".into(), name: "原生二进制 / 便携分包".into(), kind: "custom".into(), confidence: 0, output_formats: vec!["ZIP".into(), "TAR.GZ".into(), "分卷".into()], evidence: vec![], recommended: false },
    ];

    ProjectProfileMatrix {
        project_id: project_id.to_string(),
        primary_language,
        primary_shell,
        languages,
        shells,
        submodules,
        supported_languages,
        supported_shells,
    }
}

/// Apply user-chosen language & shell override while preserving custom actions.
pub fn apply_profile_override(
    proj: &mut ProjectConfig,
    language: &str,
    shell: &str,
) {
    proj.primary_language = language.trim().to_string();
    proj.shell_type = shell.trim().to_string();
    proj.kind = match shell.trim() {
        "tauri" => "tauri",
        "electron" => "electron",
        "wails" => "wails",
        "flutter" => "flutter",
        "pyinstaller" => "python",
        "web" => "node",
        "nsis" | "inno" => "nsis",
        _ => match language.trim() {
            "rust" => "rust",
            "node" => "node",
            "python" => "python",
            "go" => "go",
            "dotnet" => "dotnet",
            "dart" => "flutter",
            "cpp" => "cpp",
            "java" => "java",
            _ => "custom",
        },
    }
    .to_string();
    proj.adapter_label = format!("{} · {}", proj.primary_language.to_uppercase(), proj.shell_type);
}

pub fn find_makensis_path() -> Option<String> {
    crate::toolchain::find_makensis_native()
}

fn should_skip_scan_dir(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with('.')
        || matches!(
            lower.as_str(),
            "node_modules"
                | "target"
                | "dist"
                | "build"
                | "release"
                | "releases"
                | "out"
                | "bin"
                | "obj"
                | "venv"
                | "__pycache__"
                | "vendor"
                | "tools"
                | "cache"
                | "logs"
                | "tmp"
                | "temp"
                | "windows"
                | "program files"
                | "program files (x86)"
                | "appdata"
        )
}

/// Fast signature check determining whether a directory is a buildable software project root.
pub fn is_project_dir(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    if find_tauri_conf(dir).is_some()
        || dir.join("Cargo.toml").exists()
        || dir.join("package.json").exists()
        || dir.join("pyproject.toml").exists()
        || dir.join("go.mod").exists()
        || dir.join("pubspec.yaml").exists()
        || dir.join("wails.json").exists()
        || dir.join("CMakeLists.txt").exists()
        || dir.join("pom.xml").exists()
        || dir.join("build.gradle").exists()
        || dir.join("build.gradle.kts").exists()
        || dir.join("build.ps1").exists()
    {
        return true;
    }
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() {
                if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                    if ext.eq_ignore_ascii_case("csproj")
                        || ext.eq_ignore_ascii_case("sln")
                        || ext.eq_ignore_ascii_case("nsi")
                        || ext.eq_ignore_ascii_case("iss")
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Recursively discovers software projects under `roots` up to `max_depth` (0 = roots only, 1 = immediate children, 2 = grandchildren).
/// Stops descending into an already-recognized project directory so monorepo sub-crates don't spam duplicates.
pub fn discover_projects_in_roots(
    roots: &[PathBuf],
    existing_roots: &[String],
    max_depth: usize,
) -> Vec<DiscoveredProject> {
    let existing_keys: HashSet<String> = existing_roots
        .iter()
        .map(|s| {
            s.trim()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_ascii_lowercase()
        })
        .collect();
    let mut visited: HashSet<String> = HashSet::new();
    let mut results: Vec<DiscoveredProject> = Vec::new();

    fn walk(
        dir: &Path,
        depth: usize,
        max_depth: usize,
        existing_keys: &HashSet<String>,
        visited: &mut HashSet<String>,
        out: &mut Vec<DiscoveredProject>,
    ) {
        if out.len() >= 40 || visited.len() >= 400 || !dir.is_dir() {
            return;
        }
        let canon = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let raw_str = canon.to_string_lossy().to_string();
        let display_str = raw_str.strip_prefix(r"\\?\").unwrap_or(&raw_str).to_string();
        let key = display_str
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase();
        if !visited.insert(key.clone()) {
            return;
        }

        if is_project_dir(dir) {
            if let Ok(cfg) = detect_project(dir) {
                let ver_state = version::read_project_versions(dir, &cfg.version_files);
                out.push(DiscoveredProject {
                    path: cfg.root.clone(),
                    name: cfg.name,
                    kind: cfg.kind,
                    adapter_label: cfg.adapter_label,
                    version: ver_state.current,
                    github_repo: cfg.github_repo,
                    already_imported: existing_keys.contains(&key),
                });
            }
            // Do not descend into sub-crates / nested web folders of an identified project
            return;
        }

        if depth >= max_depth {
            return;
        }

        if let Ok(entries) = fs::read_dir(dir) {
            let mut subdirs: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .and_then(|s| s.to_str())
                            .map(|n| !should_skip_scan_dir(n))
                            .unwrap_or(false)
                })
                .collect();
            subdirs.sort();
            for sub in subdirs {
                walk(&sub, depth + 1, max_depth, existing_keys, visited, out);
            }
        }
    }

    for r in roots {
        walk(r, 0, max_depth, &existing_keys, &mut visited, &mut results);
    }
    results.sort_by(|a, b| a.already_imported.cmp(&b.already_imported).then_with(|| a.name.cmp(&b.name)));
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_owner_repo() {
        assert_eq!(
            parse_github_owner_repo("https://github.com/acme-dev/demo-app.git"),
            "acme-dev/demo-app"
        );
        assert_eq!(
            parse_github_owner_repo("git@github.com:foo-bar/my.app.git"),
            "foo-bar/my.app"
        );
        assert_eq!(
            parse_github_owner_repo("https://github.com/acme-dev/demo-app/releases/latest/download/latest.json"),
            "acme-dev/demo-app"
        );
        assert_eq!(parse_github_owner_repo("https://gitlab.com/a/b.git"), "");
    }

    #[test]
    fn test_build_ps1_introspection_avoids_phantom_switches() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_ps1_introspect_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(2)
        ));
        // Project A: minimal build.ps1 with only -Bump and -TestsOnly
        let proj_a = base.join("app-minimal");
        fs::create_dir_all(proj_a.join("src-tauri")).unwrap();
        fs::write(proj_a.join("src-tauri/tauri.conf.json"), r#"{"productName":"MinimalApp","version":"1.0.0"}"#).unwrap();
        fs::write(
            proj_a.join("build.ps1"),
            "param([string]$Bump = 'none', [switch]$TestsOnly)\nWrite-Host 'ok'\n",
        )
        .unwrap();
        let cfg_a = detect_project(&proj_a).unwrap();
        assert!(cfg_a.actions.iter().any(|a| a.id == "build-full"));
        assert!(cfg_a.actions.iter().any(|a| a.id == "test-gate"));
        assert!(!cfg_a.actions.iter().any(|a| a.id == "build-status"));
        assert!(!cfg_a.actions.iter().any(|a| a.id == "pack-portable"));

        // Project B: full pipeline build.ps1 with -Status, -Portable, -Engine and lockfile path
        let proj_b = base.join("app-pipeline");
        fs::create_dir_all(proj_b.join("src/src-tauri")).unwrap();
        fs::create_dir_all(proj_b.join("src/engine")).unwrap();
        fs::create_dir_all(proj_b.join("引擎")).unwrap();
        fs::create_dir_all(proj_b.join("scripts")).unwrap();
        fs::write(
            proj_b.join("src/src-tauri/tauri.conf.json"),
            r#"{"productName":"PipeApp","version":"2.3.0","plugins":{"updater":{"endpoints":["https://github.com/acme/pipe-app/releases/latest/download/latest.json"]}}}"#,
        )
        .unwrap();
        fs::write(
            proj_b.join("build.ps1"),
            "param([string]$Bump='none',[switch]$Status,[switch]$Portable,[switch]$Engine,[switch]$TestsOnly)\n",
        )
        .unwrap();
        fs::write(
            proj_b.join("scripts/build-manager.ps1"),
            "$LockFile = 'D:\\Builds\\.locks\\pipeapp.build.lock'\n",
        )
        .unwrap();
        let cfg_b = detect_project(&proj_b).unwrap();
        assert!(cfg_b.actions.iter().any(|a| a.id == "build-status"));
        assert!(cfg_b.actions.iter().any(|a| a.id == "pack-portable"));
        assert!(cfg_b.actions.iter().any(|a| a.id == "build-engine"));
        assert_eq!(cfg_b.lock_file, r"D:\Builds\.locks\pipeapp.build.lock");
        assert_eq!(cfg_b.github_repo, "acme/pipe-app");

        // Project C: Cargo.toml dependency git URL must NEVER be misidentified as the project's own repo!
        let proj_c = base.join("app-dep-trap");
        fs::create_dir_all(&proj_c).unwrap();
        fs::write(
            proj_c.join("Cargo.toml"),
            "[package]\nname=\"dep-trap\"\nversion=\"0.1.0\"\n\n[dependencies]\nsome_dep = { git = \"https://github.com/thirdparty/unrelated\" }\n",
        )
        .unwrap();
        assert_eq!(detect_github_repo(&proj_c), "");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_discover_projects_in_roots_skips_ignored_dirs() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_discover_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(3)
        ));
        fs::create_dir_all(base.join("app-rust")).unwrap();
        fs::write(base.join("app-rust/Cargo.toml"), "[package]\nname=\"demo\"\nversion=\"0.2.0\"\n").unwrap();
        // Nested sub-crate inside app-rust must NOT be discovered separately
        fs::create_dir_all(base.join("app-rust/subcrate")).unwrap();
        fs::write(base.join("app-rust/subcrate/Cargo.toml"), "[package]\nname=\"sub\"\nversion=\"0.1.0\"\n").unwrap();
        // Ignored node_modules must be skipped
        fs::create_dir_all(base.join("node_modules/fake-pkg")).unwrap();
        fs::write(base.join("node_modules/fake-pkg/package.json"), r#"{"name":"fake","version":"1.0.0"}"#).unwrap();

        let found = discover_projects_in_roots(&[base.clone()], &[], 2);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "rust");
        assert_eq!(found[0].version, "0.2.0");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_detect_multiple_formats_in_tempdir() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_det_test_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(1)
        ));
        let go_dir = base.join("demo-go");
        fs::create_dir_all(&go_dir).unwrap();
        fs::write(go_dir.join("go.mod"), "module example.com/demo\n\ngo 1.22\n").unwrap();
        assert_eq!(detect_project(&go_dir).unwrap().kind, "go");

        let electron_dir = base.join("demo-electron");
        fs::create_dir_all(&electron_dir).unwrap();
        fs::write(
            electron_dir.join("package.json"),
            r#"{"name":"my-elec","version":"2.1.0","devDependencies":{"electron":"^30.0.0"},"scripts":{"build":"vite build","dist":"electron-builder"}}"#,
        )
        .unwrap();
        let ecfg = detect_project(&electron_dir).unwrap();
        assert_eq!(ecfg.kind, "electron");
        assert!(ecfg.actions.iter().any(|a| a.id == "script-dist"));

        let py_dir = base.join("demo-py");
        fs::create_dir_all(&py_dir).unwrap();
        fs::write(py_dir.join("pyproject.toml"), "[project]\nname=\"app\"\nversion=\"0.4.0\"\n").unwrap();
        assert_eq!(detect_project(&py_dir).unwrap().kind, "python");

        let dotnet_dir = base.join("demo-net");
        fs::create_dir_all(&dotnet_dir).unwrap();
        fs::write(dotnet_dir.join("Demo.csproj"), "<Project><PropertyGroup><Version>3.1.4</Version></PropertyGroup></Project>").unwrap();
        assert_eq!(detect_project(&dotnet_dir).unwrap().kind, "dotnet");

        let fl_dir = base.join("demo-flutter");
        fs::create_dir_all(&fl_dir).unwrap();
        fs::write(fl_dir.join("pubspec.yaml"), "name: demo\nversion: 1.5.2+9\n").unwrap();
        assert_eq!(detect_project(&fl_dir).unwrap().kind, "flutter");

        let rust_dir = base.join("demo-rust");
        fs::create_dir_all(&rust_dir).unwrap();
        fs::write(rust_dir.join("Cargo.toml"), "[package]\nname=\"cli\"\nversion=\"0.9.1\"\n").unwrap();
        assert_eq!(detect_project(&rust_dir).unwrap().kind, "rust");

        let nsis_dir = base.join("demo-nsis");
        fs::create_dir_all(&nsis_dir).unwrap();
        fs::write(nsis_dir.join("installer.nsi"), "Name \"Demo\"\n").unwrap();
        assert_eq!(detect_project(&nsis_dir).unwrap().kind, "nsis");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_detect_join_path_lock_journal_and_hybrid_package_profiles() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_det_mgr_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(61)
        ));
        fs::create_dir_all(base.join("src/src-tauri")).unwrap();
        fs::create_dir_all(base.join("tools")).unwrap();
        fs::write(base.join("src/src-tauri/tauri.conf.json"), r#"{"productName":"DemoApp","version":"1.0.0"}"#).unwrap();
        fs::write(base.join("build.ps1"), "param([switch]$AppOnly, [switch]$Clean, [switch]$TestsOnly)\n").unwrap();
        fs::write(
            base.join("tools/build-manager.ps1"),
            "$script:LockDir = 'D:\\Builds\\.locks'\n$script:LockPath = Join-Path $script:LockDir 'demoapp.build.lock'\n$script:JournalPath = 'D:\\Builds\\demoapp-builds.jsonl'\n",
        )
        .unwrap();

        let cfg = detect_project(&base).unwrap();
        assert_eq!(cfg.lock_file, r"D:\Builds\.locks\demoapp.build.lock");
        assert_eq!(cfg.journal_file, r"D:\Builds\demoapp-builds.jsonl");

        let profs = default_package_profiles_for_project("DemoApp", "demo.exe", r"引擎\demo_engine.exe");
        let core = profs.iter().find(|p| p.id == "portable-core").unwrap();
        assert!(core.sources.iter().any(|s| s == "引擎"), "must include engine top dir: {:?}", core.sources);
        assert!(core.sources.iter().any(|s| s == "THIRD_PARTY.md"));

        let _ = fs::remove_dir_all(&base);
    }
}
