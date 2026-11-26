use crate::credentials;
use crate::git_ops;
use crate::guardrails::{compute_headroom, ensure_disk_headroom};
use crate::models::{
    GuardrailSettings, PrereqItem, PrereqReport, ProjectConfig, RepairPlan, RepairResult,
    ToolchainItem,
};
use crate::runner::{emit_log, emit_progress};
use crate::storage;
use crate::toolchain;
use crate::version;
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::AppHandle;

/// Finds the directory containing `package.json` (root first, then standard frontend subdirs).
fn find_node_project_dir(root: &Path) -> Option<PathBuf> {
    for rel in ["", "frontend", "web", "src"] {
        let dir = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(rel)
        };
        if dir.join("package.json").is_file() {
            return Some(dir);
        }
    }
    None
}

fn dir_has_any_entry(dir: &Path) -> bool {
    fs::read_dir(dir)
        .ok()
        .and_then(|mut rd| rd.next())
        .is_some()
}

fn tauri_requires_nsis(root: &Path) -> bool {
    for rel in [
        "src-tauri/tauri.conf.json",
        "src/src-tauri/tauri.conf.json",
        "tauri.conf.json",
    ] {
        let p = root.join(rel);
        let Ok(raw) = fs::read_to_string(&p) else {
            continue;
        };
        let Ok(val) = serde_json::from_str::<serde_json::Value>(raw.trim_start_matches('\u{feff}'))
        else {
            continue;
        };
        if let Some(bundle) = val.get("bundle").and_then(|b| b.as_object()) {
            if bundle.get("active").and_then(|v| v.as_bool()) == Some(false) {
                return false;
            }
            if let Some(targets) = bundle.get("targets") {
                if let Some(s) = targets.as_str() {
                    return s.eq_ignore_ascii_case("all") || s.eq_ignore_ascii_case("nsis");
                }
                if let Some(arr) = targets.as_array() {
                    return arr.iter().any(|t| {
                        t.as_str()
                            .map(|s| s.eq_ignore_ascii_case("all") || s.eq_ignore_ascii_case("nsis"))
                            .unwrap_or(false)
                    });
                }
            }
        }
        return true;
    }
    true
}

fn find_python_req_dir(root: &Path, project: &ProjectConfig) -> Option<PathBuf> {
    if root.join("requirements.txt").is_file() || root.join("pyproject.toml").is_file() {
        return Some(root.to_path_buf());
    }
    let eng = project.engine_src_dir.trim();
    if !eng.is_empty() {
        let ed = root.join(eng);
        if ed.join("requirements.txt").is_file() || ed.join("pyproject.toml").is_file() {
            return Some(ed);
        }
    }
    None
}

fn has_python_sub_engine(root: &Path, project: &ProjectConfig) -> bool {
    let eng = project.engine_src_dir.trim();
    if eng.is_empty() {
        return false;
    }
    let dir = root.join(eng);
    if !dir.is_dir() {
        return false;
    }
    if dir.join("requirements.txt").is_file() || dir.join("pyproject.toml").is_file() {
        return true;
    }
    fs::read_dir(&dir)
        .ok()
        .map(|rd| {
            rd.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                name.ends_with(".py") || name.ends_with(".spec")
            })
        })
        .unwrap_or(false)
}

fn push_node_pm_tools(
    node_dir: &Path,
    push_unique: &mut dyn FnMut(&'static str, &'static str, bool, u64),
) {
    push_unique("node", "Node.js 运行时与前端构建链", true, 80);
    if node_dir.join("pnpm-lock.yaml").exists() {
        push_unique("pnpm", "当前项目锁定使用 pnpm 包管理器", true, 18);
    } else if node_dir.join("bun.lockb").exists() || node_dir.join("bun.lock").exists() {
        push_unique("bun", "当前项目锁定使用 Bun 包管理器", true, 40);
    } else if node_dir.join("yarn.lock").exists() {
        push_unique("yarn", "当前项目锁定使用 Yarn 包管理器", true, 12);
    } else {
        push_unique("npm", "管理前端与构建脚本依赖", true, 10);
    }
}

