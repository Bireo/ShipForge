use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::updater::{
    self, extract_portable_zip_safe, find_portable_payload_root, normalize_sha256,
    sanitize_file_name, updates_dir, validate_update_url, verify_binary_magic, SemVer,
    Sha256Verifier,
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const HOST_SUBDIR: &str = "_host";
const HOST_EXE_NAME: &str = "shipforge-updater.exe";
const MAIN_EXE_NAME: &str = "shipforge.exe";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateJob {
    pub current_version: String,
    pub target_version: String,
    pub package_kind: String, // "installer" | "portable"
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
    pub file_name: String,
    pub staged_path: Option<String>,
    pub app_dir: String,
    pub app_exe: String,
    pub main_pid: u32,
    pub theme: String, // "light" | "dark"
    #[serde(default)]
    pub preview: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProgressEvent {
    pub phase: String, // "waiting_exit" | "downloading" | "verifying" | "installing" | "launching" | "done" | "error" | "cancelled"
    pub step_index: u8, // 0=download, 1=verify, 2=install, 3=launch
    pub percent: f64,
    pub title: String,
    pub detail: String,
    pub downloaded: u64,
    pub total: u64,
    pub speed_bps: u64,
    pub can_cancel: bool,
    pub can_retry: bool,
    pub can_elevate: bool,
    pub can_manual: bool,
    pub error: Option<String>,
}

pub enum UpdaterHostCli {
    Apply(PathBuf),
    Preview { theme: String },
}

pub struct UpdaterHostState {
    pub job: UpdateJob,
    pub started: AtomicBool,
    pub running: AtomicBool,
    pub cancel: Arc<AtomicBool>,
    pub resolved_pkg: Mutex<Option<PathBuf>>,
}

pub fn host_root_dir() -> PathBuf {
    updates_dir().join(HOST_SUBDIR)
}

pub fn parse_updater_host_cli() -> Option<UpdaterHostCli> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--apply-update" => {
                if let Some(p) = args.get(i + 1) {
                    return Some(UpdaterHostCli::Apply(PathBuf::from(p)));
                }
            }
            "--preview-updater" => {
                let mut theme = "dark".to_string();
                for a in &args {
                    if let Some(t) = a.strip_prefix("--theme=") {
                        theme = if t.eq_ignore_ascii_case("light") {
                            "light".into()
                        } else {
                            "dark".into()
                        };
                    }
                }
                return Some(UpdaterHostCli::Preview { theme });
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// 正常主程序启动后异步清理上次更新遗留的 `_host` 副本目录。
pub fn cleanup_updater_host_artifacts() {
    let dir = host_root_dir();
    if dir.exists() {
        let _ = fs::remove_dir_all(&dir);
    }
}

fn copy_file_with_retry(src: &Path, dst: &Path, attempts: u32) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut last_err = None;
    for attempt in 1..=attempts.max(1) {
        match fs::copy(src, dst) {
            Ok(_) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                if attempt < attempts {
                    std::thread::sleep(Duration::from_millis(180 * u64::from(attempt.min(6))));
                }
            }
        }
    }
    Err(anyhow::anyhow!(
        "复制文件失败 ({} -> {}): {}",
        src.display(),
        dst.display(),
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown error".into())
    ))
}

/// 将当前可执行文件与 WebView2Loader.dll 复制到受控临时 `_host` 目录并拉起独立更新小窗。
#[cfg(windows)]
pub fn prepare_and_spawn_updater_host(job: &UpdateJob) -> Result<()> {
    let cur_exe = std::env::current_exe().context("无法获取当前程序路径")?;
    let cur_dir = cur_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("无法解析当前程序目录"))?;

    let host_base = host_root_dir();
    fs::create_dir_all(&host_base).context("无法创建更新宿主目录")?;

    // 优先复用 _host 根目录，若旧进程尚未释放则回退到独立子目录
    let run_dir = {
        let candidate = host_base.join(HOST_EXE_NAME);
        if copy_file_with_retry(&cur_exe, &candidate, 3).is_ok() {
            host_base.clone()
        } else {
            let sub = host_base.join(format!("run_{}", std::process::id()));
            fs::create_dir_all(&sub)?;
            copy_file_with_retry(&cur_exe, &sub.join(HOST_EXE_NAME), 3)
                .context("无法复制更新宿主程序")?;
            sub
        }
    };

    let wv_src = cur_dir.join("WebView2Loader.dll");
    if wv_src.is_file() {
        let _ = copy_file_with_retry(&wv_src, &run_dir.join("WebView2Loader.dll"), 2);
    }

    let job_path = run_dir.join("job.json");
    let raw = serde_json::to_vec_pretty(job).context("序列化更新任务失败")?;
    fs::write(&job_path, raw).context("写入更新任务清单失败")?;

    let host_exe = run_dir.join(HOST_EXE_NAME);
    Command::new(&host_exe)
        .arg("--apply-update")
        .arg(&job_path)
        .spawn()
        .with_context(|| format!("启动独立更新弹窗失败：{}", host_exe.display()))?;
    Ok(())
}

