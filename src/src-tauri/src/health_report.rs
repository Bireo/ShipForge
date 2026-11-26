use crate::build_guard;
use crate::config;
use crate::git_ops;
use crate::guardrails::{build_headroom_advisories, compute_headroom};
use crate::models::{
    GuardrailSettings, HealthCheckItem, HealthReport, NextStepAction, PerformanceAdvisory,
    ProjectConfig, RollbackVerification,
};
use crate::packager::{scan_artifacts, verify_checksums};
use crate::version;
use std::path::Path;

/// Construct an ephemeral, memory-only Integrity & Health Analysis Report.
/// By design, this module performs ZERO writes to disk, journals, or caches.
pub fn build_ephemeral_report(
    project: &ProjectConfig,
    operation: &str,
    outcome: &str,
    duration_ms: u64,
    summary: String,
    rollback_proof: Option<RollbackVerification>,
    settings: &GuardrailSettings,
) -> HealthReport {
    let root = Path::new(&project.root);
    let release_dir = config::resolve_release_dir(project);
    let ver_state = version::read_project_versions(root, &project.version_files);
    let git = git_ops::inspect_git_for_project(root, &project.release_dir);
    let artifacts = scan_artifacts(&release_dir);
    let active_artifacts: Vec<_> = artifacts.iter().filter(|a| !a.is_archived).collect();

    let total_art_bytes: u64 = active_artifacts.iter().map(|a| a.size).sum();
    let headroom = compute_headroom(&release_dir, total_art_bytes.max(128 * 1024 * 1024), settings);

    let mut integrity_checks = Vec::new();
    let mut consistency_checks = Vec::new();
    let mut advisories: Vec<PerformanceAdvisory> = build_headroom_advisories(&headroom);

    // 1. Artifact & checksum integrity
    if active_artifacts.is_empty() {
        integrity_checks.push(HealthCheckItem {
            status: if outcome == "rolled_back" { "ok" } else { "warn" }.into(),
            label: "产物目录检查".into(),
            detail: if outcome == "rolled_back" {
                "已回滚并清理本次未完成产物".into()
            } else {
                "当前 release/ 目录暂无活跃产物".into()
            },
        });
    } else {
        integrity_checks.push(HealthCheckItem {
            status: "ok".into(),
            label: "活跃产物统计".into(),
            detail: format!(
                "{} 个产物，总计 {:.2} MB",
                active_artifacts.len(),
                total_art_bytes as f64 / (1024.0 * 1024.0)
            ),
        });

        let large_warn_bytes = settings.split_warning_mb.max(20) * 1024 * 1024;
        for a in &active_artifacts {
            if a.size > large_warn_bytes && project.repo_binding.provider == "gitee" {
                advisories.push(PerformanceAdvisory {
                    level: "warn".into(),
                    title: format!("{} 超过 Gitee 单文件建议上限", a.name),
                    detail: format!(
                        "文件体积 {:.1} MB，建议在「分析与分包」启用 {} MB 分卷输出后再上传。",
                        a.size as f64 / (1024.0 * 1024.0),
                        settings.split_warning_mb
                    ),
                });
            }
        }
    }

    if release_dir.join("SHA256SUMS.txt").exists() {
        match verify_checksums(&release_dir) {
            Ok(items) => {
                let listed: Vec<_> = items.iter().filter(|i| i.status != "unlisted").collect();
                let bad: Vec<_> = listed.iter().filter(|i| i.status != "ok").collect();
                let unlisted_cnt = items.len().saturating_sub(listed.len());
                integrity_checks.push(HealthCheckItem {
                    status: if bad.is_empty() { "ok" } else { "err" }.into(),
                    label: "SHA-256 完整性核验".into(),
                    detail: if bad.is_empty() {
                        if unlisted_cnt > 0 {
                            format!(
                                "{} 个清单文件全部校验一致（另有 {} 个未入清单产物）",
                                listed.len(),
                                unlisted_cnt
                            )
                        } else {
                            format!("{} 个清单文件全部校验一致", listed.len())
                        }
                    } else {
                        format!("发现 {} 个文件校验异常或缺失", bad.len())
                    },
                });
            }
            Err(e) => {
                integrity_checks.push(HealthCheckItem {
                    status: "warn".into(),
                    label: "SHA-256 完整性核验".into(),
                    detail: format!("校验跳过：{}", e),
                });
            }
        }
    }

    if release_dir.join("latest.json").exists() {
        let raw = std::fs::read_to_string(release_dir.join("latest.json")).unwrap_or_default();
        let (st, msg) = match serde_json::from_str::<serde_json::Value>(raw.trim_start_matches('\u{feff}')) {
            Err(e) => ("err", format!("JSON 格式损坏（{}），禁止用于线上发布", e)),
            Ok(val) => {
                let mver = val.get("version").and_then(|v| v.as_str()).unwrap_or("").trim();
                let mut problems: Vec<String> = Vec::new();
                if mver.is_empty() {
                    problems.push("缺少 version 字段".into());
                } else if !ver_state.readings.is_empty()
                    && ver_state.current != "0.0.0"
                    && mver != ver_state.current
                {
                    problems.push(format!(
                        "清单版本 v{} 与当前项目 v{} 不一致",
                        mver, ver_state.current
                    ));
                }
                let mut check_manifest_obj = |label: &str, obj: &serde_json::Map<String, serde_json::Value>| {
                    let url = obj.get("url").and_then(|x| x.as_str()).unwrap_or("").trim();
                    let ulow = url.to_ascii_lowercase();
                    if ulow.is_empty() {
                        problems.push(format!("{} 缺少 url", label));
                    } else if ulow.contains("127.0.0.1")
                        || ulow.contains("localhost")
                        || ulow.contains("::1")
                        || ulow.contains("0.0.0.0")
                    {
                        problems.push(format!("{} 含本机回环地址（发布前需重新生成正式清单）", label));
                    } else if !ulow.starts_with("https://") && !ulow.starts_with("http://") {
                        problems.push(format!("{} 下载地址缺少 http(s):// 协议头", label));
                    }
                    let sha_ok = obj
                        .get("sha256")
                        .and_then(|x| x.as_str())
                        .map(|s| s.trim().len() == 64 && s.trim().chars().all(|c| c.is_ascii_hexdigit()))
                        .unwrap_or(false);
                    if !sha_ok {
                        problems.push(format!("{} 缺少合法 64 位 sha256", label));
                    }
                    if let Some(fname) = obj.get("file").and_then(|x| x.as_str()) {
                        if let Err(e) = crate::guardrails::validate_single_filename(fname) {
                            problems.push(format!("{} 文件名非法（{}）", label, e));
                            return;
                        }
                        if let Some(fver) = crate::packager::extract_artifact_version(fname) {
                            if !mver.is_empty() && fver != mver {
                                problems.push(format!(
                                    "{} 指向 v{} 产物，与清单 v{} 错配",
                                    label, fver, mver
                                ));
                            }
                        }
                        match crate::guardrails::resolve_confined_path(&release_dir, fname) {
                            Ok(fpath) if fpath.is_file() => {
                                if let Some(exp_sz) = obj.get("size").and_then(|x| x.as_u64()) {
                                    let act_sz = std::fs::metadata(&fpath).map(|m| m.len()).unwrap_or(0);
                                    if act_sz != exp_sz {
                                        problems.push(format!(
                                            "{} 磁盘体积({} B)与清单({} B)不一致",
                                            fname, act_sz, exp_sz
                                        ));
                                    }
                                }
                            }
                            Ok(_) => problems.push(format!("引用产物 {} 不存在", fname)),
                            Err(e) => problems.push(format!("{} 路径越界（{}）", label, e)),
                        }
                    }
                };
                let mut entry_count = 0usize;
                for key in ["installer", "portable"] {
                    if let Some(obj) = val.get(key).and_then(|x| x.as_object()) {
                        entry_count += 1;
                        check_manifest_obj(key, obj);
                    }
                }
                if let Some(plats) = val.get("platforms").and_then(|x| x.as_object()) {
                    for (pk, pv) in plats {
                        if let Some(obj) = pv.as_object() {
                            entry_count += 1;
                            check_manifest_obj(&format!("platforms.{}", pk), obj);
                        }
                    }
                }
                if entry_count == 0 {
                    problems.push("缺少任何有效的 installer / portable / platforms 产物条目".into());
                }
                if problems.is_empty() {
                    ("ok", format!("v{} 清单结构、下载地址与本地产物一致", mver))
                } else {
                    ("warn", problems.join("；"))
                }
            }
        };
        integrity_checks.push(HealthCheckItem {
            status: st.into(),
            label: "更新清单 latest.json".into(),
            detail: msg,
        });
    }

    // 2. Version, Git & Lock consistency
    if project.version_files.is_empty() {
        consistency_checks.push(HealthCheckItem {
            status: "ok".into(),
            label: "多文件版本一致性".into(),
            detail: "未配置自动同步版本文件（跳过）".into(),
        });
    } else {
        let bad_files = ver_state
            .readings
            .iter()
            .filter(|r| !r.exists || r.error.is_some() || r.version.is_empty())
            .count();
        consistency_checks.push(HealthCheckItem {
            status: if ver_state.synced { "ok" } else { "warn" }.into(),
            label: "多文件版本一致性".into(),
            detail: if ver_state.synced {
                format!(
                    "全部 {} 个配置文件已统一为 v{}",
                    ver_state.readings.len(),
                    ver_state.current
                )
            } else if bad_files > 0 {
                format!("有 {} 个版本文件缺失或解析失败，请在「版本与源码」修复", bad_files)
            } else {
                "检测到版本号不一致，请在「版本与源码」对齐".into()
            },
        });
    }

    if git.is_repo {
        let git_clean = git.dirty_count == 0;
        let sha_part = if git.head_sha.is_empty() {
            "（尚无提交）".to_string()
        } else {
            format!(" @ {}", git.head_sha)
        };
        consistency_checks.push(HealthCheckItem {
            status: if git_clean { "ok" } else { "warn" }.into(),
            label: "Git 工作区与远端状态".into(),
            detail: format!(
                "分支 {}{}{}{}",
                git.branch,
                sha_part,
                if git.dirty_count > 0 {
                    format!("，{} 个未提交变更", git.dirty_count)
                } else {
                    "，工作区干净".into()
                },
                if git.ahead_count > 0 {
                    format!("，{} 个提交待推送", git.ahead_count)
                } else {
                    String::new()
                }
            ),
        });
    } else {
        consistency_checks.push(HealthCheckItem {
            status: "warn".into(),
            label: "Git 仓库状态".into(),
            detail: "尚未初始化 Git（已自动启用本地文件指纹追踪）".into(),
        });
    }

    if !project.lock_file.trim().is_empty() {
        let lock = build_guard::inspect_lock(&project.lock_file);
        let lock_ok = lock.as_ref().map(|l| !l.exists).unwrap_or(true);
        consistency_checks.push(HealthCheckItem {
            status: if lock_ok { "ok" } else { "warn" }.into(),
            label: "构建锁释放状态".into(),
            detail: if lock_ok {
                "空闲，无残留死锁".into()
            } else {
                "检测到锁文件仍存在".into()
            },
        });
    }

    // 3. Performance hints
    if project.kind == "tauri" || project.kind == "rust" {
        advisories.push(PerformanceAdvisory {
            level: "info".into(),
            title: "Rust 增量与多核链接提示".into(),
            detail: format!(
                "当前安全并发上限 {} 线程；已推荐使用 crate 级并行编译（codegen-units=16）与 lld 链接器保持发布速度与体积平衡。",
                headroom.safe_concurrency
            ),
        });
    }
    if project.repo_binding.repo_slug.trim().is_empty() && project.github_repo.trim().is_empty() {
        advisories.push(PerformanceAdvisory {
            level: "info".into(),
            title: "尚未绑定目标发布仓库".into(),
            detail: "前往「产物与仓库」配置账号令牌并下拉选择仓库，即可一键上传 Release。".into(),
        });
    }

    // 4. Contextual next-step guidance
    let mut next_steps = Vec::new();
    if outcome == "ok"
        && (operation == "build"
            || operation == "pipeline"
            || operation == "pack"
            || operation == "deploy")
    {
        if !project.launch_exe.is_empty() {
            next_steps.push(NextStepAction {
                id: "smoke-launch".into(),
                label: "启动程序冒烟测试".into(),
                target_page: "overview".into(),
                hint: "运行当前生成的可执行文件确认界面与核心功能正常".into(),
            });
        }
        next_steps.push(NextStepAction {
            id: "go-release".into(),
            label: if operation == "deploy" {
                "前往产物与仓库核验线上 Release".into()
            } else {
                "前往产物与仓库发布".into()
            },
            target_page: "release".into(),
            hint: if operation == "deploy" {
                "查看已上传资产列表、编辑 Release 说明或执行线上维护".into()
            } else {
                "检查产物分级、核验 SHA-256 并上传到目标仓库".into()
            },
        });
    } else if outcome == "rolled_back" {
        let has_current_artifacts = !ver_state.current.is_empty()
            && active_artifacts
                .iter()
                .any(|a| a.version.as_deref() == Some(ver_state.current.as_str()));
        let manifest_aligned = integrity_checks
            .iter()
            .any(|c| c.label == "更新清单 latest.json" && c.status == "ok");
        if has_current_artifacts {
            next_steps.push(NextStepAction {
                id: "retry-release".into(),
                label: "直接重试上传发布（无需重编）".into(),
                target_page: "release".into(),
                hint: if manifest_aligned {
                    "本次编译产物与清单均已完整保留，前往「产物与仓库」即可直接续传或重发".into()
                } else {
                    "本次编译产物已完整保留；请先在「产物与仓库」补齐更新清单后再直接上传（无需重编）".into()
                },
            });
        }
        next_steps.push(NextStepAction {
            id: "check-prereq".into(),
            label: "检查前置环境与依赖".into(),
            target_page: "compile".into(),
            hint: "排查编译前置缺失或查看执行终端中的失败位置".into(),
        });
    }

    HealthReport {
        report_id: format!("hr-{}", chrono::Local::now().timestamp_millis()),
        project_id: project.id.clone(),
        project_name: project.name.clone(),
        operation: operation.to_string(),
        outcome: outcome.to_string(),
        version: ver_state.current,
        duration_ms,
        summary,
        rollback_proof,
        integrity_checks,
        consistency_checks,
        headroom,
        advisories,
        next_steps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RepoBinding;
    use std::collections::HashMap;

    #[test]
    fn test_ephemeral_health_report_leaves_zero_files_on_disk() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_hr_test_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(29)
        ));
        std::fs::create_dir_all(dir.join("release")).unwrap();
        let proj = ProjectConfig {
            id: "hr-proj".into(),
            name: "HRProj".into(),
            root: dir.to_string_lossy().to_string(),
            kind: "go".into(),
            adapter_label: "Go".into(),
            primary_language: "go".into(),
            detected_languages: vec!["go".into()],
            shell_type: "portable".into(),
            product_name: "HRProj".into(),
            artifact_prefix: "HRProj".into(),
            release_dir: "release".into(),
            changelog_path: String::new(),
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
        let report = build_ephemeral_report(
            &proj,
            "build",
            "ok",
            1200,
            "构建成功".into(),
            None,
            &GuardrailSettings::default(),
        );
        assert_eq!(report.outcome, "ok");
        assert!(!report.integrity_checks.is_empty());
        // Verify ZERO report files were written into project dir or release dir
        let count_proj = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(count_proj, 1); // only `release` dir
        assert_eq!(std::fs::read_dir(dir.join("release")).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_health_report_flags_broken_stale_and_empty_latest_json() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_hr_mf_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(30)
        ));
        let rel = dir.join("release");
        std::fs::create_dir_all(&rel).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"version":"2.0.0"}"#).unwrap();

        let mut proj = ProjectConfig::default();
        proj.id = "hr-mf".into();
        proj.name = "HRMf".into();
        proj.root = dir.to_string_lossy().to_string();
        proj.release_dir = "release".into();
        proj.version_files.push(crate::models::VersionFileSpec {
            path: "package.json".into(),
            format: "package_json".into(),
            pattern: String::new(),
        });

        // 1. Corrupt JSON -> err
        std::fs::write(rel.join("latest.json"), "{broken json").unwrap();
        let r1 = build_ephemeral_report(&proj, "build", "ok", 100, "ok".into(), None, &GuardrailSettings::default());
        let mf1 = r1.integrity_checks.iter().find(|c| c.label.contains("latest.json")).unwrap();
        assert_eq!(mf1.status, "err");

        // 2. Empty manifest with zero entries -> warn
        std::fs::write(rel.join("latest.json"), r#"{"version":"2.0.0"}"#).unwrap();
        let r2 = build_ephemeral_report(&proj, "build", "ok", 100, "ok".into(), None, &GuardrailSettings::default());
        let mf2 = r2.integrity_checks.iter().find(|c| c.label.contains("latest.json")).unwrap();
        assert_eq!(mf2.status, "warn");
        assert!(mf2.detail.contains("缺少任何有效"));

        // 3. Version-poisoned installer filename + missing sha256 -> warn
        std::fs::write(rel.join("App_1.9.9_x64-setup.exe"), b"old-binary").unwrap();
        std::fs::write(
            rel.join("latest.json"),
            r#"{"version":"2.0.0","installer":{"file":"App_1.9.9_x64-setup.exe","url":"https://example.com/a.exe","size":10}}"#,
        )
        .unwrap();
        let r3 = build_ephemeral_report(&proj, "build", "ok", 100, "ok".into(), None, &GuardrailSettings::default());
        let mf3 = r3.integrity_checks.iter().find(|c| c.label.contains("latest.json")).unwrap();
        assert_eq!(mf3.status, "warn");
        assert!(mf3.detail.contains("错配") && mf3.detail.contains("sha256"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_health_report_offers_retry_release_without_rebuild_when_rolled_back_with_verified_artifacts() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_hr_retry_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(31)
        ));
        let rel = dir.join("release");
        std::fs::create_dir_all(&rel).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"version":"2.1.0"}"#).unwrap();
        std::fs::write(rel.join("App_2.1.0_x64-setup.exe"), b"verified-installer").unwrap();

        let mut proj = ProjectConfig::default();
        proj.id = "hr-retry".into();
        proj.name = "HRRetry".into();
        proj.root = dir.to_string_lossy().to_string();
        proj.release_dir = "release".into();
        proj.version_files.push(crate::models::VersionFileSpec {
            path: "package.json".into(),
            format: "package_json".into(),
            pattern: String::new(),
        });

        // Case A: rolled_back with verified artifacts but missing latest.json
        let r_no_mf = build_ephemeral_report(
            &proj,
            "pipeline",
            "rolled_back",
            500,
            "上传超时".into(),
            None,
            &GuardrailSettings::default(),
        );
        let retry_a = r_no_mf
            .next_steps
            .iter()
            .find(|s| s.id == "retry-release")
            .expect("should suggest retry-release without rebuild");
        assert!(retry_a.hint.contains("补齐更新清单"));

        // Case B: rolled_back with verified artifacts AND aligned latest.json
        let setup_sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        std::fs::write(
            rel.join("latest.json"),
            format!(
                r#"{{"version":"2.1.0","installer":{{"file":"App_2.1.0_x64-setup.exe","url":"https://example.com/a.exe","size":18,"sha256":"{}"}}}}"#,
                setup_sha
            ),
        )
        .unwrap();
        let r_with_mf = build_ephemeral_report(
            &proj,
            "pipeline",
            "rolled_back",
            500,
            "上传超时".into(),
            None,
            &GuardrailSettings::default(),
        );
        let retry_b = r_with_mf
            .next_steps
            .iter()
            .find(|s| s.id == "retry-release")
            .expect("should suggest direct retry-release");
        assert!(retry_b.hint.contains("产物与清单均已完整保留"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
