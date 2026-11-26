use crate::detector::{build_ps1_actions, default_package_profiles_for_project, find_makensis_path, preferred_pwsh};
use crate::models::{ActionSpec, ProjectConfig, VersionFileSpec};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameworkPreset {
    pub id: String,
    pub name: String,
    pub language: String,
    pub shell: String,
    pub kind: String,
    pub summary: String,
    pub outputs: Vec<String>,
    pub requirements: Vec<String>,
    pub recommended: bool,
}

pub fn list_presets_for_project(project: &ProjectConfig) -> Vec<FrameworkPreset> {
    let cur_kind = project.kind.as_str();
    let cur_shell = project.shell_type.as_str();
    let cur_lang = project.primary_language.as_str();

    let mut list = vec![
        FrameworkPreset {
            id: "tauri-win".into(),
            name: "Tauri 2 桌面应用".into(),
            language: "rust".into(),
            shell: "tauri".into(),
            kind: "tauri".into(),
            summary: "Rust 内核 + Web 界面，一键产出 NSIS 安装包与单文件便携版".into(),
            outputs: vec!["主程序 EXE".into(), "NSIS 安装包".into(), "便携 ZIP".into()],
            requirements: vec!["Rust".into(), "Tauri CLI".into(), "NSIS".into(), "WebView2".into()],
            recommended: cur_kind == "tauri" || cur_shell == "tauri",
        },
        FrameworkPreset {
            id: "electron-win".into(),
            name: "Electron 桌面应用".into(),
            language: "node".into(),
            shell: "electron".into(),
            kind: "electron".into(),
            summary: "适配 electron-builder / Forge，自动识别 npm / pnpm / yarn / bun".into(),
            outputs: vec!["Windows 安装包".into(), "免安装便携目录".into()],
            requirements: vec!["Node.js".into(), "node_modules".into()],
            recommended: cur_kind == "electron" || cur_shell == "electron",
        },
        FrameworkPreset {
            id: "python-desktop".into(),
            name: "Python 独立桌面软件".into(),
            language: "python".into(),
            shell: "pyinstaller".into(),
            kind: "python".into(),
            summary: "自动优先使用项目 .venv / uv 环境，适配 PyInstaller 与 Wheel 分发".into(),
            outputs: vec!["独立 EXE".into(), "便携 ZIP".into(), "Wheel 包".into()],
            requirements: vec!["Python".into(), "uv / venv".into()],
            recommended: cur_kind == "python" || cur_lang == "python",
        },
        FrameworkPreset {
            id: "node-web".into(),
            name: "Web / Vite / Node 应用".into(),
            language: "node".into(),
            shell: "web".into(),
            kind: "node".into(),
            summary: "自动执行测试、类型检查与前端构建，输出精简 dist 静态压缩包".into(),
            outputs: vec!["dist 静态包".into(), "ZIP / TAR.GZ".into()],
            requirements: vec!["Node.js".into(), "npm / pnpm".into()],
            recommended: cur_kind == "node" && cur_shell != "electron",
        },
        FrameworkPreset {
            id: "rust-cli".into(),
            name: "Rust / Cargo 原生程序".into(),
            language: "rust".into(),
            shell: "portable".into(),
            kind: "rust".into(),
            summary: "内置 Clippy 检查、单元测试、Release 优化编译与便携压缩".into(),
            outputs: vec!["Release EXE".into(), "便携 ZIP".into()],
            requirements: vec!["Rust".into(), "Cargo".into()],
            recommended: cur_kind == "rust" && cur_shell != "tauri",
        },
        FrameworkPreset {
            id: "go-wails".into(),
            name: "Go / Wails 原生应用".into(),
            language: "go".into(),
            shell: if cur_kind == "wails" || cur_shell == "wails" { "wails".into() } else { "portable".into() },
            kind: if cur_kind == "wails" || cur_shell == "wails" { "wails".into() } else { "go".into() },
            summary: "自动裁剪符号表 (-s -w) 减小体积，支持 Wails 桌面壳或原生 Go 二进制".into(),
            outputs: vec!["单文件 EXE".into(), "NSIS / 便携包".into()],
            requirements: vec!["Go".into()],
            recommended: cur_kind == "go" || cur_kind == "wails" || cur_lang == "go",
        },
        FrameworkPreset {
            id: "dotnet-win".into(),
            name: ".NET / C# Win-x64 程序".into(),
            language: "dotnet".into(),
            shell: "portable".into(),
            kind: "dotnet".into(),
            summary: "内置 Release 自包含发布（dotnet publish win-x64）与单元测试".into(),
            outputs: vec!["Win-x64 发布目录".into(), "便携 ZIP".into()],
            requirements: vec![".NET SDK".into()],
            recommended: cur_kind == "dotnet" || cur_lang == "dotnet",
        },
        FrameworkPreset {
            id: "cpp-cmake".into(),
            name: "C / C++ CMake 工程".into(),
            language: "cpp".into(),
            shell: "portable".into(),
            kind: "cpp".into(),
            summary: "一键生成 Release 工程、多核编译、运行 CTest 并打包分发".into(),
            outputs: vec!["Release 二进制".into(), "便携 ZIP".into()],
            requirements: vec!["CMake".into(), "MinGW / MSVC".into()],
            recommended: cur_kind == "cpp" || cur_lang == "cpp",
        },
        FrameworkPreset {
            id: "java-jvm".into(),
            name: "Java Maven / Gradle 应用".into(),
            language: "java".into(),
            shell: "portable".into(),
            kind: "java".into(),
            summary: "自动识别 Wrapper (mvnw / gradlew)，完成测试与 JAR/分发包构建".into(),
            outputs: vec!["可执行 JAR".into(), "便携归档".into()],
            requirements: vec!["JDK".into(), "Maven / Gradle".into()],
            recommended: cur_kind == "java" || cur_lang == "java",
        },
    ];

    // Sort recommended preset first for immediate visibility
    list.sort_by_key(|p| !p.recommended);
    list
}