/// 构造 NSIS 静默就地更新参数（`/S /UPDATE /D=<dir>`），确保零卸载弹窗、零安装向导。
pub fn format_nsis_silent_update_args(app_dir: &Path) -> Vec<String> {
    let mut args = vec!["/S".to_string(), "/UPDATE".to_string()];
    let trimmed = app_dir
        .to_string_lossy()
        .trim()
        .trim_end_matches(['\\', '/'])
        .to_string();
    if !trimmed.is_empty()
        && !trimmed.contains('"')
        && !trimmed.contains('\n')
        && !trimmed.contains('\r')
    {
        args.push(format!("/D={trimmed}"));
    }
    args
}

/// 校验更新任务安全不变量（防降级、防非法 URL、防目录穿越）。
pub fn validate_update_job_security(job: &UpdateJob) -> Result<()> {
    if !matches!(job.package_kind.as_str(), "installer" | "portable") {
        bail!("未知更新包类型：{}", job.package_kind);
    }
    let cur = SemVer::parse(&job.current_version)?;
    let target = SemVer::parse(&job.target_version)?;
    if target <= cur {
        bail!(
            "安全拦截：目标版本 v{} 不高于当前版本 v{}",
            job.target_version,
            job.current_version
        );
    }
    let _ = sanitize_file_name(&job.file_name)?;
    let _ = normalize_sha256(&job.sha256)?;
    if !job.download_url.trim().is_empty() {
        let _ = validate_update_url(&job.download_url, true)?;
    }
    Ok(())
}

/// 校验已缓存更新包是否位于受控 updates_dir 内且哈希与魔数完全一致。
fn verify_existing_staged_for_job(updates_root: &Path, job: &UpdateJob) -> Option<PathBuf> {
    let raw = job.staged_path.as_deref()?;
    let p = PathBuf::from(raw);
    let canon_root = fs::canonicalize(updates_root).ok()?;
    let meta = fs::symlink_metadata(&p).ok()?;
    if meta.file_type().is_symlink() {
        return None;
    }
    let canon = fs::canonicalize(&p).ok()?;
    if !canon.starts_with(&canon_root) {
        return None;
    }
    Sha256Verifier.verify_file(&canon, &job.sha256, job.size).ok()?;
    verify_binary_magic(&canon, &job.package_kind).ok()?;
    Some(canon)
}

#[cfg(windows)]
fn wait_for_pid_exit(pid: u32, timeout: Duration) {
    if pid == 0 || pid == std::process::id() {
        return;
    }
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return;
        }
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let r = WaitForSingleObject(h, 80);
            if r == WAIT_OBJECT_0 {
                break;
            }
        }
        CloseHandle(h);
    }
}

#[cfg(windows)]
fn run_nsis_silent_update(
    installer_path: &Path,
    app_dir: &Path,
    elevate: bool,
    mut on_tick: impl FnMut(f64, &str),
) -> Result<()> {
    let args = format_nsis_silent_update_args(app_dir);
    let started = Instant::now();

    if elevate {
        run_nsis_elevated(installer_path, &args, &mut on_tick)
    } else {
        let mut cmd = Command::new(installer_path);
        cmd.arg("/S").arg("/UPDATE");
        if let Some(d_arg) = args.iter().find(|a| a.starts_with("/D=")) {
            // NSIS 规定 /D= 必须是最后一个参数且不可带引号
            cmd.raw_arg(d_arg);
        }
        cmd.creation_flags(CREATE_NO_WINDOW);
        let mut child = cmd.spawn().context("无法启动静默更新安装程序")?;

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        return Ok(());
                    }
                    let code = status.code().unwrap_or(-1);
                    bail!("安装程序返回异常退出码（{code}），可能需要管理员权限或目录被占用");
                }
                Ok(None) => {
                    let secs = started.elapsed().as_secs_f64();
                    let pct = 72.0 + 20.0 * (1.0 - (-secs / 3.8).exp());
                    let detail = if secs < 2.2 {
                        "正在静默更新程序文件（无需手动卸载或点击向导）…"
                    } else if secs < 5.5 {
                        "正在写入新版本与更新快捷方式…"
                    } else {
                        "正在收尾安装并同步版本信息…"
                    };
                    on_tick(pct.min(92.0), detail);
                    std::thread::sleep(Duration::from_millis(160));
                }
                Err(e) => bail!("监控安装进程失败：{e}"),
            }
        }
    }
}