fn required_tools_for_project(project: &ProjectConfig) -> Vec<(&'static str, &'static str, bool, u64)> {
    // (tool_id, reason, required, estimated_mb)
    let mut list: Vec<(&'static str, &'static str, bool, u64)> = Vec::new();
    let root = Path::new(&project.root);
    let lang = project.primary_language.as_str();
    let shell = project.shell_type.as_str();
    let kind = project.kind.as_str();

    let mut push_unique = |id: &'static str, reason: &'static str, req: bool, mb: u64| {
        if let Some(existing) = list.iter_mut().find(|x| x.0 == id) {
            if req {
                existing.2 = true;
            }
        } else {
            list.push((id, reason, req, mb));
        }
    };

    // Axis 1: Primary framework / language toolchain
    if kind == "tauri" || shell == "tauri" {
        push_unique("rustc", "编译 Tauri Rust 后端内核", true, 350);
        push_unique("cargo", "管理与构建 Rust 依赖包", true, 100);
        push_unique("linker", "Rust 原生链接器（MSVC link.exe 或 MinGW gcc/lld）", true, 220);
        push_unique("cargo-tauri", "驱动 Tauri 桌面外壳构建与打包", true, 35);
        let need_nsis = tauri_requires_nsis(root);
        push_unique("makensis", "生成 Windows NSIS 安装程序", need_nsis, 25);
        push_unique("webview2", "Tauri 桌面窗口渲染运行时", true, 2);
        if let Some(nd) = find_node_project_dir(root) {
            push_node_pm_tools(&nd, &mut push_unique);
        }
    } else if kind == "rust" || lang == "rust" {
        push_unique("rustc", "编译 Rust 源码", true, 350);
        push_unique("cargo", "Rust 包管理与测试构建", true, 100);
        push_unique("linker", "Rust 原生链接器（MSVC link.exe 或 MinGW gcc/lld）", true, 220);
    } else if kind == "electron" || shell == "electron" || kind == "node" || lang == "node" {
        let nd = find_node_project_dir(root).unwrap_or_else(|| root.to_path_buf());
        push_node_pm_tools(&nd, &mut push_unique);
    } else if kind == "python" || lang == "python" {
        push_unique("python", "Python 解释器与打包环境", true, 90);
        push_unique("uv", "高速创建隔离虚拟环境与安装依赖", false, 30);
    } else if kind == "go" || lang == "go" || kind == "wails" || shell == "wails" {
        push_unique("go", "Go 编译器与模块构建工具", true, 180);
        if kind == "wails" || shell == "wails" {
            push_unique("wails", "Wails 桌面应用打包 CLI", true, 25);
            push_unique("webview2", "Wails Windows 桌面渲染运行时", true, 2);
            if let Some(nd) = find_node_project_dir(root) {
                push_node_pm_tools(&nd, &mut push_unique);
            }
        }
    } else if kind == "dotnet" || lang == "dotnet" {
        push_unique("dotnet", ".NET SDK 编译与发布工具链", true, 260);
    } else if kind == "flutter" || lang == "flutter" || lang == "dart" || shell == "flutter" {
        push_unique("flutter", "Flutter SDK 桌面构建工具链", true, 800);
    } else if kind == "cpp" || lang == "cpp" {
        push_unique("cmake", "CMake 跨平台构建配置工具", true, 45);
        push_unique("gcc", "C/C++ 编译器（MinGW GCC 或 MSVC cl.exe 任一可用）", true, 180);
    } else if kind == "java" || lang == "java" {
        push_unique("javac", "Java JDK 编译器", true, 220);
        if root.join("pom.xml").exists() {
            push_unique("mvn", "Maven 构建与打包工具（支持项目内 mvnw.cmd）", true, 20);
        }
        if root.join("build.gradle").exists() || root.join("build.gradle.kts").exists() {
            push_unique("gradle", "Gradle 构建与打包工具（支持项目内 gradlew.bat）", true, 35);
        }
    }

    // Axis 2: Orthogonal installer shell requirement (works even when primary language is Python/Rust/Go/Node)
    if kind == "nsis" || shell == "nsis" {
        push_unique("makensis", "编译 NSIS 安装脚本", true, 25);
    }
    if shell == "inno" {
        push_unique("iscc", "编译 Inno Setup 安装脚本", true, 15);
    }

    // Axis 3: Orthogonal sub-engine requirement (e.g., Tauri/Electron app with Python sub-engine)
    if has_python_sub_engine(root, project) {
        push_unique("python", "编译与测试项目内嵌 Python 子引擎", true, 90);
        push_unique("uv", "高速隔离 Python 子引擎打包环境", false, 30);
    }

    // Axis 4: Build scripts & VCS
    if project.has_build_ps1 {
        push_unique("pwsh", "执行项目根目录 build.ps1 构建脚本", false, 110);
    }
    let uses_git = root.join(".git").exists()
        || !project.repo_binding.repo_slug.trim().is_empty()
        || !project.github_repo.trim().is_empty();
    push_unique("git", "代码版本控制、自动打 Tag 与远端同步", uses_git, 65);
    list
}

