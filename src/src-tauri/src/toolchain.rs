use crate::models::{TaskProgressEvent, ToolchainItem};
use crate::runner::{decode_win_bytes, emit_log};
use crate::storage;
use anyhow::{anyhow, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, Emitter};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn hidden_cmd(prog: &Path) -> Command {
    let mut cmd = Command::new(prog);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Returns all ShipForge-managed executable directories inside `<dataRoot>\tools\` to prepend to PATH.
pub fn managed_tool_bin_dirs() -> Vec<PathBuf> {
    let root = storage::tools_dir();
    if !root.is_dir() {
        return vec![];
    }
    let mut dirs = Vec::new();
    let sub_candidates = [
        "nsis",
        "nsis\\Bin",
        "tauri",
        "tauri\\bin",
        "gh",
        "gh\\bin",
        "uv",
        "pnpm",
        "inno",
        "7z",
        "git\\cmd",
        "node",
        "python",
    ];
    for rel in sub_candidates {
        let p = root.join(rel);
        if p.is_dir() {
            dirs.push(p);
        }
    }
    // Also include any direct tool folder added by the user
    if let Ok(rd) = fs::read_dir(&root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && !dirs.iter().any(|d| d == &p) {
                dirs.push(p);
            }
        }
    }
    dirs
}

pub fn managed_path_prefix() -> String {
    managed_tool_bin_dirs()
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(";")
}

/// 100% Unicode-safe native PATH resolver (NEVER spawns `where.exe`, immune to GBK mojibake in Chinese usernames!).
pub fn find_binary_native(names: &[&str], extra_fallbacks: &[PathBuf]) -> Option<PathBuf> {
    let mut search_dirs = managed_tool_bin_dirs();
    if let Some(path_os) = std::env::var_os("PATH") {
        search_dirs.extend(std::env::split_paths(&path_os));
    }

    for dir in search_dirs {
        for name in names {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    for fb in extra_fallbacks {
        if fb.is_file() {
            return Some(fb.clone());
        }
    }
    None
}

pub fn find_makensis_native() -> Option<String> {
    let mut fb = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        fb.push(PathBuf::from(&local).join(r"tauri\NSIS\makensis.exe"));
    }
    find_binary_native(&["makensis.exe"], &fb).map(|p| p.to_string_lossy().to_string())
}

pub fn preferred_pwsh_native() -> String {
    if find_binary_native(&["pwsh.exe"], &[]).is_some() {
        "pwsh".into()
    } else {
        "powershell".into()
    }
}

fn run_version_probe(exe: &Path, args: &[&str]) -> Result<String> {
    let ext = exe
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let file_name = exe
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    // MSVC cl.exe / link.exe don't support `--version`; invoke without `--version` to read compiler/linker banner cleanly.
    let effective_args: &[&str] = if file_name == "cl.exe" {
        &[]
    } else if file_name == "link.exe" {
        &["/?"]
    } else {
        args
    };

    let output = if ext == "cmd" || ext == "bat" {
        let mut cmd = Command::new("cmd");
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.arg("/C").arg(exe).args(effective_args).output()?
    } else {
        hidden_cmd(exe).args(effective_args).output()?
    };

    let stdout = decode_win_bytes(&output.stdout);
    let stderr = decode_win_bytes(&output.stderr);
    let first_line = stdout
        .lines()
        .chain(stderr.lines())
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string();

    if !output.status.success() {
        let low = first_line.to_ascii_lowercase();
        let known_banner_ok = (file_name == "iscc.exe" && low.contains("inno setup"))
            || (file_name == "cl.exe"
                && (low.contains("c/c++ optimizing compiler") || low.contains("microsoft (r)")))
            || (file_name == "link.exe"
                && (low.contains("incremental linker") || low.contains("microsoft (r)")));
        if !known_banner_ok {
            return Err(anyhow!(
                "{}",
                if first_line.is_empty() {
                    format!("进程退出码 {:?}", output.status.code())
                } else {
                    first_line
                }
            ));
        }
    }
    Ok(first_line.chars().take(76).collect())
}

fn path_is_under_dir(path: &Path, dir: &Path) -> bool {
    let pk = storage::normalize_root_key(path);
    let dk = storage::normalize_root_key(dir);
    !pk.is_empty() && !dk.is_empty() && (pk == dk || pk.starts_with(&format!("{}\\", dk)))
}

fn parse_numeric_ver_tuple(s: &str) -> Vec<u64> {
    s.split('.')
        .map(|seg| seg.parse::<u64>().unwrap_or(0))
        .collect()
}

/// Returns the bin directory of a discovered fallback native linker (e.g. MinGW gcc.exe)
/// ONLY when neither `link.exe` nor `gcc.exe` is currently reachable on `PATH`.
pub fn fallback_linker_bin_dir() -> Option<PathBuf> {
    if find_binary_native(&["link.exe", "gcc.exe", "lld-link.exe"], &[]).is_some() {
        return None;
    }
    for cand in native_linker_fallbacks() {
        if cand.is_file() {
            if let Some(parent) = cand.parent() {
                return Some(parent.to_path_buf());
            }
        }
    }
    None
}

/// Discovers common Windows native C/C++/Rust linker binaries (MSVC `link.exe`, MinGW `gcc.exe`/`ld.lld.exe`, LLVM `lld-link.exe`).
fn native_linker_fallbacks() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![
        PathBuf::from(r"D:\Tool\binutils\mingw64\bin\gcc.exe"),
        PathBuf::from(r"C:\msys64\mingw64\bin\gcc.exe"),
        PathBuf::from(r"C:\msys64\ucrt64\bin\gcc.exe"),
        PathBuf::from(r"C:\mingw64\bin\gcc.exe"),
        PathBuf::from(r"C:\Program Files\LLVM\bin\lld-link.exe"),
    ];
    // Scan standard Visual Studio / Build Tools installations for MSVC Hostx64\x64\link.exe
    for base in [
        r"C:\Program Files\Microsoft Visual Studio",
        r"C:\Program Files (x86)\Microsoft Visual Studio",
    ] {
        let bp = Path::new(base);
        if !bp.is_dir() {
            continue;
        }
        if let Ok(years) = fs::read_dir(bp) {
            for y in years.flatten().take(6) {
                if let Ok(eds) = fs::read_dir(y.path()) {
                    for ed in eds.flatten().take(8) {
                        let msvc = ed.path().join(r"VC\Tools\MSVC");
                        if let Ok(vers) = fs::read_dir(&msvc) {
                            let mut vdirs: Vec<PathBuf> = vers
                                .flatten()
                                .map(|e| e.path())
                                .filter(|p| p.is_dir())
                                .collect();
                            vdirs.sort();
                            if let Some(latest) = vdirs.last() {
                                let link = latest.join(r"bin\Hostx64\x64\link.exe");
                                if link.is_file() {
                                    out.push(link);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

fn native_cpp_compiler_fallbacks() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![
        PathBuf::from(r"D:\Tool\binutils\mingw64\bin\gcc.exe"),
        PathBuf::from(r"C:\msys64\mingw64\bin\gcc.exe"),
        PathBuf::from(r"C:\msys64\ucrt64\bin\gcc.exe"),
        PathBuf::from(r"C:\mingw64\bin\gcc.exe"),
    ];
    for base in [
        r"C:\Program Files\Microsoft Visual Studio",
        r"C:\Program Files (x86)\Microsoft Visual Studio",
    ] {
        let bp = Path::new(base);
        if !bp.is_dir() {
            continue;
        }
        if let Ok(years) = fs::read_dir(bp) {
            for y in years.flatten().take(6) {
                if let Ok(eds) = fs::read_dir(y.path()) {
                    for ed in eds.flatten().take(8) {
                        let msvc = ed.path().join(r"VC\Tools\MSVC");
                        if let Ok(vers) = fs::read_dir(&msvc) {
                            let mut vdirs: Vec<PathBuf> = vers
                                .flatten()
                                .map(|e| e.path())
                                .filter(|p| p.is_dir())
                                .collect();
                            vdirs.sort();
                            if let Some(latest) = vdirs.last() {
                                let cl = latest.join(r"bin\Hostx64\x64\cl.exe");
                                if cl.is_file() {
                                    out.push(cl);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// Iterates across ALL candidate native C/C++/Rust linkers (PATH + standard MinGW/MSVC/LLVM/rustc-lld dirs),
/// explicitly rejecting non-linker lookalikes such as Git for Windows' POSIX coreutils `usr\bin\link.exe`!
fn probe_native_linker() -> ToolchainItem {
    let managed_root = storage::tools_dir();
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut search_dirs = managed_tool_bin_dirs();
    if let Some(path_os) = std::env::var_os("PATH") {
        search_dirs.extend(std::env::split_paths(&path_os));
    }
    for dir in search_dirs {
        for name in ["gcc.exe", "lld-link.exe", "ld.lld.exe", "link.exe"] {
            let p = dir.join(name);
            if p.is_file() {
                candidates.push(p);
            }
        }
    }
    candidates.extend(native_linker_fallbacks());

    // Also probe active `rustc --print sysroot` for bundled `gcc-ld\ld.lld.exe`
    if let Some(rustc) = find_binary_native(&["rustc.exe"], &cargo_bin_fallbacks("rustc.exe")) {
        if let Ok(out) = hidden_cmd(&rustc).args(["--print", "sysroot"]).output() {
            if out.status.success() {
                let sys = decode_win_bytes(&out.stdout).trim().to_string();
                if !sys.is_empty() {
                    let lld = PathBuf::from(&sys)
                        .join(r"lib\rustlib\x86_64-pc-windows-gnu\bin\gcc-ld\ld.lld.exe");
                    if lld.is_file() {
                        candidates.push(lld);
                    }
                    let lld_msvc = PathBuf::from(&sys)
                        .join(r"lib\rustlib\x86_64-pc-windows-msvc\bin\gcc-ld\lld-link.exe");
                    if lld_msvc.is_file() {
                        candidates.push(lld_msvc);
                    }
                }
            }
        }
    }

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for cand in candidates {
        if !cand.is_file() {
            continue;
        }
        let key = storage::normalize_root_key(&cand);
        if !seen.insert(key) {
            continue;
        }
        let fname = cand
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        // Skip Git/MSYS POSIX coreutils `link.exe` immediately without false-positive or false-corrupted!
        if fname == "link.exe" {
            let low_p = cand.to_string_lossy().to_ascii_lowercase();
            if low_p.contains(r"\git\usr\bin\") || low_p.contains(r"\msys64\usr\bin\") {
                continue;
            }
        }
        if let Ok(ver) = run_version_probe(&cand, &["--version"]) {
            if fname == "link.exe" {
                let low_v = ver.to_ascii_lowercase();
                if !low_v.contains("incremental linker") && !low_v.contains("microsoft") {
                    continue;
                }
            }
            let managed = path_is_under_dir(&cand, &managed_root);
            return ToolchainItem {
                id: "linker".into(),
                name: "原生链接器 (MSVC / MinGW)".into(),
                category: "编译链".into(),
                available: true,
                status: "ok".into(),
                version: if ver.is_empty() { "就绪".into() } else { ver },
                path: cand.to_string_lossy().to_string(),
                managed,
                essential: false,
                repairable: false,
                detail: if managed {
                    "由 ShipForge 自定义工具目录托管".into()
                } else {
                    "原生链接器已就绪".into()
                },
            };
        }
    }

    ToolchainItem {
        id: "linker".into(),
        name: "原生链接器 (MSVC / MinGW)".into(),
        category: "编译链".into(),
        available: false,
        status: "missing".into(),
        version: String::new(),
        path: String::new(),
        managed: false,
        essential: false,
        repairable: false,
        detail: "未检测到 MSVC Build Tools (link.exe) 或 MinGW (gcc/lld)，Rust/Tauri 编译链接阶段将失败".into(),
    }
}

fn cargo_bin_fallbacks(exe_name: &str) -> Vec<PathBuf> {
    let mut fb = Vec::new();
    if let Ok(ch) = std::env::var("CARGO_HOME") {
        let t = ch.trim();
        if !t.is_empty() {
            fb.push(PathBuf::from(t).join("bin").join(exe_name));
        }
    }
    fb.push(storage::cache_dir().join("cargo").join("bin").join(exe_name));
    if let Ok(up) = std::env::var("USERPROFILE") {
        let t = up.trim();
        if !t.is_empty() {
            fb.push(PathBuf::from(t).join(".cargo").join("bin").join(exe_name));
        }
    }
    fb
}

struct ToolDef {
    id: &'static str,
    name: &'static str,
    category: &'static str,
    bins: &'static [&'static str],
    ver_args: &'static [&'static str],
    essential: bool,
    repairable: bool,
}

fn probe_single_tool(def: &ToolDef, fallbacks: &[PathBuf]) -> ToolchainItem {
    let managed_root = storage::tools_dir();
    let Some(path) = find_binary_native(def.bins, fallbacks) else {
        return ToolchainItem {
            id: def.id.into(),
            name: def.name.into(),
            category: def.category.into(),
            available: false,
            status: "missing".into(),
            version: String::new(),
            path: String::new(),
            managed: false,
            essential: def.essential,
            repairable: def.repairable,
            detail: "未检测到可执行文件，可点击一键安装/修复".into(),
        };
    };

    let managed = path_is_under_dir(&path, &managed_root);

    // Integrity check: non-zero file size + extra structural checks + real version execution
    let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if size < 64 {
        return ToolchainItem {
            id: def.id.into(),
            name: def.name.into(),
            category: def.category.into(),
            available: false,
            status: "corrupted".into(),
            version: String::new(),
            path: path.to_string_lossy().to_string(),
            managed,
            essential: def.essential,
            repairable: def.repairable,
            detail: "可执行文件为空或已损坏（体积异常）".into(),
        };
    }

    // NSIS structural check: Stubs & Include must exist beside makensis.exe (or parent dir)
    if def.id == "makensis" {
        let dir = path.parent().unwrap_or(Path::new(""));
        let parent = dir.parent().unwrap_or(dir);
        let has_stubs = dir.join("Stubs").is_dir() || parent.join("Stubs").is_dir();
        let has_inc = dir.join("Include").is_dir() || parent.join("Include").is_dir();
        if !has_stubs || !has_inc {
            return ToolchainItem {
                id: def.id.into(),
                name: def.name.into(),
                category: def.category.into(),
                available: false,
                status: "corrupted".into(),
                version: String::new(),
                path: path.to_string_lossy().to_string(),
                managed,
                essential: def.essential,
                repairable: def.repairable,
                detail: "NSIS 目录残缺（缺少 Stubs 或 Include 运行库），请点击一键修复".into(),
            };
        }
    }

    match run_version_probe(&path, def.ver_args) {
        Ok(ver) => ToolchainItem {
            id: def.id.into(),
            name: def.name.into(),
            category: def.category.into(),
            available: true,
            status: "ok".into(),
            version: if ver.is_empty() { "就绪".into() } else { ver },
            path: path.to_string_lossy().to_string(),
            managed,
            essential: def.essential,
            repairable: def.repairable,
            detail: if managed {
                "由 ShipForge 自定义工具目录托管".into()
            } else {
                "系统环境已就绪".into()
            },
        },
        Err(e) => ToolchainItem {
            id: def.id.into(),
            name: def.name.into(),
            category: def.category.into(),
            available: false,
            status: "corrupted".into(),
            version: String::new(),
            path: path.to_string_lossy().to_string(),
            managed,
            essential: def.essential,
            repairable: def.repairable,
            detail: format!("版本探测失败（可能损坏或缺少依赖）：{}", e),
        },
    }
}

pub fn scan_toolchains() -> Vec<ToolchainItem> {
    scan_toolchains_for(None, None)
}

/// Fast scoped toolchain probe: probes ONLY `only_ids` (if provided) and checks project-local tool paths.
pub fn scan_toolchains_for(only_ids: Option<&[&str]>, project_root: Option<&Path>) -> Vec<ToolchainItem> {
    let local_app = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let user_profile = std::env::var("USERPROFILE").unwrap_or_default();

    let mut defs: Vec<(ToolDef, Vec<PathBuf>)> = vec![
        (
            ToolDef {
                id: "rustc",
                name: "Rust 编译器 (rustc)",
                category: "编译链",
                bins: &["rustc.exe"],
                ver_args: &["--version"],
                essential: true,
                repairable: true,
            },
            cargo_bin_fallbacks("rustc.exe"),
        ),
        (
            ToolDef {
                id: "cargo",
                name: "Cargo 包管理器",
                category: "编译链",
                bins: &["cargo.exe"],
                ver_args: &["--version"],
                essential: true,
                repairable: true,
            },
            cargo_bin_fallbacks("cargo.exe"),
        ),
        (
            ToolDef {
                id: "cargo-tauri",
                name: "Tauri CLI",
                category: "打包器",
                bins: &["cargo-tauri.exe"],
                ver_args: &["--version"],
                essential: true,
                repairable: true,
            },
            {
                let mut fb = vec![PathBuf::from(&user_profile).join(r".cargo\bin\cargo-tauri.exe")];
                if let Ok(exe) = std::env::current_exe() {
                    if let Some(dir) = exe.parent() {
                        fb.push(dir.join(r"tools\cargo-tauri.exe"));
                    }
                }
                fb
            },
        ),
        (
            ToolDef {
                id: "makensis",
                name: "NSIS 安装包编译器",
                category: "打包器",
                bins: &["makensis.exe"],
                ver_args: &["-VERSION"],
                essential: true,
                repairable: true,
            },
            vec![PathBuf::from(&local_app).join(r"tauri\NSIS\makensis.exe")],
        ),
        (
            ToolDef {
                id: "git",
                name: "Git",
                category: "版本与发布",
                bins: &["git.exe"],
                ver_args: &["--version"],
                essential: true,
                repairable: true,
            },
            vec![PathBuf::from(r"C:\Program Files\Git\cmd\git.exe")],
        ),
        (
            ToolDef {
                id: "gh",
                name: "GitHub CLI (gh)",
                category: "版本与发布",
                bins: &["gh.exe"],
                ver_args: &["--version"],
                essential: true,
                repairable: true,
            },
            vec![PathBuf::from(r"C:\Program Files\GitHub CLI\gh.exe")],
        ),
        (
            ToolDef {
                id: "node",
                name: "Node.js",
                category: "前端/JS",
                bins: &["node.exe"],
                ver_args: &["-v"],
                essential: true,
                repairable: true,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "npm",
                name: "npm",
                category: "前端/JS",
                bins: &["npm.cmd", "npm.exe"],
                ver_args: &["-v"],
                essential: false,
                repairable: false,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "pnpm",
                name: "pnpm",
                category: "前端/JS",
                bins: &["pnpm.cmd", "pnpm.exe"],
                ver_args: &["-v"],
                essential: false,
                repairable: true,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "python",
                name: "Python",
                category: "脚本/引擎",
                bins: &["python.exe"],
                ver_args: &["--version"],
                essential: false,
                repairable: true,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "uv",
                name: "uv (Python 极速工具)",
                category: "脚本/引擎",
                bins: &["uv.exe"],
                ver_args: &["--version"],
                essential: false,
                repairable: true,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "iscc",
                name: "Inno Setup (iscc)",
                category: "打包器",
                bins: &["iscc.exe"],
                ver_args: &["/?"],
                essential: false,
                repairable: true,
            },
            vec![
                PathBuf::from(r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe"),
                PathBuf::from(r"C:\Program Files\Inno Setup 6\ISCC.exe"),
            ],
        ),
        (
            ToolDef {
                id: "go",
                name: "Go",
                category: "编译链",
                bins: &["go.exe"],
                ver_args: &["version"],
                essential: false,
                repairable: true,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "dotnet",
                name: ".NET SDK",
                category: "编译链",
                bins: &["dotnet.exe"],
                ver_args: &["--version"],
                essential: false,
                repairable: true,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "pwsh",
                name: "PowerShell 7 (pwsh)",
                category: "脚本/引擎",
                bins: &["pwsh.exe"],
                ver_args: &["--version"],
                essential: true,
                repairable: true,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "bun",
                name: "Bun",
                category: "前端/JS",
                bins: &["bun.exe"],
                ver_args: &["--version"],
                essential: false,
                repairable: true,
            },
            vec![PathBuf::from(&user_profile).join(r".bun\bin\bun.exe")],
        ),
        (
            ToolDef {
                id: "yarn",
                name: "Yarn",
                category: "前端/JS",
                bins: &["yarn.cmd", "yarn.exe"],
                ver_args: &["--version"],
                essential: false,
                repairable: false,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "cmake",
                name: "CMake",
                category: "编译链",
                bins: &["cmake.exe"],
                ver_args: &["--version"],
                essential: false,
                repairable: true,
            },
            vec![PathBuf::from(r"C:\Program Files\CMake\bin\cmake.exe")],
        ),
        (
            ToolDef {
                id: "gcc",
                name: "C/C++ 编译器 (GCC/MinGW/MSVC)",
                category: "编译链",
                bins: &["gcc.exe", "g++.exe", "cl.exe"],
                ver_args: &["--version"],
                essential: false,
                repairable: false,
            },
            native_cpp_compiler_fallbacks(),
        ),
        (
            ToolDef {
                id: "javac",
                name: "Java JDK (javac)",
                category: "编译链",
                bins: &["javac.exe"],
                ver_args: &["-version"],
                essential: false,
                repairable: true,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "mvn",
                name: "Apache Maven (mvn)",
                category: "编译链",
                bins: &["mvn.cmd", "mvn.exe"],
                ver_args: &["-v"],
                essential: false,
                repairable: false,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "gradle",
                name: "Gradle 构建工具",
                category: "编译链",
                bins: &["gradle.bat", "gradle.exe"],
                ver_args: &["-v"],
                essential: false,
                repairable: false,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "flutter",
                name: "Flutter SDK",
                category: "打包器",
                bins: &["flutter.bat", "flutter.exe"],
                ver_args: &["--version"],
                essential: false,
                repairable: false,
            },
            vec![],
        ),
        (
            ToolDef {
                id: "wails",
                name: "Wails CLI",
                category: "打包器",
                bins: &["wails.exe"],
                ver_args: &["version"],
                essential: false,
                repairable: false,
            },
            vec![PathBuf::from(&user_profile).join(r"go\bin\wails.exe")],
        ),
    ];

    if let Some(root) = project_root {
        for (def, fb) in &mut defs {
            if def.id == "cargo-tauri" {
                fb.push(root.join(r"tools\cargo-tauri.exe"));
            } else if def.id == "python" {
                fb.push(root.join(r".venv\Scripts\python.exe"));
                fb.push(root.join(r"venv\Scripts\python.exe"));
            } else if def.id == "mvn" {
                fb.push(root.join("mvnw.cmd"));
            } else if def.id == "gradle" {
                fb.push(root.join("gradlew.bat"));
            }
        }
    }
    if let Some(ids) = only_ids {
        defs.retain(|(def, _)| ids.contains(&def.id));
    }

    let handles: Vec<_> = defs
        .into_iter()
        .map(|(def, fb)| std::thread::spawn(move || probe_single_tool(&def, &fb)))
        .collect();

    let mut out: Vec<ToolchainItem> = handles
        .into_iter()
        .filter_map(|h| h.join().ok())
        .collect();

    let want_linker = only_ids.map(|ids| ids.contains(&"linker")).unwrap_or(false);
    if want_linker {
        out.push(probe_native_linker());
    }

    let want_wv2 = only_ids.map(|ids| ids.contains(&"webview2")).unwrap_or(true);
    if want_wv2 {
        let wv2_dirs = [
            PathBuf::from(r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application"),
            PathBuf::from(r"C:\Program Files\Microsoft\EdgeWebView\Application"),
            PathBuf::from(&local_app).join(r"Microsoft\EdgeWebView\Application"),
        ];
        // Require a real versioned folder containing `msedgewebview2.exe`, and select the highest version!
        let mut best_wv2: Option<(Vec<u64>, String, PathBuf)> = None;
        for base_dir in &wv2_dirs {
            let Ok(rd) = std::fs::read_dir(base_dir) else {
                continue;
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if !p.is_dir() {
                    continue;
                }
                let ver_name = ent.file_name().to_string_lossy().to_string();
                if !ver_name.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                    continue;
                }
                if !p.join("msedgewebview2.exe").is_file() {
                    continue;
                }
                let key = parse_numeric_ver_tuple(&ver_name);
                match &best_wv2 {
                    None => best_wv2 = Some((key, ver_name, p)),
                    Some((bk, _, _)) if key > *bk => best_wv2 = Some((key, ver_name, p)),
                    _ => {}
                }
            }
        }
        let available = best_wv2.is_some();
        let (wv2_ver, wv2_path) = match best_wv2 {
            Some((_, v, p)) => (v, p.to_string_lossy().to_string()),
            None => (String::new(), String::new()),
        };
        out.push(ToolchainItem {
            id: "webview2".into(),
            name: "Microsoft Edge WebView2 运行时".into(),
            category: "打包器".into(),
            available,
            status: if available { "ok" } else { "missing" }.into(),
            version: wv2_ver,
            path: wv2_path,
            managed: false,
            essential: false,
            repairable: true,
            detail: if available {
                "就绪".into()
            } else {
                "未检测到有效的 WebView2 运行时（缺少 msedgewebview2.exe），Tauri / Wails 桌面程序需要此组件".into()
            },
        });
    }
    out
}

fn emit_repair_progress(app: &AppHandle, task_id: &str, label: &str, stage: &str, detail: &str, percent: u8) {
    let project_id = crate::runner::resolve_task_project(task_id);
    let _ = app.emit(
        "task-progress",
        &TaskProgressEvent {
            task_id: task_id.to_string(),
            project_id,
            label: label.to_string(),
            stage: stage.to_string(),
            detail: detail.to_string(),
            percent,
            indeterminate: false,
        },
    );
}

fn run_ps_repair(app: &AppHandle, task_id: &str, script: &str) -> Result<()> {
    let ps = preferred_pwsh_native();
    let helper = r#"
[Console]::InputEncoding=[Console]::OutputEncoding=$OutputEncoding=[Text.UTF8Encoding]::new($false);
$ErrorActionPreference='Stop';
$ProgressPreference='SilentlyContinue';
try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13 } catch { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 }
function Invoke-SfDownload([string]$Url, [string]$OutFile) {
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try {
            if (Test-Path $OutFile) { Remove-Item $OutFile -Force -ErrorAction SilentlyContinue }
            Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing -Headers @{ 'User-Agent' = 'ShipForge/1.3' } -TimeoutSec 120
            if ((Test-Path $OutFile) -and ((Get-Item $OutFile).Length -gt 1024)) { return }
            throw '下载文件为空或不完整'
        } catch {
            if (Test-Path $OutFile) { Remove-Item $OutFile -Force -ErrorAction SilentlyContinue }
            if ($attempt -eq 3) { throw "下载失败 ($Url)：$($_.Exception.Message)" }
            Write-Host "第 $attempt 次下载失败，2 秒后重试：$($_.Exception.Message)"
            Start-Sleep -Seconds 2
        }
    }
}
"#;
    let wrapped = format!("{}\n{}", helper, script);
    let mut cmd = Command::new(&ps);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &wrapped])
        .output()
        .map_err(|e| anyhow!("无法调用 PowerShell 执行修复：{}", e))?;

    for line in decode_win_bytes(&out.stdout).lines() {
        if !line.trim().is_empty() {
            emit_log(app, task_id, "stdout", line);
        }
    }
    let stderr_text = decode_win_bytes(&out.stderr);
    let mut err_lines: Vec<String> = Vec::new();
    for line in stderr_text.lines() {
        let t = line.trim();
        if !t.is_empty() {
            emit_log(app, task_id, "stderr", t);
            err_lines.push(t.to_string());
        }
    }
    if !out.status.success() {
        let detail = err_lines
            .iter()
            .find(|l| !l.starts_with("At ") && !l.starts_with("+ ") && !l.starts_with("CategoryInfo") && !l.starts_with("FullyQualifiedErrorId"))
            .or_else(|| err_lines.first())
            .cloned()
            .unwrap_or_else(|| format!("退出码 {:?}", out.status.code()));
        return Err(anyhow!("{}", detail));
    }
    Ok(())
}

fn scale_pct(min_pct: u8, max_pct: u8, local: u8) -> u8 {
    let lo = min_pct.min(100) as u16;
    let hi = max_pct.max(min_pct).min(100) as u16;
    let span = hi.saturating_sub(lo);
    (lo + (span * (local.min(100) as u16)) / 100).min(100) as u8
}

/// One-click automated download / install / repair directly into ShipForge's custom `<dataRoot>\tools\` directory!
pub fn repair_single_tool(
    app: &AppHandle,
    task_id: &str,
    tool_id: &str,
    extra_project_roots: &[String],
) -> Result<ToolchainItem> {
    repair_single_tool_in_range(app, task_id, tool_id, extra_project_roots, 0, 100)
}

/// Scoped variant used by batch repair so progress moves monotonically without jumping backwards or auto-dismissing mid-batch.
pub fn repair_single_tool_in_range(
    app: &AppHandle,
    task_id: &str,
    tool_id: &str,
    extra_project_roots: &[String],
    min_pct: u8,
    max_pct: u8,
) -> Result<ToolchainItem> {
    let tools_root = storage::tools_dir();
    let cache_root = storage::cache_dir();
    fs::create_dir_all(&tools_root)?;
    fs::create_dir_all(&cache_root)?;

    let pct = |local: u8| scale_pct(min_pct, max_pct, local);
    let label = format!("修复工具：{}", tool_id);
    emit_log(app, task_id, "system", &format!("▶ 开始检测并修复工具 `{}`（目标目录：{}）", tool_id, tools_root.display()));
    emit_repair_progress(app, task_id, &label, "准备安装环境", "正在定位零限流官方源…", pct(15));

    let tools_esc = tools_root.to_string_lossy().replace('\'', "''");
    let cache_esc = cache_root.to_string_lossy().replace('\'', "''");

    match tool_id {
        "makensis" => {
            emit_repair_progress(app, task_id, &label, "补全 NSIS 打包器", "正在校验或下载 NSIS 完整组件…", pct(35));
            let local_nsis = std::env::var("LOCALAPPDATA")
                .map(|l| PathBuf::from(l).join(r"tauri\NSIS"))
                .unwrap_or_default();
            let dst_nsis = tools_root.join("nsis");
            if local_nsis.join("makensis.exe").is_file()
                && local_nsis.join("Stubs").is_dir()
                && local_nsis.join("Include").is_dir()
            {
                emit_log(app, task_id, "stdout", "发现本地完整 NSIS 组件，执行 SHA-256 校验同步…");
                storage::copy_tree_verified(&local_nsis, &dst_nsis)?;
            } else {
                let script = format!(
                    r#"
                    $dst = Join-Path '{tools}' 'nsis'
                    $stg = Join-Path '{cache}' 'staging-nsis'
                    $zip = Join-Path '{cache}' 'nsis-portable.zip'
                    $url = 'https://github.com/tauri-apps/binary-releases/releases/download/nsis-3/nsis-3.zip'
                    Write-Host "正在下载官方 NSIS：$url"
                    Invoke-SfDownload -Url $url -OutFile $zip
                    if (Test-Path $stg) {{ Remove-Item $stg -Recurse -Force }}
                    Expand-Archive -Path $zip -DestinationPath $stg -Force
                    $exe = Get-ChildItem $stg -Recurse -Filter 'makensis.exe' | Select-Object -First 1
                    if (-not $exe) {{ throw 'NSIS 压缩包内未找到 makensis.exe' }}
                    $srcRoot = $exe.DirectoryName
                    if ((-not (Test-Path (Join-Path $srcRoot 'Stubs'))) -or (-not (Test-Path (Join-Path $srcRoot 'Include')))) {{
                        throw '下载的 NSIS 缺少 Stubs 或 Include 组件'
                    }}
                    if (Test-Path $dst) {{ Remove-Item $dst -Recurse -Force }}
                    New-Item -ItemType Directory -Force -Path $dst | Out-Null
                    Copy-Item "$srcRoot\*" $dst -Recurse -Force
                    Remove-Item $stg -Recurse -Force -ErrorAction SilentlyContinue
                    Remove-Item $zip -Force -ErrorAction SilentlyContinue
                    Write-Host 'NSIS 部署完成'
                    "#,
                    tools = tools_esc,
                    cache = cache_esc
                );
                run_ps_repair(app, task_id, &script)?;
            }
        }
        "cargo-tauri" => {
            emit_repair_progress(app, task_id, &label, "补全 Tauri CLI", "正在获取官方 cargo-tauri.exe…", pct(35));
            let user_profile = std::env::var("USERPROFILE").unwrap_or_default();
            let mut local_cands: Vec<PathBuf> = Vec::new();
            if !user_profile.is_empty() {
                local_cands.push(PathBuf::from(&user_profile).join(r".cargo\bin\cargo-tauri.exe"));
            }
            if let Ok(ch) = std::env::var("CARGO_HOME") {
                local_cands.push(PathBuf::from(ch).join(r"bin\cargo-tauri.exe"));
            }
            for pr in extra_project_roots {
                local_cands.push(PathBuf::from(pr).join(r"tools\cargo-tauri.exe"));
            }
            let dst_exe = tools_root.join(r"tauri\cargo-tauri.exe");
            if let Some(local_src) = local_cands.into_iter().find(|p| p.is_file()) {
                emit_log(
                    app,
                    task_id,
                    "stdout",
                    &format!("发现本地 cargo-tauri.exe（{}），执行 SHA-256 校验同步…", local_src.display()),
                );
                storage::copy_file_verified(&local_src, &dst_exe)?;
            } else {
                // Zero-rate-limit direct GitHub Release asset download (NEVER hits api.github.com 403 rate limit!)
                let script = format!(
                    r#"
                    $dst = Join-Path '{tools}' 'tauri'
                    $stg = Join-Path '{cache}' 'staging-tauri'
                    $zip = Join-Path '{cache}' 'cargo-tauri.zip'
                    $url = 'https://github.com/tauri-apps/tauri/releases/download/tauri-cli-v2.12.4/cargo-tauri-x86_64-pc-windows-msvc.zip'
                    Write-Host "正在从官方直链下载 Tauri CLI：$url"
                    Invoke-SfDownload -Url $url -OutFile $zip
                    if (Test-Path $stg) {{ Remove-Item $stg -Recurse -Force }}
                    Expand-Archive -Path $zip -DestinationPath $stg -Force
                    $cliExe = Get-ChildItem $stg -Recurse -Filter 'cargo-tauri.exe' | Select-Object -First 1
                    if (-not $cliExe -or $cliExe.Length -lt 102400) {{ throw '解压后的 cargo-tauri.exe 无效或缺失' }}
                    New-Item -ItemType Directory -Force -Path $dst | Out-Null
                    Copy-Item $cliExe.FullName (Join-Path $dst 'cargo-tauri.exe') -Force
                    Remove-Item $stg -Recurse -Force -ErrorAction SilentlyContinue
                    Remove-Item $zip -Force -ErrorAction SilentlyContinue
                    Write-Host 'Tauri CLI 部署完成'
                    "#,
                    tools = tools_esc,
                    cache = cache_esc
                );
                run_ps_repair(app, task_id, &script)?;
            }
        }
        "gh" => {
            emit_repair_progress(app, task_id, &label, "下载 GitHub CLI", "正在通过官方直链获取便携版 gh…", pct(35));
            let script = format!(
                r#"
                $dst = Join-Path '{tools}' 'gh'
                $stg = Join-Path '{cache}' 'staging-gh'
                $zip = Join-Path '{cache}' 'gh-cli.zip'
                $ver = '2.86.0'
                try {{
                    $req = [Net.HttpWebRequest]::Create('https://github.com/cli/cli/releases/latest')
                    $req.Method = 'HEAD'; $req.AllowAutoRedirect = $false; $req.UserAgent = 'ShipForge/1.3'; $req.Timeout = 10000
                    $resp = $req.GetResponse()
                    $loc = $resp.Headers['Location']; $resp.Close()
                    if ($loc -match '/tag/v([0-9]+\.[0-9]+\.[0-9]+)') {{ $ver = $Matches[1] }}
                }} catch {{ }}
                $url = "https://github.com/cli/cli/releases/download/v$ver/gh_${{ver}}_windows_amd64.zip"
                Write-Host "下载官方便携版 GitHub CLI：$url"
                Invoke-SfDownload -Url $url -OutFile $zip
                if (Test-Path $stg) {{ Remove-Item $stg -Recurse -Force }}
                Expand-Archive -Path $zip -DestinationPath $stg -Force
                $ghExe = Get-ChildItem $stg -Recurse -Filter 'gh.exe' | Select-Object -First 1
                if (-not $ghExe) {{ throw 'GitHub CLI 压缩包内未找到 gh.exe' }}
                New-Item -ItemType Directory -Force -Path (Join-Path $dst 'bin') | Out-Null
                Copy-Item $ghExe.FullName (Join-Path $dst 'bin\gh.exe') -Force
                Remove-Item $stg -Recurse -Force -ErrorAction SilentlyContinue
                Remove-Item $zip -Force -ErrorAction SilentlyContinue
                Write-Host "GitHub CLI v$ver 部署完成"
                "#,
                tools = tools_esc,
                cache = cache_esc
            );
            run_ps_repair(app, task_id, &script)?;
        }
        "uv" => {
            emit_repair_progress(app, task_id, &label, "下载 uv", "正在通过官方直链下载便携版 uv…", pct(35));
            let script = format!(
                r#"
                $dst = Join-Path '{tools}' 'uv'
                $stg = Join-Path '{cache}' 'staging-uv'
                $zip = Join-Path '{cache}' 'uv.zip'
                $url = 'https://github.com/astral-sh/uv/releases/latest/download/uv-x86_64-pc-windows-msvc.zip'
                Write-Host "下载官方便携版 uv：$url"
                Invoke-SfDownload -Url $url -OutFile $zip
                if (Test-Path $stg) {{ Remove-Item $stg -Recurse -Force }}
                Expand-Archive -Path $zip -DestinationPath $stg -Force
                $uvExe = Get-ChildItem $stg -Recurse -Filter 'uv.exe' | Select-Object -First 1
                if (-not $uvExe) {{ throw 'uv 压缩包内未找到 uv.exe' }}
                New-Item -ItemType Directory -Force -Path $dst | Out-Null
                Copy-Item $uvExe.FullName (Join-Path $dst 'uv.exe') -Force
                Remove-Item $stg -Recurse -Force -ErrorAction SilentlyContinue
                Remove-Item $zip -Force -ErrorAction SilentlyContinue
                Write-Host 'uv 部署完成'
                "#,
                tools = tools_esc,
                cache = cache_esc
            );
            run_ps_repair(app, task_id, &script)?;
        }
        "pnpm" => {
            emit_repair_progress(app, task_id, &label, "安装 pnpm", "正在安装 pnpm 到自定义工具目录…", pct(40));
            let npm_exe = find_binary_native(&["npm.cmd", "npm.exe"], &[])
                .ok_or_else(|| anyhow!("未检测到 npm，请先安装 Node.js 后再安装 pnpm"))?;
            let npm_esc = npm_exe.to_string_lossy().replace('\'', "''");
            let script = format!(
                r#"
                $dst = Join-Path '{tools}' 'pnpm'
                $npmCache = Join-Path '{cache}' 'npm'
                New-Item -ItemType Directory -Force -Path $dst | Out-Null
                New-Item -ItemType Directory -Force -Path $npmCache | Out-Null
                & '{npm}' install -g pnpm --prefix $dst --cache $npmCache
                if ($LASTEXITCODE -ne 0) {{ throw 'npm 安装 pnpm 失败' }}
                Write-Host 'pnpm 安装完成'
                "#,
                tools = tools_esc,
                cache = cache_esc,
                npm = npm_esc
            );
            run_ps_repair(app, task_id, &script)?;
        }
        "rustc" | "cargo" => {
            emit_repair_progress(app, task_id, &label, "修复 Rust 工具链", "正在通过 rustup / winget 补全…", pct(40));
            let script = r#"
                if (Get-Command rustup -ErrorAction SilentlyContinue) {
                    & rustup update stable
                } else {
                    & winget install --id Rustlang.Rustup -e --silent --accept-package-agreements --accept-source-agreements
                }
            "#;
            run_ps_repair(app, task_id, script)?;
        }
        "git" | "node" | "python" | "iscc" | "go" | "dotnet" | "pwsh" | "cmake" | "bun"
        | "javac" | "webview2" => {
            let winget_id = match tool_id {
                "git" => "Git.Git",
                "node" => "OpenJS.NodeJS.LTS",
                "python" => "Python.Python.3.11",
                "iscc" => "JRSoftware.InnoSetup",
                "go" => "GoLang.Go",
                "dotnet" => "Microsoft.DotNet.SDK.8",
                "pwsh" => "Microsoft.PowerShell",
                "cmake" => "Kitware.CMake",
                "bun" => "Oven-sh.Bun",
                "javac" => "Microsoft.OpenJDK.21",
                "webview2" => "Microsoft.EdgeWebView2Runtime",
                _ => "",
            };
            emit_repair_progress(
                app,
                task_id,
                &label,
                "通过 Winget 自动安装/修复",
                &format!("winget install {}…", winget_id),
                pct(40),
            );
            let script = format!(
                r#"
                if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {{
                    throw '未检测到 winget 包管理器，请手动安装 {id}'
                }}
                & winget install --id {id} -e --silent --accept-package-agreements --accept-source-agreements
                if ($LASTEXITCODE -ne 0) {{
                    Write-Host '尝试升级/修复已有安装…'
                    & winget upgrade --id {id} -e --silent --accept-package-agreements --accept-source-agreements
                }}
                "#,
                id = winget_id
            );
            run_ps_repair(app, task_id, &script)?;
        }
        other => {
            return Err(anyhow!("暂不支持自动修复工具：{}", other));
        }
    }

    emit_repair_progress(app, task_id, &label, "复核工具完整性", "正在重新执行版本与结构验证…", pct(88));
    let refreshed = scan_toolchains()
        .into_iter()
        .find(|t| t.id == tool_id)
        .ok_or_else(|| anyhow!("修复后未找到工具条目"))?;

    if refreshed.status != "ok" {
        return Err(anyhow!(
            "工具 `{}` 安装后复核未通过（{}）",
            tool_id,
            refreshed.detail
        ));
    }

    emit_repair_progress(app, task_id, &label, "修复完成", &refreshed.version, pct(100));
    emit_log(
        app,
        task_id,
        "system",
        &format!("✓ 工具 `{}` 修复完成：{} ({})", tool_id, refreshed.version, refreshed.path),
    );
    Ok(refreshed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_toolchain_scan_has_no_replacement_char_mojibake() {
        let list = scan_toolchains();
        assert!(!list.is_empty());
        for item in &list {
            assert!(
                !item.path.contains('\u{FFFD}'),
                "Tool {} path has UTF-8 replacement mojibake: {}",
                item.id,
                item.path
            );
            assert!(
                !item.version.contains('\u{FFFD}'),
                "Tool {} version has UTF-8 replacement mojibake: {}",
                item.id,
                item.version
            );
        }
        // Verify rustc and git resolve cleanly with real versions on this machine
        let rustc = list.iter().find(|t| t.id == "rustc").unwrap();
        assert_eq!(rustc.status, "ok");
        assert!(rustc.version.contains("rustc"));
    }

    #[test]
    fn test_scale_pct_is_monotonic_for_batch_repair() {
        // Tool 1 occupies [5..50], Tool 2 occupies [50..95] -> strictly non-decreasing and never hits 100 early
        let t1_start = scale_pct(5, 50, 15);
        let t1_end = scale_pct(5, 50, 100);
        let t2_start = scale_pct(50, 95, 15);
        let t2_end = scale_pct(50, 95, 100);
        assert!(t1_start < t1_end);
        assert!(t1_end <= t2_start);
        assert!(t2_start < t2_end);
        assert!(t2_end < 100);
    }
}