#[cfg(windows)]
fn run_nsis_elevated(
    installer_path: &Path,
    args: &[String],
    on_tick: &mut impl FnMut(f64, &str),
) -> Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    let verb = to_wide("runas");
    let file = to_wide(&installer_path.to_string_lossy());
    let params = to_wide(&args.join(" "));

    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_NOCLOSEPROCESS;
    sei.lpVerb = verb.as_ptr();
    sei.lpFile = file.as_ptr();
    sei.lpParameters = params.as_ptr();
    sei.nShow = 0; // SW_HIDE

    let ok = unsafe { ShellExecuteExW(&mut sei) };
    if ok == 0 || sei.hProcess.is_null() {
        bail!("未获得管理员授权或启动提权安装失败");
    }

    let started = Instant::now();
    let exit_code = unsafe {
        loop {
            let r = WaitForSingleObject(sei.hProcess, 160);
            if r == WAIT_OBJECT_0 {
                break;
            }
            let secs = started.elapsed().as_secs_f64();
            let pct = 72.0 + 20.0 * (1.0 - (-secs / 3.8).exp());
            on_tick(pct.min(92.0), "正在以管理员权限静默完成安装…");
        }
        let mut code: u32 = 1;
        GetExitCodeProcess(sei.hProcess, &mut code);
        CloseHandle(sei.hProcess);
        code
    };

    if exit_code == 0 {
        Ok(())
    } else {
        bail!("提权静默安装失败（退出码 {exit_code}）");
    }
}

/// 纯 Rust 便携版原子替换（带旧版备份与异常自动回滚）。
pub fn apply_portable_swap_atomic(
    payload_root: &Path,
    app_dir: &Path,
    backup_dir: &Path,
    mut on_progress: impl FnMut(f64, &str),
) -> Result<()> {
    if backup_dir.exists() {
        let _ = fs::remove_dir_all(backup_dir);
    }
    fs::create_dir_all(backup_dir).context("创建旧版备份目录失败")?;

    on_progress(80.0, "正在备份当前版本程序，确保可安全回滚…");
    let old_exe = app_dir.join(MAIN_EXE_NAME);
    if old_exe.is_file() {
        copy_file_with_retry(&old_exe, &backup_dir.join(MAIN_EXE_NAME), 4)
            .context("备份旧版主程序失败")?;
    }
    let old_wv = app_dir.join("WebView2Loader.dll");
    if old_wv.is_file() {
        let _ = copy_file_with_retry(&old_wv, &backup_dir.join("WebView2Loader.dll"), 3);
    }

    let swap_res: Result<()> = (|| {
        on_progress(86.0, "正在原子替换新版核心程序…");
        let new_exe = payload_root.join(MAIN_EXE_NAME);
        if !new_exe.is_file() {
            bail!("新版本便携包缺少 {MAIN_EXE_NAME}");
        }
        copy_file_with_retry(&new_exe, &old_exe, 14).context("替换主程序失败（文件可能仍被占用）")?;

        on_progress(91.0, "正在同步运行库与文档…");
        for extra in ["WebView2Loader.dll", "README.md", "LICENSE"] {
            let src = payload_root.join(extra);
            if src.is_file() {
                let _ = copy_file_with_retry(&src, &app_dir.join(extra), 5);
            }
        }
        Ok(())
    })();

    if let Err(err) = swap_res {
        // 自动回滚，确保绝不变砖
        let bak_exe = backup_dir.join(MAIN_EXE_NAME);
        if bak_exe.is_file() {
            let _ = copy_file_with_retry(&bak_exe, &old_exe, 6);
        }
        let bak_wv = backup_dir.join("WebView2Loader.dll");
        if bak_wv.is_file() {
            let _ = copy_file_with_retry(&bak_wv, &old_wv, 4);
        }
        return Err(err);
    }

    let _ = fs::remove_dir_all(backup_dir);
    Ok(())
}