fn detect_pm(root: &Path) -> &'static str {
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

fn make_action(
    id: &str,
    label: &str,
    category: &str,
    command: &str,
    args: &[&str],
    cwd: &str,
    description: &str,
    dangerous: bool,
) -> ActionSpec {
    ActionSpec {
        id: id.into(),
        label: label.into(),
        category: category.into(),
        command: command.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: cwd.into(),
        description: description.into(),
        dangerous,
        supports_bump: false,
        supports_channel: false,
        supports_skip_tests: false,
    }
}

/// Apply a turnkey framework build preset adapted cleanly to the project's actual directory layout.
/// Preserves any user-created `custom-*` actions and existing `build.ps1` pipeline actions if present.
pub fn apply_preset_to_project(proj: &mut ProjectConfig, preset_id: &str) -> Result<(), String> {
    let root = Path::new(&proj.root);
    let custom_actions: Vec<ActionSpec> = proj
        .actions
        .iter()
        .filter(|a| a.id.starts_with("custom-"))
        .cloned()
        .collect();

    let mut actions: Vec<ActionSpec> = Vec::new();
    let mut version_files: Vec<VersionFileSpec> = proj.version_files.clone();
    let mut push_vf = |path: &str, fmt: &str| {
        if root.join(path).exists() && !version_files.iter().any(|v| v.path.eq_ignore_ascii_case(path)) {
            version_files.push(VersionFileSpec {
                path: path.into(),
                format: fmt.into(),
                pattern: String::new(),
            });
        }
    };

    match preset_id {
        "tauri-win" => {
            proj.kind = "tauri".into();
            proj.primary_language = "rust".into();
            proj.shell_type = "tauri".into();
            proj.adapter_label = "Tauri 2 桌面应用".into();

            let tauri_cwd = if root.join("src/src-tauri/tauri.conf.json").exists() {
                "src\\src-tauri"
            } else if root.join("src-tauri/tauri.conf.json").exists() {
                "src-tauri"
            } else {
                ""
            };
            let conf_rel = if tauri_cwd.is_empty() {
                "tauri.conf.json".into()
            } else {
                format!("{}\\tauri.conf.json", tauri_cwd)
            };
            let cargo_rel = if tauri_cwd.is_empty() {
                "Cargo.toml".into()
            } else {
                format!("{}\\Cargo.toml", tauri_cwd)
            };
            push_vf(&conf_rel, "tauri_json");
            push_vf(&cargo_rel, "cargo_toml");
            push_vf("package.json", "package_json");

            if root.join("build.ps1").exists() {
                let ps = preferred_pwsh();
                actions.extend(build_ps1_actions(root, &ps));
            } else {
                actions.push(make_action(
                    "tauri-build-nsis",
                    "编译并生成 NSIS 安装包",
                    "build",
                    "cargo-tauri",
                    &["build", "--bundles", "nsis"],
                    tauri_cwd,
                    "编译 Release 主程序并打包 Windows NSIS 安装包",
                    false,
                ));
                actions.push(make_action(
                    "tauri-build-exe",
                    "仅编译主程序 EXE",
                    "build",
                    "cargo-tauri",
                    &["build", "--no-bundle"],
                    tauri_cwd,
                    "跳过安装器快速输出可执行程序",
                    false,
                ));
                actions.push(make_action(
                    "cargo-test",
                    "运行 Rust 单元测试",
                    "test",
                    "cargo",
                    &["test"],
                    tauri_cwd,
                    "验证 Rust 后端逻辑与测试套件",
                    false,
                ));
                actions.push(make_action(
                    "cargo-clean",
                    "清理构建缓存",
                    "maintenance",
                    "cargo",
                    &["clean"],
                    tauri_cwd,
                    "清空 target 缓存目录",
                    true,
                ));
            }
        }
        "electron-win" => {
            let pm = detect_pm(root);
            proj.kind = "electron".into();
            proj.primary_language = "node".into();
            proj.shell_type = "electron".into();
            proj.adapter_label = format!("Electron 桌面应用 ({})", pm);
            push_vf("package.json", "package_json");

            actions.push(make_action(
                "electron-dist",
                "构建并打包安装包 (dist)",
                "package",
                pm,
                &["run", "dist"],
                "",
                "调用 package.json 的 dist 脚本生成安装包",
                false,
            ));
            actions.push(make_action(
                "electron-build",
                "编译应用资源 (build)",
                "build",
                pm,
                &["run", "build"],
                "",
                "编译前端与主进程代码",
                false,
            ));
            actions.push(make_action(
                "electron-test",
                "运行项目测试 (test)",
                "test",
                pm,
                &["test"],
                "",
                "运行测试套件",
                false,
            ));
        }
        "python-desktop" => {
            proj.kind = "python".into();
            proj.primary_language = "python".into();
            proj.shell_type = "pyinstaller".into();
            proj.adapter_label = "Python 独立桌面软件".into();
            push_vf("pyproject.toml", "pyproject_toml");

            let py = if root.join(".venv/Scripts/python.exe").exists() {
                ".venv\\Scripts\\python.exe"
            } else if root.join("venv/Scripts/python.exe").exists() {
                "venv\\Scripts\\python.exe"
            } else {
                "python"
            };
            let entry = if root.join("main.py").exists() {
                "main.py"
            } else if root.join("app.py").exists() {
                "app.py"
            } else {
                "src/main.py"
            };
            actions.push(make_action(
                "pyinstaller-onefile",
                "PyInstaller 单文件打包",
                "package",
                py,
                &["-m", "PyInstaller", "--noconfirm", "--onefile", "--distpath", "release", entry],
                "",
                "将入口脚本打包为单文件可执行程序至 release/",
                false,
            ));
            actions.push(make_action(
                "py-build-wheel",
                "构建 Wheel 分发包",
                "build",
                py,
                &["-m", "build"],
                "",
                "生成标准 Python .whl 与源码归档",
                false,
            ));
            actions.push(make_action(
                "py-test",
                "运行 Python 测试",
                "test",
                py,
                &["-m", "unittest", "discover"],
                "",
                "自动发现并执行单元测试",
                false,
            ));
        }
        "node-web" => {
            let pm = detect_pm(root);
            proj.kind = "node".into();
            proj.primary_language = "node".into();
            proj.shell_type = "web".into();
            proj.adapter_label = format!("Web / Node 应用 ({})", pm);
            push_vf("package.json", "package_json");

            actions.push(make_action(
                "web-build",
                "构建生产版本 (build)",
                "build",
                pm,
                &["run", "build"],
                "",
                "输出生产环境静态资源到 dist/",
                false,
            ));
            actions.push(make_action(
                "web-test",
                "运行测试 (test)",
                "test",
                pm,
                &["test"],
                "",
                "运行单元测试与校验",
                false,
            ));
            actions.push(make_action(
                "web-lint",
                "代码规范检查 (lint)",
                "test",
                pm,
                &["run", "lint"],
                "",
                "执行静态代码规范检查",
                false,
            ));
        }
        "rust-cli" => {
            proj.kind = "rust".into();
            proj.primary_language = "rust".into();
            proj.shell_type = "portable".into();
            proj.adapter_label = "Rust / Cargo 原生程序".into();
            push_vf("Cargo.toml", "cargo_toml");

            actions.push(make_action(
                "cargo-build-release",
                "编译 Release 程序",
                "build",
                "cargo",
                &["build", "--release"],
                "",
                "以发布模式优化编译二进制文件",
                false,
            ));
            actions.push(make_action(
                "cargo-test",
                "运行全部测试",
                "test",
                "cargo",
                &["test"],
                "",
                "运行单元与集成测试",
                false,
            ));
            actions.push(make_action(
                "cargo-clippy",
                "Clippy 代码检查",
                "test",
                "cargo",
                &["clippy", "--all-targets"],
                "",
                "检查潜在缺陷与性能隐患",
                false,
            ));
            actions.push(make_action(
                "cargo-clean",
                "清理编译缓存",
                "maintenance",
                "cargo",
                &["clean"],
                "",
                "删除 target 缓存目录",
                true,
            ));
        }
        "go-wails" => {
            let is_wails = root.join("wails.json").exists();
            proj.kind = if is_wails { "wails".into() } else { "go".into() };
            proj.primary_language = "go".into();
            proj.shell_type = if is_wails { "wails".into() } else { "portable".into() };
            proj.adapter_label = if is_wails {
                "Wails 桌面应用 (Go)".into()
            } else {
                "Go 原生程序".into()
            };
            if is_wails {
                actions.push(make_action(
                    "wails-build-nsis",
                    "构建 Wails + NSIS 安装包",
                    "package",
                    "wails",
                    &["build", "-nsis"],
                    "",
                    "编译并生成 Windows 安装程序",
                    false,
                ));
                actions.push(make_action(
                    "wails-build",
                    "仅构建 Wails 主程序",
                    "build",
                    "wails",
                    &["build"],
                    "",
                    "快速输出桌面可执行程序",
                    false,
                ));
            } else {
                let out = format!("release\\{}.exe", proj.artifact_prefix);
                actions.push(ActionSpec {
                    id: "go-build-release".into(),
                    label: "编译精简 Release 二进制".into(),
                    category: "build".into(),
                    command: "go".into(),
                    args: vec![
                        "build".into(),
                        "-trimpath".into(),
                        "-ldflags".into(),
                        "-s -w".into(),
                        "-o".into(),
                        out,
                        ".".into(),
                    ],
                    cwd: String::new(),
                    description: "去除调试符号减小体积，直接输出至 release/".into(),
                    dangerous: false,
                    supports_bump: false,
                    supports_channel: false,
                    supports_skip_tests: false,
                });
            }
            actions.push(make_action(
                "go-test",
                "运行 Go 测试",
                "test",
                "go",
                &["test", "./..."],
                "",
                "执行全部 Go 单元测试",
                false,
            ));
        }
        "dotnet-win" => {
            proj.kind = "dotnet".into();
            proj.primary_language = "dotnet".into();
            proj.shell_type = "portable".into();
            proj.adapter_label = ".NET / C# Win-x64 程序".into();

            actions.push(make_action(
                "dotnet-publish-sc",
                "发布 Win-x64 自包含单文件",
                "package",
                "dotnet",
                &[
                    "publish",
                    "-c",
                    "Release",
                    "-r",
                    "win-x64",
                    "--self-contained",
                    "true",
                    "-p:PublishSingleFile=true",
                    "-o",
                    "release\\win-x64",
                ],
                "",
                "产出免装运行时的单文件程序到 release\\win-x64",
                false,
            ));
            actions.push(make_action(
                "dotnet-test",
                "运行单元测试",
                "test",
                "dotnet",
                &["test"],
                "",
                "执行项目测试集",
                false,
            ));
        }
        "cpp-cmake" => {
            proj.kind = "cpp".into();
            proj.primary_language = "cpp".into();
            proj.shell_type = "portable".into();
            proj.adapter_label = "C / C++ CMake 工程".into();

            actions.push(make_action(
                "cmake-config",
                "配置 CMake Release 工程",
                "build",
                "cmake",
                &["-B", "build", "-DCMAKE_BUILD_TYPE=Release"],
                "",
                "在 build/ 目录生成 Release 构建配置",
                false,
            ));
            actions.push(make_action(
                "cmake-build",
                "多核编译 Release 目标",
                "build",
                "cmake",
                &["--build", "build", "--config", "Release", "--parallel"],
                "",
                "多核并行编译 C/C++ 发布程序",
                false,
            ));
            actions.push(make_action(
                "ctest-run",
                "运行 CTest 测试",
                "test",
                "ctest",
                &["--test-dir", "build", "--output-on-failure"],
                "",
                "运行已注册的 CTest 用例",
                false,
            ));
        }
        "java-jvm" => {
            proj.kind = "java".into();
            proj.primary_language = "java".into();
            proj.shell_type = "portable".into();
            let is_gradle = root.join("build.gradle").exists() || root.join("build.gradle.kts").exists();
            proj.adapter_label = if is_gradle {
                "Java / Gradle 应用".into()
            } else {
                "Java / Maven 应用".into()
            };
            if is_gradle {
                let cmd = if root.join("gradlew.bat").exists() { "gradlew.bat" } else { "gradle.bat" };
                actions.push(make_action("gradle-build", "Gradle 构建与打包", "package", cmd, &["build"], "", "编译并输出分发 JAR", false));
                actions.push(make_action("gradle-test", "运行 Gradle 测试", "test", cmd, &["test"], "", "执行单元测试", false));
            } else {
                let cmd = if root.join("mvnw.cmd").exists() { "mvnw.cmd" } else { "mvn.cmd" };
                actions.push(make_action("mvn-package", "Maven 打包 (package)", "package", cmd, &["package"], "", "编译测试并打包 JAR", false));
                actions.push(make_action("mvn-test", "运行 Maven 测试", "test", cmd, &["test"], "", "执行单元测试", false));
            }
        }
        other => return Err(anyhow::anyhow!("未知预设模板：{}", other).to_string()),
    }

    // Also attach NSIS action automatically if a .nsi script sits in project root
    if let Ok(rd) = std::fs::read_dir(root) {
        if let Some(nsi) = rd.flatten().find_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            if n.to_ascii_lowercase().ends_with(".nsi") { Some(n) } else { None }
        }) {
            let nsis_bin = find_makensis_path().unwrap_or_else(|| "makensis".into());
            if !actions.iter().any(|a| a.id.contains("nsis")) {
                actions.push(ActionSpec {
                    id: "nsis-script".into(),
                    label: format!("编译安装包脚本 ({})", nsi),
                    category: "package".into(),
                    command: nsis_bin,
                    args: vec![nsi],
                    cwd: String::new(),
                    description: "使用 NSIS 编译生成 Windows 安装程序".into(),
                    dangerous: false,
                    supports_bump: false,
                    supports_channel: false,
                    supports_skip_tests: false,
                });
            }
        }
    }

    actions.extend(custom_actions);
    proj.actions = actions;
    proj.version_files = version_files;
    if proj.package_profiles.is_empty() {
        proj.package_profiles = default_package_profiles_for_project(
            &proj.artifact_prefix,
            &proj.launch_exe,
            &proj.engine_exe_path,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RepoBinding;
    use std::collections::HashMap;

    #[test]
    fn test_apply_framework_presets_adapt_and_preserve_custom_actions() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_preset_test_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(71)
        ));
        std::fs::create_dir_all(dir.join("src-tauri")).unwrap();
        std::fs::write(dir.join("src-tauri/tauri.conf.json"), r#"{"version":"1.0.0"}"#).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"name":"app","version":"1.0.0"}"#).unwrap();

        let mut proj = ProjectConfig {
            id: "p".into(),
            name: "Demo".into(),
            root: dir.to_string_lossy().to_string(),
            kind: "custom".into(),
            adapter_label: "Custom".into(),
            primary_language: "custom".into(),
            detected_languages: vec![],
            shell_type: "portable".into(),
            product_name: "Demo".into(),
            artifact_prefix: "Demo".into(),
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
            actions: vec![make_action("custom-my", "我的自定义脚本", "custom", "echo", &["hi"], "", "", false)],
            package_profiles: vec![],
        };

        let presets = list_presets_for_project(&proj);
        assert_eq!(presets.len(), 9);

        apply_preset_to_project(&mut proj, "tauri-win").unwrap();
        assert_eq!(proj.kind, "tauri");
        assert!(proj.actions.iter().any(|a| a.id == "tauri-build-nsis"));
        assert!(proj.actions.iter().any(|a| a.id == "custom-my"));
        assert!(proj.version_files.iter().any(|v| v.format == "tauri_json"));

        apply_preset_to_project(&mut proj, "python-desktop").unwrap();
        assert_eq!(proj.kind, "python");
        assert!(proj.actions.iter().any(|a| a.id == "pyinstaller-onefile"));
        assert!(proj.actions.iter().any(|a| a.id == "custom-my"));

        // Tauri preset with build.ps1 MUST introspect real switches instead of hardcoding -Channel/-TestsOnly
        std::fs::write(dir.join("build.ps1"), "param([switch]$AppOnly, [switch]$Clean, [switch]$TestsOnly)\n").unwrap();
        apply_preset_to_project(&mut proj, "tauri-win").unwrap();
        assert!(proj.actions.iter().any(|a| a.id == "build-app-only"), "must preserve introspected -AppOnly");
        assert!(proj.actions.iter().any(|a| a.id == "build-clean"), "must preserve introspected -Clean");
        let full = proj.actions.iter().find(|a| a.id == "build-full").unwrap();
        assert!(!full.supports_channel, "must not fabricate -Channel when build.ps1 lacks $Channel");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