fn repair_strategy_for_tool(id: &str) -> &'static str {
    match id {
        "cargo-tauri" | "makensis" | "uv" | "gh" => "portable",
        "rustc" | "cargo" | "node" | "pnpm" | "python" | "go" | "dotnet" | "git" | "pwsh"
        | "iscc" | "cmake" => "system_sdk",
        _ => "manual",
    }
}

/// Inspect all 3 tiers of prerequisites specifically scoped to the active project.
pub fn inspect_project_prerequisites(
    project: &ProjectConfig,
    settings: &GuardrailSettings,
) -> PrereqReport {
    let root = Path::new(&project.root);
    let needed = required_tools_for_project(project);
    let needed_ids: Vec<&str> = needed.iter().map(|x| x.0).collect();
    let tools: HashMap<String, ToolchainItem> =
        toolchain::scan_toolchains_for(Some(&needed_ids), Some(root))
            .into_iter()
            .map(|t| (t.id.clone(), t))
            .collect();

    let mut items: Vec<PrereqItem> = Vec::new();
    let tools_dir = storage::tools_dir().to_string_lossy().to_string();

    // Tier 1: Project-scoped toolchains & runtimes
    for (tool_id, reason, required, est_mb) in needed {
        if let Some(t) = tools.get(tool_id) {
            let strat = repair_strategy_for_tool(tool_id);
            items.push(PrereqItem {
                id: format!("tool:{}", tool_id),
                tier: "toolchain".into(),
                name: t.name.clone(),
                status: t.status.clone(),
                required,
                reason: reason.to_string(),
                detail: if t.available {
                    format!("{} ({})", t.version, t.path)
                } else {
                    t.detail.clone()
                },
                repairable: !t.available && t.repairable,
                repair_strategy: strat.into(),
                target_path: if strat == "portable" {
                    tools_dir.clone()
                } else {
                    "系统工具链目录".into()
                },
                estimated_mb: est_mb,
            });
        }
    }

    // Tier 2: Project-local dependencies & essential config files
    if let Some(node_dir) = find_node_project_dir(root) {
        let nm_dir = node_dir.join("node_modules");
        let has_nm = nm_dir.is_dir() && dir_has_any_entry(&nm_dir);
        items.push(PrereqItem {
            id: "dep:node_modules".into(),
            tier: "project".into(),
            name: "Node 依赖目录 (node_modules)".into(),
            status: if has_nm { "ok" } else { "missing" }.into(),
            required: matches!(project.kind.as_str(), "node" | "electron")
                || node_dir.join("package-lock.json").exists()
                || node_dir.join("pnpm-lock.yaml").exists()
                || node_dir.join("yarn.lock").exists()
                || node_dir.join("bun.lockb").exists()
                || node_dir.join("bun.lock").exists(),
            reason: "前端打包或 Electron 构建需要本地安装 npm 依赖".into(),
            detail: if has_nm {
                "已就绪".into()
            } else if nm_dir.is_dir() {
                "node_modules 为空目录，构建前需执行依赖安装".into()
            } else {
                "未检测到 node_modules，构建前需执行依赖安装".into()
            },
            repairable: !has_nm,
            repair_strategy: "project_dep".into(),
            target_path: nm_dir.to_string_lossy().to_string(),
            estimated_mb: 120,
        });
    }

    if (project.kind == "python"
        || project.primary_language == "python"
        || has_python_sub_engine(root, project))
        && find_python_req_dir(root, project).is_some()
    {
        let py_dir = find_python_req_dir(root, project).unwrap_or_else(|| root.to_path_buf());
        let is_valid_venv = |d: PathBuf| {
            d.join(r"Scripts\python.exe").is_file() || d.join("bin/python").is_file()
        };
        let has_venv = is_valid_venv(root.join(".venv"))
            || is_valid_venv(root.join("venv"))
            || is_valid_venv(py_dir.join(".venv"))
            || is_valid_venv(py_dir.join("venv"));
        items.push(PrereqItem {
            id: "dep:python_venv".into(),
            tier: "project".into(),
            name: "Python 隔离虚拟环境 (.venv)".into(),
            status: if has_venv { "ok" } else { "warn" }.into(),
            required: false,
            reason: "隔离项目依赖，避免污染全局 Python 环境或打包缺失模块".into(),
            detail: if has_venv {
                "已检测到项目虚拟环境".into()
            } else {
                "未创建 .venv，可一键初始化并安装依赖".into()
            },
            repairable: !has_venv,
            repair_strategy: "project_dep".into(),
            target_path: root.join(".venv").to_string_lossy().to_string(),
            estimated_mb: 80,
        });
    }

    // Version files sync check
    if !project.version_files.is_empty() {
        let ver = version::read_project_versions(root, &project.version_files);
        let bad_count = ver
            .readings
            .iter()
            .filter(|r| !r.exists || r.error.is_some() || r.version.is_empty())
            .count();
        items.push(PrereqItem {
            id: "cfg:version_sync".into(),
            tier: "project".into(),
            name: "多文件版本号对齐".into(),
            status: if ver.synced { "ok" } else { "warn" }.into(),
            required: false,
            reason: "保证安装包、清单与配置文件对外展示同一版本号".into(),
            detail: if ver.synced {
                format!("全部 {} 处配置一致：v{}", ver.readings.len(), ver.current)
            } else if bad_count > 0 {
                format!("有 {} 处版本配置文件缺失或解析失败，请先核查配置", bad_count)
            } else {
                "检测到不同配置文件版本号不一致，建议一键对齐至最高版本".into()
            },
            repairable: !ver.synced && bad_count == 0,
            repair_strategy: "config_fix".into(),
            target_path: project.root.clone(),
            estimated_mb: 0,
        });
    }

    // .gitignore protection for release/
    let rel_dir_name = storage::normalize_project_rel_path(root, &project.release_dir)
        .unwrap_or_else(|| "release".to_string());
    let gitignore_path = root.join(".gitignore");
    let has_rel_ignore = fs::read_to_string(&gitignore_path)
        .map(|s| {
            s.lines().any(|l| {
                let t = l
                    .trim()
                    .trim_start_matches("./")
                    .trim_start_matches('/')
                    .trim_end_matches('/');
                t.eq_ignore_ascii_case(&rel_dir_name)
            })
        })
        .unwrap_or(false);
    items.push(PrereqItem {
        id: "cfg:gitignore_release".into(),
        tier: "project".into(),
        name: format!(".gitignore 忽略产物目录 ({}/)", rel_dir_name),
        status: if has_rel_ignore { "ok" } else { "warn" }.into(),
        required: false,
        reason: "防止安装包与大体积压缩包被误提交进 Git 仓库历史".into(),
        detail: if has_rel_ignore {
            "已配置忽略规则".into()
        } else {
            format!("未在 .gitignore 中排除 {}/", rel_dir_name)
        },
        repairable: !has_rel_ignore,
        repair_strategy: "config_fix".into(),
        target_path: gitignore_path.to_string_lossy().to_string(),
        estimated_mb: 0,
    });

    // Tier 3: Git & Repository publish readiness
    let git = git_ops::inspect_git_for_project(root, &project.release_dir);
    items.push(PrereqItem {
        id: "pub:git_repo".into(),
        tier: "publish".into(),
        name: "Git 仓库初始化".into(),
        status: if git.is_repo { "ok" } else { "warn" }.into(),
        required: false,
        reason: "用于版本追溯、自动生成变更草稿和打 Release Tag".into(),
        detail: if git.is_repo {
            if git.head_sha.is_empty() {
                format!("当前分支 {}（尚无提交）", git.branch)
            } else {
                format!("当前分支 {} @ {}", git.branch, git.head_sha)
            }
        } else {
            "目录尚未初始化 Git，可一键初始化".into()
        },
        repairable: !git.is_repo,
        repair_strategy: "config_fix".into(),
        target_path: root.join(".git").to_string_lossy().to_string(),
        estimated_mb: 0,
    });

    let repo_slug = if !project.repo_binding.repo_slug.trim().is_empty() {
        project.repo_binding.repo_slug.trim().to_string()
    } else {
        project.github_repo.trim().to_string()
    };
    let prov = if !project.repo_binding.provider.trim().is_empty() {
        project.repo_binding.provider.as_str()
    } else {
        "github"
    };
    let has_token = credentials::resolve_token(
        prov,
        &project.repo_binding.base_url,
        &project.repo_binding.credential_id,
    )
    .is_ok();

    items.push(PrereqItem {
        id: "pub:repo_binding".into(),
        tier: "publish".into(),
        name: "目标发布仓库与认证令牌".into(),
        status: if !repo_slug.is_empty() && has_token {
            "ok"
        } else {
            "warn"
        }
        .into(),
        required: false,
        reason: "用于直接将打包产物上传至你的 GitHub / Gitee / GitLab / Gitea 仓库".into(),
        detail: if !repo_slug.is_empty() && has_token {
            format!("已绑定 {} · {}", prov, repo_slug)
        } else if repo_slug.is_empty() {
            "尚未绑定目标仓库，可在「产物与仓库」或向导中下拉选择".into()
        } else {
            format!("已绑定 {}，但尚未配置 {} 访问令牌", repo_slug, prov)
        },
        repairable: false,
        repair_strategy: "manual".into(),
        target_path: "产物与仓库".into(),
        estimated_mb: 0,
    });

    let missing_required = items
        .iter()
        .filter(|i| i.required && i.status != "ok")
        .count();
    let warnings = items
        .iter()
        .filter(|i| !i.required && i.status != "ok")
        .count();

    let headroom = compute_headroom(root, 512 * 1024 * 1024, settings);

    PrereqReport {
        project_id: project.id.clone(),
        language: project.primary_language.clone(),
        shell: project.shell_type.clone(),
        ready: missing_required == 0 && headroom.disk_ok,
        missing_required,
        warnings,
        items,
        headroom,
    }
}