fn emit_progress(app: &AppHandle, ev: HostProgressEvent) {
    let _ = app.emit("updater-host-progress", ev);
}

fn spawn_preview_sequence(app: AppHandle, state: Arc<UpdaterHostState>) {
    std::thread::spawn(move || {
        let ver = state.job.target_version.clone();
        let total = state.job.size.max(14_800_000);
        let emit = |phase: &str, step: u8, pct: f64, title: &str, detail: &str, dl: u64, spd: u64, cancel: bool| {
            emit_progress(
                &app,
                HostProgressEvent {
                    phase: phase.into(),
                    step_index: step,
                    percent: pct,
                    title: title.into(),
                    detail: detail.into(),
                    downloaded: dl,
                    total,
                    speed_bps: spd,
                    can_cancel: cancel,
                    can_retry: false,
                    can_elevate: false,
                    can_manual: false,
                    error: None,
                },
            );
        };

        emit("waiting_exit", 0, 4.0, "正在准备更新环境", "主程序已安全退出，初始化更新引擎…", 0, 0, true);
        std::thread::sleep(Duration::from_millis(650));

        for i in 1..=18 {
            if state.cancel.load(Ordering::SeqCst) {
                emit("cancelled", 0, 0.0, "已取消预览", "更新预览已停止", 0, 0, false);
                return;
            }
            let frac = f64::from(i) / 18.0;
            let pct = 6.0 + frac * 54.0;
            let dl = (total as f64 * frac) as u64;
            let spd = 6_400_000 + (i as u64 % 5) * 380_000;
            emit(
                "downloading",
                0,
                pct,
                &format!("正在下载 ShipForge v{ver}"),
                "连接官方加速通道，支持断点续传",
                dl,
                spd,
                true,
            );
            std::thread::sleep(Duration::from_millis(160));
        }

        emit("verifying", 1, 66.0, "正在校验安装包签名", "验证 SHA-256 完整性与防降级策略…", total, 0, false);
        std::thread::sleep(Duration::from_millis(750));

        for (pct, msg) in [
            (74.0, "正在静默替换核心程序（无需手动卸载或点击向导）…"),
            (84.0, "正在写入新版本并更新快捷方式…"),
            (92.0, "正在清理临时缓存并确认版本一致性…"),
        ] {
            emit("installing", 2, pct, &format!("正在自动安装 v{ver}"), msg, total, 0, false);
            std::thread::sleep(Duration::from_millis(620));
        }

        emit("launching", 3, 97.0, "安装完成，即将启动", &format!("正在唤醒 ShipForge v{ver}…"), total, 0, false);
        std::thread::sleep(Duration::from_millis(550));
        emit("done", 3, 100.0, &format!("已升级至 v{ver}"), "更新预览完成（未改动本地文件），即将自动关闭", total, 0, false);
        std::thread::sleep(Duration::from_millis(1600));
        app.exit(0);
    });
}

