use crate::build_guard;
use crate::config;
use crate::guardrails::{atomic_write_bytes, retry_on_win_lock};
use crate::models::{
    CrashRecoveryResult, ProjectConfig, RollbackCheckItem, RollbackVerification,
};
use crate::packager::sha256_file;
use crate::storage;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileBackupRecord {
    abs_path: String,
    existed: bool,
    original_sha256: String,
    backup_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitBaseline {
    is_repo: bool,
    head_sha: String,
    local_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseFileBaseline {
    name: String,
    size: u64,
    backup_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionSnapshot {
    pub tx_id: String,
    pub pid: u32,
    pub project_id: String,
    pub project_root: String,
    pub release_dir: String,
    pub lock_file: String,
    pub operation: String,
    pub started_at: String,
    #[serde(default)]
    pub remote_pushed: bool,
    /// Set once the build/packaging step completes & passes verification; subsequent network/publish
    /// failures MUST NOT delete verified local release artifacts!
    #[serde(default)]
    pub build_verified: bool,
    file_backups: Vec<FileBackupRecord>,
    #[serde(default)]
    bucket_backups: Vec<FileBackupRecord>,
    git_baseline: Option<GitBaseline>,
    release_baseline: Vec<ReleaseFileBaseline>,
    tx_backup_dir: String,
    tx_meta_file: String,
}

#[cfg(windows)]
fn is_pid_alive_os(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    if pid == 0 {
        return false;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(h, &mut code);
        CloseHandle(h);
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

#[cfg(not(windows))]
fn is_pid_alive_os(pid: u32) -> bool {
    pid == std::process::id()
}

impl TransactionSnapshot {
    /// Mark that commits/tags have already been pushed to remote; rollback must NOT rewrite pushed history
    /// nor delete verified release artifacts.
    pub fn mark_remote_pushed(&mut self) {
        self.remote_pushed = true;
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = atomic_write_bytes(Path::new(&self.tx_meta_file), &json);
        }
    }

    /// Mark that the build/packaging stage completed & verified cleanly; later network/publish failures
    /// preserve verified local artifacts & version state so the user can retry publishing without recompiling.
    pub fn mark_build_verified(&mut self) {
        self.build_verified = true;
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = atomic_write_bytes(Path::new(&self.tx_meta_file), &json);
        }
    }
}

fn hidden_git(root: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let out = cmd.output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn sha256_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    format!("{:x}", h.finalize())
}

fn capture_git_baseline(root: &Path) -> Option<GitBaseline> {
    let is_repo = hidden_git(root, &["rev-parse", "--is-inside-work-tree"])
        .map(|s| s == "true")
        .unwrap_or(false);
    if !is_repo {
        return None;
    }
    let head_sha = hidden_git(root, &["rev-parse", "HEAD"]).unwrap_or_default();
    let local_tags = hidden_git(root, &["tag", "-l"])
        .map(|s| {
            s.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Some(GitBaseline {
        is_repo: true,
        head_sha,
        local_tags,
    })
}

/// Begin a crash-recoverable transaction snapshot prior to any mutating operation.
pub fn begin_transaction(project: &ProjectConfig, operation: &str) -> Result<TransactionSnapshot> {
    begin_transaction_in(project, operation, &storage::tx_dir())
}

pub fn begin_transaction_in(
    project: &ProjectConfig,
    operation: &str,
    base_tx_dir: &Path,
) -> Result<TransactionSnapshot> {
    fs::create_dir_all(base_tx_dir)?;
    let stamp = chrono::Local::now().timestamp_nanos_opt().unwrap_or(0);
    let tx_id = format!("tx-{}-{}", std::process::id(), stamp);
    let backup_dir = base_tx_dir.join(&tx_id);
    fs::create_dir_all(&backup_dir)?;

    let root = PathBuf::from(&project.root);
    let release_dir = config::resolve_release_dir(project);

    let mut targets: Vec<PathBuf> = Vec::new();
    for vf in &project.version_files {
        let p = root.join(&vf.path);
        if !targets.iter().any(|x| x == &p) {
            targets.push(p);
        }
    }
    for lock_p in crate::version::cargo_lock_paths_for_specs(&root, &project.version_files) {
        if !targets.iter().any(|x| x == &lock_p) {
            targets.push(lock_p);
        }
    }
    if !project.changelog_path.trim().is_empty() {
        let p = root.join(project.changelog_path.trim());
        if !targets.iter().any(|x| x == &p) {
            targets.push(p);
        }
    }

    let mut file_backups = Vec::new();
    for (idx, p) in targets.into_iter().enumerate() {
        if p.exists() && p.is_file() {
            if let Ok(bytes) = fs::read(&p) {
                let hash = sha256_bytes(&bytes);
                let bak = backup_dir.join(format!("file_{}.bak", idx));
                fs::write(&bak, &bytes)?;
                file_backups.push(FileBackupRecord {
                    abs_path: p.to_string_lossy().to_string(),
                    existed: true,
                    original_sha256: hash,
                    backup_file: bak.to_string_lossy().to_string(),
                });
            }
        } else {
            file_backups.push(FileBackupRecord {
                abs_path: p.to_string_lossy().to_string(),
                existed: false,
                original_sha256: String::new(),
                backup_file: String::new(),
            });
        }
    }

    let git_baseline = capture_git_baseline(&root);

    let mut release_baseline = Vec::new();
    if release_dir.is_dir() {
        if let Ok(rd) = fs::read_dir(&release_dir) {
            for (idx, entry) in rd.flatten().enumerate() {
                let p = entry.path();
                if !p.is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                // Backup small metadata/manifest files (<= 2 MiB) so overwritten manifests restore cleanly
                let mut bak_str = String::new();
                let lower = name.to_ascii_lowercase();
                if size <= 2 * 1024 * 1024
                    && (lower == "latest.json"
                        || lower == "sha256sums.txt"
                        || lower.ends_with(".split.json"))
                {
                    let bak = backup_dir.join(format!("rel_{}.bak", idx));
                    if fs::copy(&p, &bak).is_ok() {
                        bak_str = bak.to_string_lossy().to_string();
                    }
                }
                release_baseline.push(ReleaseFileBaseline {
                    name,
                    size,
                    backup_file: bak_str,
                });
            }
        }
    }

    // Backup project version isolation buckets so rolled-back builds never leave phantom buildCount
    let mut bucket_backups = Vec::new();
    let clean_pid: String = project
        .id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let bdir = storage::workspaces_dir().join(if clean_pid.is_empty() { "default" } else { &clean_pid });
    if bdir.is_dir() {
        if let Ok(rd) = fs::read_dir(&bdir) {
            for (idx, e) in rd.flatten().enumerate() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("json") && p.is_file() {
                    if let Ok(bytes) = fs::read(&p) {
                        let hash = sha256_bytes(&bytes);
                        let bak = backup_dir.join(format!("bucket_{}.bak", idx));
                        if fs::write(&bak, &bytes).is_ok() {
                            bucket_backups.push(FileBackupRecord {
                                abs_path: p.to_string_lossy().to_string(),
                                existed: true,
                                original_sha256: hash,
                                backup_file: bak.to_string_lossy().to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    let meta_file = base_tx_dir.join(format!("{}.json", tx_id));
    let snap = TransactionSnapshot {
        tx_id: tx_id.clone(),
        pid: std::process::id(),
        project_id: project.id.clone(),
        project_root: project.root.clone(),
        release_dir: release_dir.to_string_lossy().to_string(),
        lock_file: project.lock_file.clone(),
        operation: operation.to_string(),
        started_at: chrono::Local::now().to_rfc3339(),
        remote_pushed: false,
        build_verified: false,
        file_backups,
        bucket_backups,
        git_baseline,
        release_baseline,
        tx_backup_dir: backup_dir.to_string_lossy().to_string(),
        tx_meta_file: meta_file.to_string_lossy().to_string(),
    };

    let json = serde_json::to_vec_pretty(&snap)?;
    atomic_write_bytes(&meta_file, &json)?;
    Ok(snap)
}

/// Finalize a successful transaction and delete all temporary snapshot data immediately.
pub fn commit_transaction(snap: &TransactionSnapshot) {
    let _ = fs::remove_file(&snap.tx_meta_file);
    let _ = fs::remove_dir_all(&snap.tx_backup_dir);
}

/// Automatically rollback all mutations performed during the transaction and run SHA-256/Git verification.
pub fn rollback_and_verify(
    snap: &TransactionSnapshot,
    triggered_by: &str,
) -> RollbackVerification {
    let started = Instant::now();
    let mut checks: Vec<RollbackCheckItem> = Vec::new();
    let mut all_restored = true;

    // 1. Restore version files & changelog byte-for-byte + verify SHA-256
    // (Preserve version files if commits were pushed to remote OR if the build already succeeded & was verified,
    // so a transient upload/network failure NEVER forces a full 3-minute rebuild!)
    if snap.remote_pushed || snap.build_verified {
        checks.push(RollbackCheckItem {
            item: "版本与配置文件".into(),
            restored: true,
            detail: if snap.remote_pushed {
                "已推送至远端，保留与远端提交一致的版本状态（避免本地回退产生脏变更）".into()
            } else {
                "编译与本地验收已成功，保留当前版本配置以便直接重试发布（无需重新编译）".into()
            },
        });
    } else {
        for fb in &snap.file_backups {
            let target = Path::new(&fb.abs_path);
            let label = target
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&fb.abs_path)
                .to_string();

            if fb.existed {
                let bak = Path::new(&fb.backup_file);
                match fs::read(bak) {
                    Ok(orig_bytes) => {
                        let cur_hash = sha256_file(target).unwrap_or_default();
                        if cur_hash == fb.original_sha256 {
                            checks.push(RollbackCheckItem {
                                item: format!("文件 {}", label),
                                restored: true,
                                detail: "未发生变更，哈希一致".into(),
                            });
                        } else {
                            match atomic_write_bytes(target, &orig_bytes) {
                                Ok(()) => {
                                    let verified = sha256_file(target)
                                        .map(|h| h == fb.original_sha256)
                                        .unwrap_or(false);
                                    if !verified {
                                        all_restored = false;
                                    }
                                    checks.push(RollbackCheckItem {
                                        item: format!("文件 {}", label),
                                        restored: verified,
                                        detail: if verified {
                                            "已字节级还原并通过 SHA-256 复核".into()
                                        } else {
                                            "还原后哈希校验不一致".into()
                                        },
                                    });
                                }
                                Err(e) => {
                                    all_restored = false;
                                    checks.push(RollbackCheckItem {
                                        item: format!("文件 {}", label),
                                        restored: false,
                                        detail: format!("还原失败：{}", e),
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => {
                        all_restored = false;
                        checks.push(RollbackCheckItem {
                            item: format!("文件 {}", label),
                            restored: false,
                            detail: format!("读取事务备份失败：{}", e),
                        });
                    }
                }
            } else if target.exists() {
                let rm_ok = retry_on_win_lock(|| fs::remove_file(target)).is_ok();
                if !rm_ok {
                    all_restored = false;
                }
                checks.push(RollbackCheckItem {
                    item: format!("新建文件 {}", label),
                    restored: rm_ok,
                    detail: if rm_ok {
                        "已移除本次新建文件".into()
                    } else {
                        "移除失败，可能仍被占用".into()
                    },
                });
            }
        }

        // 1b. Restore version isolation buckets so buildCount doesn't drift on rollback
        for bb in &snap.bucket_backups {
            let target = Path::new(&bb.abs_path);
            let bak = Path::new(&bb.backup_file);
            if let Ok(orig) = fs::read(bak) {
                let _ = atomic_write_bytes(target, &orig);
            }
        }
    }

    // 2. Git surgical rollback (NEVER --hard, NEVER rewrite pushed commits/tags,
    // and NEVER split-brain a verified build's local commit/tag from its preserved version & release/)
    if let Some(gb) = &snap.git_baseline {
        let root = Path::new(&snap.project_root);
        if gb.is_repo && root.is_dir() {
            if snap.remote_pushed || snap.build_verified {
                checks.push(RollbackCheckItem {
                    item: if snap.remote_pushed {
                        "Git 远端历史保护".into()
                    } else {
                        "Git 本地构建点保护".into()
                    },
                    restored: true,
                    detail: if snap.remote_pushed {
                        "本次提交与标签已推送至远端，保守保留已推送历史以避免分支分叉".into()
                    } else {
                        "编译与产物验收已成功，同步保留本地版本提交与标签（避免与已保留的版本文件/产物裂脑）".into()
                    },
                });
            } else {
                // 2a. Remove newly created local tags only
                let cur_tags: Vec<String> = hidden_git(root, &["tag", "-l"])
                    .map(|s| {
                        s.lines()
                            .map(|l| l.trim().to_string())
                            .filter(|l| !l.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                let base_tag_set: HashSet<&str> = gb.local_tags.iter().map(|s| s.as_str()).collect();
                let mut removed_tags = Vec::new();
                for tag in cur_tags {
                    if !base_tag_set.contains(tag.as_str()) {
                        if hidden_git(root, &["tag", "-d", "--", &tag]).is_some() {
                            removed_tags.push(tag);
                        } else {
                            all_restored = false;
                            checks.push(RollbackCheckItem {
                                item: format!("Git Tag {}", tag),
                                restored: false,
                                detail: "删除事务新建标签失败".into(),
                            });
                        }
                    }
                }
                if !removed_tags.is_empty() {
                    checks.push(RollbackCheckItem {
                        item: "Git 本地标签".into(),
                        restored: true,
                        detail: format!("已撤回新建标签：{}", removed_tags.join(", ")),
                    });
                }

                // 2b. Reset local commits created ONLY by this transaction via --mixed (preserves user edits)
                if !gb.head_sha.is_empty() {
                    let cur_head = hidden_git(root, &["rev-parse", "HEAD"]).unwrap_or_default();
                    if !cur_head.is_empty() && cur_head != gb.head_sha {
                        let is_ancestor = hidden_git(
                            root,
                            &["merge-base", "--is-ancestor", &gb.head_sha, "HEAD"],
                        )
                        .is_some();
                        if is_ancestor {
                            let reset_ok =
                                hidden_git(root, &["reset", "--mixed", &gb.head_sha]).is_some();
                            let final_head =
                                hidden_git(root, &["rev-parse", "HEAD"]).unwrap_or_default();
                            let matched = reset_ok && final_head == gb.head_sha;
                            if !matched {
                                all_restored = false;
                            }
                            checks.push(RollbackCheckItem {
                                item: "Git HEAD 提交".into(),
                                restored: matched,
                                detail: if matched {
                                    format!(
                                        "已安全回退事务内提交至 {}（保留工作区原修改）",
                                        &gb.head_sha[..gb.head_sha.len().min(8)]
                                    )
                                } else {
                                    "撤回事务内本地提交失败".into()
                                },
                            });
                        } else {
                            checks.push(RollbackCheckItem {
                                item: "Git HEAD 提交".into(),
                                restored: true,
                                detail: "检测到非线性分支变动，已保守跳过自动 reset".into(),
                            });
                        }
                    } else {
                        checks.push(RollbackCheckItem {
                            item: "Git HEAD 提交".into(),
                            restored: true,
                            detail: format!(
                                "HEAD 保持初始基线 {}",
                                &gb.head_sha[..gb.head_sha.len().min(8)]
                            ),
                        });
                    }
                }
            }
        }
    }

    // 3. Release directory cleanup (purge partial/new files ONLY if build was NOT verified and NOT pushed!)
    let rel_dir = Path::new(&snap.release_dir);
    if rel_dir.is_dir() {
        if snap.remote_pushed || snap.build_verified {
            checks.push(RollbackCheckItem {
                item: "产物目录 release/（已验收成品保护）".into(),
                restored: true,
                detail: "编译与产物验收已成功，已安全保留 release/ 成品与清单，可直接前往「产物与仓库」重试上传（无需重新编译）".into(),
            });
        } else {
            let base_map: HashMap<&str, &ReleaseFileBaseline> = snap
                .release_baseline
                .iter()
                .map(|r| (r.name.as_str(), r))
                .collect();
            let mut removed_partials = 0usize;
            let mut remove_errors = 0usize;

            if let Ok(rd) = fs::read_dir(rel_dir) {
                for entry in rd.flatten() {
                    let p = entry.path();
                    if !p.is_file() {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !base_map.contains_key(name.as_str()) {
                        if retry_on_win_lock(|| fs::remove_file(&p)).is_ok() {
                            removed_partials += 1;
                        } else {
                            remove_errors += 1;
                            all_restored = false;
                        }
                    }
                }
            }

            let mut restored_manifests = 0usize;
            for base in &snap.release_baseline {
                if !base.backup_file.is_empty() {
                    let bak = Path::new(&base.backup_file);
                    if let Ok(bytes) = fs::read(bak) {
                        let dst = rel_dir.join(&base.name);
                        if atomic_write_bytes(&dst, &bytes).is_ok() {
                            restored_manifests += 1;
                        } else {
                            all_restored = false;
                        }
                    }
                }
            }

            checks.push(RollbackCheckItem {
                item: "产物目录 release/".into(),
                restored: remove_errors == 0,
                detail: if remove_errors == 0 {
                    format!(
                        "已清理 {} 个半成品/新增产物，恢复 {} 个原有清单",
                        removed_partials, restored_manifests
                    )
                } else {
                    format!(
                        "清理了 {} 个产物，但有 {} 个文件仍被外部占用",
                        removed_partials, remove_errors
                    )
                },
            });
        }
    }

    // 4. Clean stale external build lock if dead
    if !snap.lock_file.trim().is_empty() {
        if let Some(lock) = build_guard::inspect_lock(&snap.lock_file) {
            if lock.exists && !lock.alive {
                let ok = build_guard::clean_stale_lock(&snap.lock_file).is_ok();
                if !ok {
                    all_restored = false;
                }
                checks.push(RollbackCheckItem {
                    item: "外部构建锁".into(),
                    restored: ok,
                    detail: if ok {
                        format!("已清理残留死锁 #{}", lock.pid)
                    } else {
                        "死锁清理失败".into()
                    },
                });
            }
        }
    }

    // 5. Destroy temporary transaction journal & backups
    commit_transaction(snap);

    RollbackVerification {
        tx_id: snap.tx_id.clone(),
        triggered_by: triggered_by.to_string(),
        all_restored,
        checks,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

/// Scan `<dataRoot>\tx\` on startup to recover any interrupted transactions from an unexpected crash.
pub fn recover_interrupted_transactions() -> CrashRecoveryResult {
    recover_interrupted_transactions_in(&storage::tx_dir())
}

pub fn recover_interrupted_transactions_in(base_tx_dir: &Path) -> CrashRecoveryResult {
    let mut verifications = Vec::new();
    if !base_tx_dir.is_dir() {
        return CrashRecoveryResult {
            recovered_count: 0,
            verifications,
        };
    }
    let my_pid = std::process::id();
    let entries: Vec<PathBuf> = fs::read_dir(base_tx_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();

    for meta_path in entries {
        let Ok(raw) = fs::read_to_string(&meta_path) else {
            continue;
        };
        let Ok(snap) = serde_json::from_str::<TransactionSnapshot>(&raw) else {
            let _ = fs::remove_file(&meta_path);
            continue;
        };
        if snap.pid == my_pid || is_pid_alive_os(snap.pid) {
            continue;
        }
        // Only recover when the recording process is verified dead!
        let v = rollback_and_verify(&snap, "上次异常中断自动恢复");
        verifications.push(v);
    }

    CrashRecoveryResult {
        recovered_count: verifications.len(),
        verifications,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RepoBinding, VersionFileSpec};

    fn dummy_project(root: &Path) -> ProjectConfig {
        ProjectConfig {
            id: "tx-test".into(),
            name: "TxTest".into(),
            root: root.to_string_lossy().to_string(),
            kind: "node".into(),
            adapter_label: "Node".into(),
            primary_language: "node".into(),
            detected_languages: vec!["node".into()],
            shell_type: "web".into(),
            product_name: "TxTest".into(),
            artifact_prefix: "TxTest".into(),
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
            version_files: vec![VersionFileSpec {
                path: "package.json".into(),
                format: "package_json".into(),
                pattern: String::new(),
            }],
            env: HashMap::new(),
            actions: vec![],
            package_profiles: vec![],
        }
    }

    #[test]
    fn test_transaction_rollback_restores_versions_git_and_release_cleanly() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_tx_test_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(19)
        ));
        let tx_root = base.join("_tx");
        let proj_dir = base.join("proj");
        let rel_dir = proj_dir.join("release");
        fs::create_dir_all(&rel_dir).unwrap();

        let orig_pkg = r#"{"name":"demo","version":"1.0.0"}"#;
        let orig_manifest = r#"{"version":"1.0.0"}"#;
        fs::write(proj_dir.join("package.json"), orig_pkg).unwrap();
        fs::write(proj_dir.join("CHANGELOG.md"), "# Log\n").unwrap();
        fs::write(proj_dir.join("user_wip.txt"), "do not delete me").unwrap();
        fs::write(rel_dir.join("latest.json"), orig_manifest).unwrap();

        // Initialize git repo with initial commit
        assert!(hidden_git(&proj_dir, &["init"]).is_some());
        assert!(hidden_git(&proj_dir, &["config", "user.email", "t@t.com"]).is_some());
        assert!(hidden_git(&proj_dir, &["config", "user.name", "Tester"]).is_some());
        assert!(hidden_git(&proj_dir, &["add", "package.json", "CHANGELOG.md"]).is_some());
        assert!(hidden_git(&proj_dir, &["commit", "-m", "init"]).is_some());
        let initial_head = hidden_git(&proj_dir, &["rev-parse", "HEAD"]).unwrap();

        let proj = dummy_project(&proj_dir);
        let snap = begin_transaction_in(&proj, "pipeline", &tx_root).unwrap();

        // Simulate mid-pipeline mutations: bump version, corrupt manifest, write partial zip, commit & tag
        fs::write(
            proj_dir.join("package.json"),
            r#"{"name":"demo","version":"1.0.1"}"#,
        )
        .unwrap();
        fs::write(rel_dir.join("latest.json"), r#"{"version":"1.0.1-broken"}"#).unwrap();
        fs::write(rel_dir.join("partial_build.zip.tmp"), b"corrupt").unwrap();
        assert!(hidden_git(&proj_dir, &["add", "package.json"]).is_some());
        assert!(hidden_git(&proj_dir, &["commit", "-m", "chore: v1.0.1"]).is_some());
        assert!(hidden_git(&proj_dir, &["tag", "v1.0.1"]).is_some());

        // Trigger rollback
        let proof = rollback_and_verify(&snap, "用户取消");
        assert!(proof.all_restored, "Rollback checks failed: {:?}", proof.checks);

        // Verify byte-for-byte restoration and user WIP preservation
        assert_eq!(fs::read_to_string(proj_dir.join("package.json")).unwrap(), orig_pkg);
        assert_eq!(fs::read_to_string(rel_dir.join("latest.json")).unwrap(), orig_manifest);
        assert!(!rel_dir.join("partial_build.zip.tmp").exists());
        assert_eq!(
            fs::read_to_string(proj_dir.join("user_wip.txt")).unwrap(),
            "do not delete me"
        );
        assert_eq!(
            hidden_git(&proj_dir, &["rev-parse", "HEAD"]).unwrap(),
            initial_head
        );
        assert!(hidden_git(&proj_dir, &["tag", "-l"]).unwrap().is_empty());
        assert!(!Path::new(&snap.tx_meta_file).exists());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_crash_recovery_replays_abandoned_transaction() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_crash_tx_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(23)
        ));
        let tx_root = base.join("_tx");
        let proj_dir = base.join("proj");
        fs::create_dir_all(&proj_dir).unwrap();
        let orig = r#"{"name":"demo","version":"2.0.0"}"#;
        fs::write(proj_dir.join("package.json"), orig).unwrap();

        let proj = dummy_project(&proj_dir);
        let mut snap = begin_transaction_in(&proj, "pipeline", &tx_root).unwrap();
        // Simulate foreign dead PID writing the snapshot before crashing
        snap.pid = 999_999;
        fs::write(&snap.tx_meta_file, serde_json::to_vec(&snap).unwrap()).unwrap();
        fs::write(
            proj_dir.join("package.json"),
            r#"{"name":"demo","version":"2.0.1"}"#,
        )
        .unwrap();

        let rec = recover_interrupted_transactions_in(&tx_root);
        assert_eq!(rec.recovered_count, 1);
        assert!(rec.verifications[0].all_restored);
        assert_eq!(fs::read_to_string(proj_dir.join("package.json")).unwrap(), orig);

        // Idempotent second run must recover 0
        let rec2 = recover_interrupted_transactions_in(&tx_root);
        assert_eq!(rec2.recovered_count, 0);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_remote_pushed_preserves_git_history_and_skips_live_pid_recovery() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_rt1_tx_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(67)
        ));
        let tx_root = base.join("_tx");
        let proj_dir = base.join("proj");
        fs::create_dir_all(&proj_dir).unwrap();
        fs::write(proj_dir.join("package.json"), r#"{"name":"d","version":"1.0.0"}"#).unwrap();

        assert!(hidden_git(&proj_dir, &["init"]).is_some());
        assert!(hidden_git(&proj_dir, &["config", "user.email", "t@t.com"]).is_some());
        assert!(hidden_git(&proj_dir, &["config", "user.name", "Tester"]).is_some());
        assert!(hidden_git(&proj_dir, &["add", "package.json"]).is_some());
        assert!(hidden_git(&proj_dir, &["commit", "-m", "init"]).is_some());

        let proj = dummy_project(&proj_dir);
        let mut snap = begin_transaction_in(&proj, "pipeline", &tx_root).unwrap();

        // Active current PID must NOT be touched by crash recovery
        let rec_live = recover_interrupted_transactions_in(&tx_root);
        assert_eq!(rec_live.recovered_count, 0);

        // Simulate commit + tag + remote push succeeded, then later step failed
        fs::write(proj_dir.join("package.json"), r#"{"name":"d","version":"1.0.1"}"#).unwrap();
        assert!(hidden_git(&proj_dir, &["add", "package.json"]).is_some());
        assert!(hidden_git(&proj_dir, &["commit", "-m", "release v1.0.1"]).is_some());
        assert!(hidden_git(&proj_dir, &["tag", "v1.0.1"]).is_some());
        let pushed_head = hidden_git(&proj_dir, &["rev-parse", "HEAD"]).unwrap();
        snap.mark_remote_pushed();

        let proof = rollback_and_verify(&snap, "上传失败");
        assert!(proof.all_restored);
        // Pushed HEAD, Tag, and pushed version file MUST stay aligned (zero dirty downgrade!)
        assert_eq!(hidden_git(&proj_dir, &["rev-parse", "HEAD"]).unwrap(), pushed_head);
        assert!(hidden_git(&proj_dir, &["tag", "-l"]).unwrap().contains("v1.0.1"));
        assert!(fs::read_to_string(proj_dir.join("package.json")).unwrap().contains("1.0.1"));
        assert!(hidden_git(&proj_dir, &["status", "--porcelain"]).unwrap().is_empty());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_build_verified_preserves_version_git_and_release_on_publish_failure() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_bv_tx_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(71)
        ));
        let tx_root = base.join("_tx");
        let proj_dir = base.join("proj");
        let rel_dir = proj_dir.join("release");
        fs::create_dir_all(&rel_dir).unwrap();
        fs::write(proj_dir.join("package.json"), r#"{"name":"d","version":"1.0.0"}"#).unwrap();
        fs::write(rel_dir.join("latest.json"), r#"{"version":"1.0.0"}"#).unwrap();

        assert!(hidden_git(&proj_dir, &["init"]).is_some());
        assert!(hidden_git(&proj_dir, &["config", "user.email", "t@t.com"]).is_some());
        assert!(hidden_git(&proj_dir, &["config", "user.name", "Tester"]).is_some());
        assert!(hidden_git(&proj_dir, &["add", "package.json"]).is_some());
        assert!(hidden_git(&proj_dir, &["commit", "-m", "init"]).is_some());

        let proj = dummy_project(&proj_dir);
        let mut snap = begin_transaction_in(&proj, "pipeline", &tx_root).unwrap();

        // Simulate bump -> build verified -> manifest -> local git commit/tag (NOT pushed) -> publish failed
        fs::write(proj_dir.join("package.json"), r#"{"name":"d","version":"1.0.1"}"#).unwrap();
        fs::write(rel_dir.join("TxTest_1.0.1_x64-setup.exe"), b"verified-installer-binary").unwrap();
        fs::write(rel_dir.join("latest.json"), r#"{"version":"1.0.1"}"#).unwrap();
        snap.mark_build_verified();

        assert!(hidden_git(&proj_dir, &["add", "package.json"]).is_some());
        assert!(hidden_git(&proj_dir, &["commit", "-m", "chore: v1.0.1"]).is_some());
        assert!(hidden_git(&proj_dir, &["tag", "v1.0.1"]).is_some());
        let local_head = hidden_git(&proj_dir, &["rev-parse", "HEAD"]).unwrap();

        let proof = rollback_and_verify(&snap, "发布上传超时");
        assert!(proof.all_restored, "Rollback checks: {:?}", proof.checks);

        // ALL THREE (version file, local Git HEAD+tag, verified release artifacts) MUST stay coherently at 1.0.1!
        assert!(fs::read_to_string(proj_dir.join("package.json")).unwrap().contains("1.0.1"));
        assert!(rel_dir.join("TxTest_1.0.1_x64-setup.exe").exists());
        assert!(fs::read_to_string(rel_dir.join("latest.json")).unwrap().contains("1.0.1"));
        assert_eq!(hidden_git(&proj_dir, &["rev-parse", "HEAD"]).unwrap(), local_head);
        assert!(hidden_git(&proj_dir, &["tag", "-l"]).unwrap().contains("v1.0.1"));
        assert!(hidden_git(&proj_dir, &["status", "--porcelain", "--", "package.json"]).unwrap().is_empty());
        assert!(!Path::new(&snap.tx_meta_file).exists());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_crash_recovery_preserves_verified_build_artifacts_when_crashed_during_upload() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_bv_crash_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(73)
        ));
        let tx_root = base.join("_tx");
        let proj_dir = base.join("proj");
        let rel_dir = proj_dir.join("release");
        fs::create_dir_all(&rel_dir).unwrap();
        fs::write(proj_dir.join("package.json"), r#"{"name":"d","version":"3.0.0"}"#).unwrap();

        let proj = dummy_project(&proj_dir);
        let mut snap = begin_transaction_in(&proj, "pipeline", &tx_root).unwrap();

        // Simulate build succeeded (persisted via mark_build_verified), then process died mid-upload
        fs::write(proj_dir.join("package.json"), r#"{"name":"d","version":"3.0.1"}"#).unwrap();
        fs::write(rel_dir.join("TxTest_3.0.1_x64-portable.zip"), b"verified-zip").unwrap();
        snap.pid = 999_998;
        snap.mark_build_verified();

        let rec = recover_interrupted_transactions_in(&tx_root);
        assert_eq!(rec.recovered_count, 1);
        assert!(rec.verifications[0].all_restored);
        // Verified artifacts & version MUST survive crash recovery!
        assert!(rel_dir.join("TxTest_3.0.1_x64-portable.zip").exists());
        assert!(fs::read_to_string(proj_dir.join("package.json")).unwrap().contains("3.0.1"));
        assert!(!Path::new(&snap.tx_meta_file).exists());

        let _ = fs::remove_dir_all(&base);
    }
}