/// Build a transparent, user-consent repair plan preview prior to executing any download or change.
pub fn plan_prerequisite_repair(
    project: &ProjectConfig,
    selected_ids: &[String],
    settings: &GuardrailSettings,
) -> RepairPlan {
    let report = inspect_project_prerequisites(project, settings);
    let chosen: Vec<PrereqItem> = report
        .items
        .into_iter()
        .filter(|i| {
            i.status != "ok"
                && i.repairable
                && (selected_ids.is_empty() || selected_ids.iter().any(|id| id == &i.id))
        })
        .collect();
    let total_mb: u64 = chosen.iter().map(|i| i.estimated_mb).sum();
    let disk_free_mb = report.headroom.disk_free_bytes / (1024 * 1024);
    let required_mb = settings.min_free_disk_mb.max(total_mb.saturating_mul(2));
    let disk_ok = report.headroom.disk_total_bytes == 0 || disk_free_mb >= required_mb;
    let summary = if chosen.is_empty() {
        "当前没有需要自动补全的前置项。".into()
    } else {
        format!(
            "将补全 {} 项前置依赖，预计下载/占用约 {} MB（目标盘可用 {} MB）。",
            chosen.len(),
            total_mb,
            disk_free_mb
        )
    };
    RepairPlan {
        project_id: project.id.clone(),
        items: chosen,
        total_estimated_mb: total_mb,
        disk_free_mb,
        disk_ok,
        summary,
    }
}