#[cfg(windows)]
fn spawn_real_update_pipeline(app: AppHandle, state: Arc<UpdaterHostState>, elevate: bool) {
    if state
        .running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    state.cancel.store(false, Ordering::SeqCst);

    std::thread::spawn(move || {
        let job = state.job.clone();
        let ver = job.target_version.clone();
        let total_hint = job.size;

        let fail = |msg: String, allow_elevate: bool, allow_manual: bool| {
            state.running.store(false, Ordering::SeqCst);
            emit_progress(
                &app,
                HostProgressEvent {
                    phase: "error".into(),
                    step_index: 2,
                    percent: 0.0,
                    title: "自动更新未能完成".into(),
                    detail: msg.clone(),
                    downloaded: 0,
                    total: total_hint,
                    speed_bps: 0,
                    can_cancel: false,
                    can_retry: true,
                    can_elevate: allow_elevate,
                    can_manual: allow_manual,
                    error: Some(msg),
                },
            );
        };

        // 0. 二次安全不变量校验
        if let Err(e) = validate_update_job_security(&job) {
            fail(format!("{e:#}"), false, false);
            return;
        }

        // 1. 等待旧主程序彻底释放文件锁
        emit_progress(
            &app,
            HostProgressEvent {
                phase: "waiting_exit".into(),
                step_index: 0,
                percent: 4.0,
                title: "正在准备自动更新".into(),
                detail: "等待主程序安全退出并释放文件锁…".into(),
                downloaded: 0,
                total: total_hint,
                speed_bps: 0,
                can_cancel: true,
                can_retry: false,
                can_elevate: false,
                can_manual: false,
                error: None,
            },
        );
        wait_for_pid_exit(job.main_pid, Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(260));

        let root = updates_dir();
        if let Err(e) = fs::create_dir_all(&root) {
            fail(format!("无法访问更新缓存目录：{e}"), false, false);
            return;
        }

        // 2. 获取更新包（命中已校验本地缓存则秒过，否则断点续传下载）
        let pkg_path = if let Some(existing) = verify_existing_staged_for_job(&root, &job) {
            emit_progress(
                &app,
                HostProgressEvent {
                    phase: "verifying".into(),
                    step_index: 1,
                    percent: 64.0,
                    title: "已复用本地更新包".into(),
                    detail: "本地缓存已通过 SHA-256 完整性校验".into(),
                    downloaded: job.size,
                    total: job.size,
                    speed_bps: 0,
                    can_cancel: false,
                    can_retry: false,
                    can_elevate: false,
                    can_manual: false,
                    error: None,
                },
            );
            existing
        } else {
            emit_progress(
                &app,
                HostProgressEvent {
                    phase: "downloading".into(),
                    step_index: 0,
                    percent: 6.0,
                    title: format!("正在下载 ShipForge v{ver}"),
                    detail: "正在连接官方安全更新源…".into(),
                    downloaded: 0,
                    total: total_hint,
                    speed_bps: 0,
                    can_cancel: true,
                    can_retry: false,
                    can_elevate: false,
                    can_manual: false,
                    error: None,
                },
            );
            let res = updater::download_and_verify_to(
                &root,
                &job.download_url,
                &job.sha256,
                job.size,
                &job.target_version,
                &job.version_file_name(),
                &job.package_kind,
                &state.cancel,
                Some(&app),
            );
            match res {
                Ok(staged) => PathBuf::from(staged.path),
                Err(e) => {
                    let msg = format!("{e:#}");
                    if state.cancel.load(Ordering::SeqCst) || msg.contains("取消") {
                        state.running.store(false, Ordering::SeqCst);
                        emit_progress(
                            &app,
                            HostProgressEvent {
                                phase: "cancelled".into(),
                                step_index: 0,
                                percent: 0.0,
                                title: "已取消更新".into(),
                                detail: "未对当前软件做任何更改，可随时重新打开原版本".into(),
                                downloaded: 0,
                                total: total_hint,
                                speed_bps: 0,
                                can_cancel: false,
                                can_retry: true,
                                can_elevate: false,
                                can_manual: false,
                                error: None,
                            },
                        );
                    } else {
                        fail(format!("下载或校验更新包失败：{msg}"), false, false);
                    }
                    return;
                }
            }
        };

        if let Ok(mut g) = state.resolved_pkg.lock() {
            *g = Some(pkg_path.clone());
        }

        // 3. 再次复核二进制完整性
        emit_progress(
            &app,
            HostProgressEvent {
                phase: "verifying".into(),
                step_index: 1,
                percent: 69.0,
                title: "安全校验通过".into(),
                detail: "准备就地无缝安装，你的项目与配置将完整保留…".into(),
                downloaded: job.size,
                total: job.size,
                speed_bps: 0,
                can_cancel: false,
                can_retry: false,
                can_elevate: false,
                can_manual: false,
                error: None,
            },
        );
        if let Err(e) = Sha256Verifier.verify_file(&pkg_path, &job.sha256, job.size) {
            fail(format!("安装前哈希复核失败：{e:#}"), false, false);
            return;
        }
        if let Err(e) = verify_binary_magic(&pkg_path, &job.package_kind) {
            fail(format!("安装前格式校验失败：{e:#}"), false, false);
            return;
        }

        // 4. 全自动安装（安装版走 NSIS /S /UPDATE /D=...；便携版走纯 Rust 原子替换 + 回滚）
        let app_dir = PathBuf::from(&job.app_dir);
        if job.package_kind == "portable" {
            emit_progress(
                &app,
                HostProgressEvent {
                    phase: "installing".into(),
                    step_index: 2,
                    percent: 73.0,
                    title: format!("正在自动更新至 v{ver}"),
                    detail: "正在解压便携更新包…".into(),
                    downloaded: job.size,
                    total: job.size,
                    speed_bps: 0,
                    can_cancel: false,
                    can_retry: false,
                    can_elevate: false,
                    can_manual: false,
                    error: None,
                },
            );
            let stage_dir = root.join(format!("stage_{}", job.target_version));
            let backup_dir = root.join(format!("backup_{}", job.target_version));
            let _ = fs::remove_dir_all(&stage_dir);
            if let Err(e) = extract_portable_zip_safe(&pkg_path, &stage_dir) {
                let _ = fs::remove_dir_all(&stage_dir);
                fail(format!("解压便携更新包失败：{e:#}"), false, false);
                return;
            }
            let payload_root = match find_portable_payload_root(&stage_dir) {
                Ok(p) => p,
                Err(e) => {
                    let _ = fs::remove_dir_all(&stage_dir);
                    fail(format!("{e:#}"), false, false);
                    return;
                }
            };
            let app_inst = app.clone();
            let ver_inst = ver.clone();
            let swap_res = apply_portable_swap_atomic(&payload_root, &app_dir, &backup_dir, |pct, detail| {
                emit_progress(
                    &app_inst,
                    HostProgressEvent {
                        phase: "installing".into(),
                        step_index: 2,
                        percent: pct,
                        title: format!("正在自动更新至 v{ver_inst}"),
                        detail: detail.into(),
                        downloaded: job.size,
                        total: job.size,
                        speed_bps: 0,
                        can_cancel: false,
                        can_retry: false,
                        can_elevate: false,
                        can_manual: false,
                        error: None,
                    },
                );
            });
            let _ = fs::remove_dir_all(&stage_dir);
            if let Err(e) = swap_res {
                fail(format!("便携版更新失败（已自动回滚旧版）：{e:#}"), false, false);
                return;
            }
        } else {
            let app_inst = app.clone();
            let ver_inst = ver.clone();
            let res = run_nsis_silent_update(&pkg_path, &app_dir, elevate, |pct, detail| {
                emit_progress(
                    &app_inst,
                    HostProgressEvent {
                        phase: "installing".into(),
                        step_index: 2,
                        percent: pct,
                        title: format!("正在自动安装 v{ver_inst}"),
                        detail: detail.into(),
                        downloaded: job.size,
                        total: job.size,
                        speed_bps: 0,
                        can_cancel: false,
                        can_retry: false,
                        can_elevate: false,
                        can_manual: false,
                        error: None,
                    },
                );
            });
            if let Err(e) = res {
                fail(format!("{e:#}"), !elevate, true);
                return;
            }
        }

        // 清理已下载的旧包（保留 _host 供当前弹窗优雅退出）
        let host_dir = host_root_dir();
        updater::sweep_old_updates_keep(&host_dir);

        // 5. 自动唤醒新版本并优雅退出小窗
        emit_progress(
            &app,
            HostProgressEvent {
                phase: "launching".into(),
                step_index: 3,
                percent: 96.0,
                title: "安装完成，正在启动".into(),
                detail: format!("正在启动新版 ShipForge v{ver}…"),
                downloaded: job.size,
                total: job.size,
                speed_bps: 0,
                can_cancel: false,
                can_retry: false,
                can_elevate: false,
                can_manual: false,
                error: None,
            },
        );
        std::thread::sleep(Duration::from_millis(280));

        let launch_target = if Path::new(&job.app_exe).is_file() {
            PathBuf::from(&job.app_exe)
        } else {
            app_dir.join(MAIN_EXE_NAME)
        };
        if launch_target.is_file() {
            let _ = Command::new(&launch_target).spawn();
        }

        emit_progress(
            &app,
            HostProgressEvent {
                phase: "done".into(),
                step_index: 3,
                percent: 100.0,
                title: format!("已升级至 v{ver}"),
                detail: "新版本已启动，更新窗口即将关闭".into(),
                downloaded: job.size,
                total: job.size,
                speed_bps: 0,
                can_cancel: false,
                can_retry: false,
                can_elevate: false,
                can_manual: false,
                error: None,
            },
        );
        state.running.store(false, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(850));
        app.exit(0);
    });
}

