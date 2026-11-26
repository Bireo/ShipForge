use crate::detector::{detect_project, discover_projects_in_roots};
use crate::models::{AppConfig, ProjectConfig};
use crate::storage;
use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const CURRENT_CONFIG_VERSION: u32 = 4;

pub fn app_data_dir() -> PathBuf {
    storage::active_data_root()
}

pub fn config_path() -> PathBuf {
    storage::config_file_path()
}

pub fn journal_path() -> PathBuf {
    storage::journal_file_path()
}

fn is_safe_auto_discovery_dir(dir: &Path) -> bool {
    if !dir.is_dir() || dir.parent().is_none() {
        return false;
    }
    let lower = dir
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    if lower.contains("program files")
        || lower.contains("\\windows")
        || lower.contains("\\appdata")
        || lower.ends_with("\\desktop")
        || lower.ends_with("\\downloads")
        || lower.ends_with("\\documents")
        || lower.ends_with("\\users")
    {
        return false;
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let prof_norm = profile
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase();
        if lower == prof_norm {
            return false;
        }
    }
    // Never auto-scan huge directories with >60 top-level entries on cold startup
    if let Ok(rd) = fs::read_dir(dir) {
        if rd.take(61).count() > 60 {
            return false;
        }
    }
    true
}

pub fn load_config() -> AppConfig {
    let path = config_path();
    let mut cfg = if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<AppConfig>(s.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
    } else {
        AppConfig {
            auto_watch: true,
            ..Default::default()
        }
    };

    if cfg.default_port == 0 {
        cfg.default_port = 18765;
    }

    let mut dirty = false;

    if cfg.projects.is_empty() {
        // Universal zero-hardcode auto-discovery: inspect configured scan_roots, current working dir,
        // and the executable's sibling workspace (only if not installed inside Program Files / system dirs).
        let mut discovery_roots: Vec<PathBuf> = cfg
            .scan_roots
            .iter()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .collect();

        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                if is_safe_auto_discovery_dir(dir) {
                    discovery_roots.push(dir.to_path_buf());
                    if let Some(parent) = dir.parent() {
                        if is_safe_auto_discovery_dir(parent) {
                            discovery_roots.push(parent.to_path_buf());
                        }
                    }
                }
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            if is_safe_auto_discovery_dir(&cwd) && !discovery_roots.iter().any(|p| p == &cwd) {
                discovery_roots.push(cwd);
            }
        }

        let discovered = discover_projects_in_roots(&discovery_roots, &[], 1);
        for item in discovered.into_iter().take(12) {
            if let Ok(proj) = detect_project(Path::new(&item.path)) {
                if !cfg
                    .projects
                    .iter()
                    .any(|p| p.root.eq_ignore_ascii_case(&proj.root))
                {
                    cfg.projects.push(proj);
                }
            }
        }
        if let Some(first) = cfg.projects.first() {
            cfg.active_project_id = Some(first.id.clone());
        }
        cfg.config_version = CURRENT_CONFIG_VERSION;
        dirty = true;
    } else if cfg.config_version < CURRENT_CONFIG_VERSION {
        for proj in &mut cfg.projects {
            let root = Path::new(&proj.root);
            if root.is_dir() {
                if let Ok(fresh) = detect_project(root) {
                    let custom_actions: Vec<_> = proj
                        .actions
                        .iter()
                        .filter(|a| a.id.starts_with("custom-"))
                        .cloned()
                        .collect();
                    let mut merged = fresh.actions;
                    merged.extend(custom_actions);
                    proj.actions = merged;
                    proj.adapter_label = fresh.adapter_label;
                    proj.has_build_ps1 = fresh.has_build_ps1;
                    proj.has_publish_ps1 = fresh.has_publish_ps1;
                    if proj.primary_language.is_empty() {
                        proj.primary_language = fresh.primary_language;
                    }
                    if proj.detected_languages.is_empty() {
                        proj.detected_languages = fresh.detected_languages;
                    }
                    if proj.shell_type.is_empty() {
                        proj.shell_type = fresh.shell_type;
                    }
                    if proj.repo_binding.repo_slug.is_empty() {
                        if !proj.github_repo.is_empty() {
                            proj.repo_binding.provider = "github".into();
                            proj.repo_binding.repo_slug = proj.github_repo.clone();
                        } else {
                            proj.repo_binding = fresh.repo_binding;
                        }
                    }
                    if proj.package_profiles.is_empty() {
                        proj.package_profiles = fresh.package_profiles;
                    }
                }
            }
        }
        cfg.config_version = CURRENT_CONFIG_VERSION;
        dirty = true;
    }

    if cfg.active_project_id.is_none() {
        cfg.active_project_id = cfg.projects.first().map(|p| p.id.clone());
    }

    if dirty {
        let _ = save_config(&cfg);
    }

    cfg
}

pub fn save_config(cfg: &AppConfig) -> Result<()> {
    let path = config_path();
    let data = serde_json::to_string_pretty(cfg).context("序列化配置失败")?;
    crate::guardrails::atomic_write_bytes(&path, data.as_bytes())?;
    Ok(())
}

pub fn append_journal(record: &serde_json::Value) {
    let dir = storage::config_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = journal_path();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        if let Ok(line) = serde_json::to_string(record) {
            let _ = writeln!(f, "{}", line);
        }
    }
}

pub fn resolve_release_dir(project: &ProjectConfig) -> PathBuf {
    let root = Path::new(&project.root);
    let rel = if project.release_dir.trim().is_empty() {
        "release"
    } else {
        project.release_dir.trim()
    };
    // Confine release_dir strictly inside project root even if config.json was externally edited
    crate::guardrails::resolve_confined_path(root, rel).unwrap_or_else(|_| root.join("release"))
}