fn run_hidden_cmd(root: &Path, bin: &str, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new(bin);
    cmd.current_dir(root).args(args);
    let managed_prefix = toolchain::managed_path_prefix();
    if !managed_prefix.is_empty() {
        let cur_path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{};{}", managed_prefix, cur_path));
    }
    for (k, v) in storage::managed_cache_env_vars() {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let out = cmd.output()?;
    if out.status.success() {
        Ok(crate::runner::decode_win_bytes(&out.stdout).trim().to_string())
    } else {
        let err = crate::runner::decode_win_bytes(&out.stderr);
        let msg = if err.trim().is_empty() {
            crate::runner::decode_win_bytes(&out.stdout)
        } else {
            err
        };
        Err(anyhow::anyhow!("{} 执行失败：{}", bin, msg.trim()))
    }
}

/// Execute ONLY the user-approved repair items with real-time progress and post-repair verification.
pub fn execute_prerequisite_repair(
    app: &AppHandle,
    task_id: &str,
    project: &ProjectConfig,
    approved_ids: &[String],
    extra_project_roots: &[String],
    settings: &GuardrailSettings,
) -> Result<RepairResult> {
    // Explicit consent guard: empty approval list MUST perform zero mutations!
    if approved_ids.is_empty() {
        let rep = inspect_project_prerequisites(project, settings);
        return Ok(RepairResult {
            succeeded: vec![],
            failed: vec![],
            report: rep,
        });
    }
    let plan = plan_prerequisite_repair(project, approved_ids, settings);
    if plan.items.is_empty() {
        let rep = inspect_project_prerequisites(project, settings);
        return Ok(RepairResult {
            succeeded: vec![],
            failed: vec![],
            report: rep,
        });
    }

    let root = Path::new(&project.root);
    let tool_mb: u64 = plan
        .items
        .iter()
        .filter(|i| i.id.starts_with("tool:"))
        .map(|i| i.estimated_mb)
        .sum();
    let dep_mb: u64 = plan
        .items
        .iter()
        .filter(|i| i.id.starts_with("dep:"))
        .map(|i| i.estimated_mb)
        .sum();
    if tool_mb > 0 {
        ensure_disk_headroom(&storage::tools_dir(), tool_mb * 1024 * 1024, settings)?;
    }
    if dep_mb > 0 {
        ensure_disk_headroom(root, dep_mb * 1024 * 1024, settings)?;
    }

    let total = plan.items.len();
    let mut succeeded = Vec::new();
    let mut failed = Vec::new();

    for (idx, item) in plan.items.iter().enumerate() {
        let min_pct = ((idx * 95) / total).max(2) as u8;
        let max_pct = (((idx + 1) * 95) / total).min(98) as u8;
        emit_progress(
            app,
            task_id,
            "前置依赖补全",
            &format!("[{}/{}] {}", idx + 1, total, item.name),
            "正在补全…",
            min_pct,
            false,
        );
        emit_log(
            app,
            task_id,
            "system",
            &format!("[{}/{}] 正在补全：{}", idx + 1, total, item.name),
        );

        let res: Result<()> = if let Some(tool_id) = item.id.strip_prefix("tool:") {
            toolchain::repair_single_tool_in_range(
                app,
                task_id,
                tool_id,
                extra_project_roots,
                min_pct,
                max_pct,
            )
            .map(|_| ())
        } else {
            match item.id.as_str() {
                "dep:node_modules" => {
                    let node_dir = find_node_project_dir(root).unwrap_or_else(|| root.to_path_buf());
                    let pm = if node_dir.join("pnpm-lock.yaml").exists() {
                        "pnpm.cmd"
                    } else if node_dir.join("yarn.lock").exists() {
                        "yarn.cmd"
                    } else if node_dir.join("bun.lockb").exists() || node_dir.join("bun.lock").exists() {
                        "bun.exe"
                    } else {
                        "npm.cmd"
                    };
                    run_hidden_cmd(&node_dir, pm, &["install"]).map(|_| ())
                }
                "dep:python_venv" => {
                    let py_dir = find_python_req_dir(root, project).unwrap_or_else(|| root.to_path_buf());
                    let req_txt = py_dir.join("requirements.txt");
                    if let Some(uv) = toolchain::find_binary_native(&["uv.exe"], &[]) {
                        run_hidden_cmd(root, &uv.to_string_lossy(), &["venv", ".venv"])?;
                        if req_txt.is_file() {
                            let venv_py = root.join(r".venv\Scripts\python.exe");
                            run_hidden_cmd(
                                root,
                                &uv.to_string_lossy(),
                                &[
                                    "pip",
                                    "install",
                                    "--python",
                                    &venv_py.to_string_lossy(),
                                    "-r",
                                    &req_txt.to_string_lossy(),
                                ],
                            )?;
                        }
                        Ok(())
                    } else {
                        let py_bin = toolchain::find_binary_native(&["python.exe"], &[])
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|| "python".to_string());
                        run_hidden_cmd(root, &py_bin, &["-m", "venv", ".venv"])?;
                        if req_txt.is_file() {
                            let venv_pip = root.join(r".venv\Scripts\pip.exe");
                            if venv_pip.is_file() {
                                run_hidden_cmd(
                                    root,
                                    &venv_pip.to_string_lossy(),
                                    &["install", "-r", &req_txt.to_string_lossy()],
                                )?;
                            }
                        }
                        Ok(())
                    }
                }
                "cfg:version_sync" => {
                    if !project.lock_file.trim().is_empty() {
                        if let Some(lk) = crate::build_guard::inspect_lock(&project.lock_file) {
                            if lk.exists && lk.alive {
                                return Err(anyhow::anyhow!(
                                    "外部构建进程正在占用构建锁（PID {}），请等待构建完成后再同步版本",
                                    lk.pid
                                ));
                            }
                        }
                    }
                    let ver_state = version::read_project_versions(root, &project.version_files);
                    let target = version::highest_valid_version(&ver_state)
                        .ok_or_else(|| anyhow::anyhow!("未找到可对齐的有效语义版本号"))?;
                    version::write_project_versions(root, &project.version_files, &target).map(|_| ())
                }
                "cfg:gitignore_release" => {
                    let rel_name = storage::normalize_project_rel_path(root, &project.release_dir)
                        .unwrap_or_else(|| "release".to_string());
                    let gi = root.join(".gitignore");
                    let mut existing = fs::read_to_string(&gi).unwrap_or_default();
                    let already_ignored = existing.lines().any(|l| {
                        let t = l.trim().trim_matches('/');
                        !t.starts_with('#') && t.eq_ignore_ascii_case(rel_name.trim_matches('/'))
                    });
                    if already_ignored {
                        Ok(())
                    } else {
                        if !existing.is_empty() && !existing.ends_with('\n') {
                            existing.push('\n');
                        }
                        existing.push_str(&format!("\n# ShipForge build artifacts\n{}/\n", rel_name));
                        crate::guardrails::atomic_write_bytes(&gi, existing.as_bytes())
                    }
                }
                "pub:git_repo" => run_hidden_cmd(root, "git", &["init"]).map(|_| ()),
                _ => Err(anyhow::anyhow!("该项需要手动处理：{}", item.name)),
            }
        };

        match res {
            Ok(()) => {
                succeeded.push(item.name.clone());
                emit_log(app, task_id, "stdout", &format!("✓ {} 补全完成", item.name));
            }
            Err(e) => {
                let msg = format!("{}（{}）", item.name, e);
                failed.push(msg.clone());
                emit_log(app, task_id, "stderr", &format!("✗ {}", msg));
            }
        }
    }

    let final_report = inspect_project_prerequisites(project, settings);
    emit_progress(
        app,
        task_id,
        "前置依赖补全",
        if failed.is_empty() { "补全完成" } else { "部分项失败" },
        &format!("成功 {} 项，失败 {} 项", succeeded.len(), failed.len()),
        100,
        false,
    );
    Ok(RepairResult {
        succeeded,
        failed,
        report: final_report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RepoBinding;

    #[test]
    fn test_go_project_preflight_does_not_require_rust_or_nsis() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_preflight_go_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(41)
        ));
        fs::create_dir_all(&dir).unwrap();
        let proj = ProjectConfig {
            id: "go-demo".into(),
            name: "GoDemo".into(),
            root: dir.to_string_lossy().to_string(),
            kind: "go".into(),
            adapter_label: "Go".into(),
            primary_language: "go".into(),
            detected_languages: vec!["go".into()],
            shell_type: "portable".into(),
            product_name: "GoDemo".into(),
            artifact_prefix: "GoDemo".into(),
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
        let rep = inspect_project_prerequisites(&proj, &GuardrailSettings::default());
        assert!(rep.items.iter().any(|i| i.id == "tool:go"));
        assert!(!rep.items.iter().any(|i| i.id == "tool:rustc"));
        assert!(!rep.items.iter().any(|i| i.id == "tool:makensis"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tauri_hybrid_python_engine_and_orthogonal_shell_prerequisites() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_preflight_hybrid_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(42)
        ));
        fs::create_dir_all(dir.join("src/engine")).unwrap();
        fs::create_dir_all(dir.join("src-tauri")).unwrap();
        fs::write(dir.join("src/engine/worker.py"), "print('ok')").unwrap();
        fs::write(dir.join("src/engine/requirements.txt"), "requests\n").unwrap();
        // Empty .venv without Scripts/python.exe must NOT be reported as ready!
        fs::create_dir_all(dir.join(".venv")).unwrap();
        fs::write(
            dir.join("src-tauri/tauri.conf.json"),
            r#"{"version":"1.0.0","bundle":{"active":true,"targets":["msi"]}}"#,
        )
        .unwrap();
        fs::write(dir.join(".gitignore"), "release/\n").unwrap();

        let mut proj = ProjectConfig::default();
        proj.id = "hybrid-demo".into();
        proj.name = "HybridDemo".into();
        proj.root = dir.to_string_lossy().to_string();
        proj.kind = "tauri".into();
        proj.primary_language = "rust".into();
        proj.shell_type = "tauri".into();
        proj.release_dir = "./release".into();
        proj.engine_src_dir = "src/engine".into();

        let tools = required_tools_for_project(&proj);
        assert!(tools.iter().any(|t| t.0 == "linker" && t.2), "Tauri must require native linker");
        assert!(tools.iter().any(|t| t.0 == "python" && t.2), "Hybrid Python engine must require python");
        // MSI-only Tauri app should NOT mark makensis as required
        assert!(tools.iter().any(|t| t.0 == "makensis" && !t.2), "MSI-only Tauri must not require makensis");
        // Non-git project without remote must NOT mark git as required
        assert!(tools.iter().any(|t| t.0 == "git" && !t.2), "Non-git project must not require git");

        let rep = inspect_project_prerequisites(&proj, &GuardrailSettings::default());
        let gi_item = rep.items.iter().find(|i| i.id == "cfg:gitignore_release").unwrap();
        assert_eq!(gi_item.status, "ok", "./release must match release/ in .gitignore");
        let venv_item = rep.items.iter().find(|i| i.id == "dep:python_venv").unwrap();
        assert_ne!(venv_item.status, "ok", "Empty .venv without python.exe must not report ok");

        let _ = fs::remove_dir_all(&dir);
    }
}