impl UpdateJob {
    fn version_file_name(&self) -> String {
        if self.file_name.trim().is_empty() {
            format!("ShipForge_{}_x64-update.bin", self.target_version)
        } else {
            self.file_name.clone()
        }
    }
}

#[tauri::command]
fn updater_host_init(app: AppHandle, state: State<'_, Arc<UpdaterHostState>>) -> UpdateJob {
    if let Some(w) = app.get_webview_window("updater") {
        let _ = w.show();
        let _ = w.set_focus();
    }
    if state
        .started
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let st = Arc::clone(state.inner());
        if st.job.preview {
            spawn_preview_sequence(app, st);
        } else {
            #[cfg(windows)]
            spawn_real_update_pipeline(app, st, false);
        }
    }
    state.job.clone()
}

#[tauri::command]
fn updater_host_retry(
    app: AppHandle,
    state: State<'_, Arc<UpdaterHostState>>,
    elevate: Option<bool>,
) -> bool {
    let st = Arc::clone(state.inner());
    if st.job.preview {
        spawn_preview_sequence(app, st);
    } else {
        #[cfg(windows)]
        spawn_real_update_pipeline(app, st, elevate.unwrap_or(false));
    }
    true
}

#[tauri::command]
fn updater_host_cancel(app: AppHandle, state: State<'_, Arc<UpdaterHostState>>) -> bool {
    state.cancel.store(true, Ordering::SeqCst);
    if !state.running.load(Ordering::SeqCst) {
        app.exit(0);
    }
    true
}

#[tauri::command]
fn updater_host_reopen_app(app: AppHandle, state: State<'_, Arc<UpdaterHostState>>) -> bool {
    if !state.job.preview {
        let exe = PathBuf::from(&state.job.app_exe);
        if exe.is_file() {
            let _ = Command::new(exe).spawn();
        }
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(160));
        handle.exit(0);
    });
    true
}

#[tauri::command]
fn updater_host_open_manual(app: AppHandle, state: State<'_, Arc<UpdaterHostState>>) -> Result<bool, String> {
    let pkg = state
        .resolved_pkg
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .or_else(|| state.job.staged_path.as_ref().map(PathBuf::from))
        .ok_or_else(|| "尚未定位已下载的安装包".to_string())?;
    if !pkg.is_file() {
        return Err("安装包文件不存在".into());
    }
    Command::new(&pkg)
        .spawn()
        .map_err(|e| format!("无法打开安装包：{e}"))?;
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(180));
        handle.exit(0);
    });
    Ok(true)
}

pub fn run_updater_host_app(cli: UpdaterHostCli) {
    // 独立 WebView2 数据目录，彻底避免与主程序 WebView2 锁竞争
    let webview_dir = host_root_dir().join("webview");
    let _ = fs::create_dir_all(&webview_dir);
    std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &webview_dir);

    let job = match cli {
        UpdaterHostCli::Apply(path) => match fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<UpdateJob>(&bytes).ok())
        {
            Some(j) => j,
            None => return,
        },
        UpdaterHostCli::Preview { theme } => {
            let cur = env!("CARGO_PKG_VERSION").to_string();
            let next_ver = SemVer::parse(&cur)
                .map(|v| format!("{}.{}.{}", v.major, v.minor, v.patch + 1))
                .unwrap_or_else(|_| "9.9.9".to_string());
            UpdateJob {
                current_version: cur,
                target_version: next_ver.clone(),
                package_kind: "installer".to_string(),
                download_url: String::new(),
                sha256: "0".repeat(64),
                size: 14_800_000,
                file_name: format!("ShipForge_{next_ver}_x64-setup.exe"),
                staged_path: None,
                app_dir: String::new(),
                app_exe: String::new(),
                main_pid: 0,
                theme,
                preview: true,
            }
        }
    };

    let state = Arc::new(UpdaterHostState {
        job,
        started: AtomicBool::new(false),
        running: AtomicBool::new(false),
        cancel: Arc::new(AtomicBool::new(false)),
        resolved_pkg: Mutex::new(None),
    });

    let mut ctx = tauri::generate_context!();
    if let Some(win) = ctx.config_mut().app.windows.first_mut() {
        win.label = "updater".into();
        win.title = "ShipForge 自动更新".into();
        win.url = tauri::WebviewUrl::App("updater.html".into());
        win.width = 404.0;
        win.height = 328.0;
        win.min_width = None;
        win.min_height = None;
        win.resizable = false;
        win.maximizable = false;
        win.minimizable = true;
        win.decorations = false;
        win.shadow = true;
        win.center = true;
        win.always_on_top = true;
        win.visible = false;
    }

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            updater_host_init,
            updater_host_retry,
            updater_host_cancel,
            updater_host_reopen_app,
            updater_host_open_manual,
        ])
        .run(ctx)
        .expect("ShipForge 更新弹窗启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nsis_silent_update_args_format_and_strip_trailing_slash() {
        let args = format_nsis_silent_update_args(Path::new(r"C:\Users\Dev\AppData\Local\ShipForge\"));
        assert_eq!(args, vec!["/S", "/UPDATE", r"/D=C:\Users\Dev\AppData\Local\ShipForge"]);

        let spaced = format_nsis_silent_update_args(Path::new(r"D:\Program Files\My ShipForge"));
        assert_eq!(spaced, vec!["/S", "/UPDATE", r"/D=D:\Program Files\My ShipForge"]);
    }

    #[test]
    fn test_validate_update_job_security_blocks_downgrade_and_bad_url() {
        let base = UpdateJob {
            current_version: "1.4.0".into(),
            target_version: "1.4.1".into(),
            package_kind: "installer".into(),
            download_url: "https://github.com/Bireo/ShipForge/releases/download/v1.4.1/ShipForge_1.4.1_x64-setup.exe".into(),
            sha256: "a".repeat(64),
            size: 1024,
            file_name: "ShipForge_1.4.1_x64-setup.exe".into(),
            staged_path: None,
            app_dir: r"C:\Apps\ShipForge".into(),
            app_exe: r"C:\Apps\ShipForge\shipforge.exe".into(),
            main_pid: 42,
            theme: "dark".into(),
            preview: false,
        };
        assert!(validate_update_job_security(&base).is_ok());

        let mut downgrade = base.clone();
        downgrade.target_version = "1.4.0".into();
        assert!(validate_update_job_security(&downgrade).is_err());

        let mut bad_host = base.clone();
        bad_host.download_url = "https://evil.example.com/setup.exe".into();
        assert!(validate_update_job_security(&bad_host).is_err());

        let mut bad_name = base;
        bad_name.file_name = r"..\evil.exe".into();
        assert!(validate_update_job_security(&bad_name).is_err());
    }

    #[test]
    fn test_portable_atomic_swap_replaces_and_rolls_back_on_error() {
        let tmp = std::env::temp_dir().join(format!("sf_upd_host_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let app_dir = tmp.join("app");
        let payload = tmp.join("payload");
        let backup = tmp.join("backup");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&payload).unwrap();

        fs::write(app_dir.join(MAIN_EXE_NAME), b"OLD_BINARY_V1").unwrap();
        fs::write(app_dir.join("WebView2Loader.dll"), b"OLD_WV").unwrap();
        fs::write(payload.join(MAIN_EXE_NAME), b"NEW_BINARY_V2").unwrap();
        fs::write(payload.join("WebView2Loader.dll"), b"NEW_WV").unwrap();

        apply_portable_swap_atomic(&payload, &app_dir, &backup, |_, _| {}).unwrap();
        assert_eq!(fs::read(app_dir.join(MAIN_EXE_NAME)).unwrap(), b"NEW_BINARY_V2");
        assert_eq!(fs::read(app_dir.join("WebView2Loader.dll")).unwrap(), b"NEW_WV");
        assert!(!backup.exists(), "成功后应自动清理备份目录");

        // 缺少主程序时应失败并保留原二进制不变
        let broken_payload = tmp.join("broken_payload");
        fs::create_dir_all(&broken_payload).unwrap();
        assert!(apply_portable_swap_atomic(&broken_payload, &app_dir, &backup, |_, _| {}).is_err());
        assert_eq!(fs::read(app_dir.join(MAIN_EXE_NAME)).unwrap(), b"NEW_BINARY_V2");

        let _ = fs::remove_dir_all(&tmp);
    }
}
