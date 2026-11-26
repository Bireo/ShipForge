use crate::config::append_journal;
use crate::models::{ProjectConfig, TaskFinishedEvent, TaskLogEvent, TaskProgressEvent};
use crate::toolchain;
use anyhow::{anyhow, Result};
use regex::Regex;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn ansi_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap())
}

#[derive(Default)]
pub struct TaskRunner {
    running: Mutex<HashMap<String, RunningTask>>,
    project_busy: Mutex<HashMap<String, String>>,
    pipelines: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

struct RunningTask {
    pid: u32,
    cancelled: Arc<AtomicBool>,
}

/// Decodes Windows stdout/stderr bytes cleanly:
/// 1) Valid UTF-8 -> also heals double-mojibake if a Windows child decoded UTF-8 as GBK (e.g. `鈹€` -> `──`)
/// 2) Non-UTF-8 -> decodes via Win32 CP_ACP (CP936 GBK on Chinese Windows)
pub fn decode_win_bytes(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return heal_utf8_as_gbk_mojibake(s);
    }
    #[cfg(windows)]
    {
        if let Some(decoded) = cp_to_string(bytes, 936)
            .or_else(|| cp_to_string(bytes, windows_sys::Win32::Globalization::CP_ACP))
        {
            return decoded;
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(windows)]
fn cp_to_string(bytes: &[u8], codepage: u32) -> Option<String> {
    use windows_sys::Win32::Globalization::MultiByteToWideChar;
    if bytes.is_empty() {
        return Some(String::new());
    }
    unsafe {
        let needed = MultiByteToWideChar(
            codepage,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        );
        if needed <= 0 {
            return None;
        }
        let mut wide = vec![0u16; needed as usize];
        let written = MultiByteToWideChar(
            codepage,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            needed,
        );
        if written > 0 {
            Some(String::from_utf16_lossy(&wide[..written as usize]))
        } else {
            None
        }
    }
}

#[cfg(windows)]
fn string_to_gbk(s: &str) -> Option<Vec<u8>> {
    use windows_sys::Win32::Globalization::WideCharToMultiByte;
    let wide: Vec<u16> = s.encode_utf16().collect();
    if wide.is_empty() {
        return Some(vec![]);
    }
    unsafe {
        let mut used_default = 0i32;
        let needed = WideCharToMultiByte(
            936,
            0,
            wide.as_ptr(),
            wide.len() as i32,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            &mut used_default,
        );
        if needed <= 0 || used_default != 0 {
            return None;
        }
        let mut out = vec![0u8; needed as usize];
        let written = WideCharToMultiByte(
            936,
            0,
            wide.as_ptr(),
            wide.len() as i32,
            out.as_mut_ptr(),
            needed,
            std::ptr::null(),
            &mut used_default,
        );
        if written > 0 && used_default == 0 {
            out.truncate(written as usize);
            Some(out)
        } else {
            None
        }
    }
}

/// Repairs classic Windows PowerShell pipe double-mojibake where UTF-8 bytes were misinterpreted as GBK
/// and re-encoded as UTF-8 (e.g. `鈹€鈹€` -> `──`, `閹氬` -> `通过`). Only activates when reverse-GBK yields
/// strictly valid UTF-8 containing real CJK/box-drawing characters and fewer rare/garbage glyphs.
pub fn heal_utf8_as_gbk_mojibake(s: &str) -> String {
    #[cfg(windows)]
    {
        // Fast path: pure ASCII never has mojibake
        if s.is_ascii() {
            return s.to_string();
        }
        // Common telltale characters of UTF-8 misinterpreted as GBK (`鈹`, `閹`, `缂`, `鐏`, `閸`, `€`)
        let suspicious = ['鈹', '閹', '缂', '缁', '鐏', '閸', '閺', '閻', '闁', '闂', '鐠', '瑜', '娴', '€']
            .iter()
            .any(|ch| s.contains(*ch));
        if suspicious {
            if let Some(gbk_bytes) = string_to_gbk(s) {
                if let Ok(repaired) = std::str::from_utf8(&gbk_bytes) {
                    let has_ctrl = repaired
                        .chars()
                        .any(|c| (c as u32) < 0x20 && !matches!(c, '\t' | '\r' | '\n'));
                    if repaired != s && !has_ctrl && repaired.chars().count() < s.chars().count() {
                        return repaired.to_string();
                    }
                }
            }
        }
    }
    s.to_string()
}

pub fn strip_ansi(input: &str) -> String {
    ansi_re().replace_all(input, "").into_owned()
}

/// Progressive compact duration formatter:
/// - `< 60s` (`sub_second = true`): `0.0s`..`59.9s` (rolls cleanly from `59.95s` to `1m0s`, never emits `60.0s`)
/// - `< 60s` (`sub_second = false`): `0s`..`59s`
/// - `>= 60s`: `1m0s`, `4m52s`, `1h2m3s`, `1d1h1m1s` (only renders units reached so far)
pub fn format_duration_ms(ms: u64, sub_second: bool) -> String {
    if sub_second {
        let tenths = ms.saturating_add(50) / 100;
        if tenths < 600 {
            return format!("{}.{}s", tenths / 10, tenths % 10);
        }
        return format_compact_seconds(tenths / 10);
    }
    let total_sec = ms / 1000;
    if total_sec < 60 {
        return format!("{}s", total_sec);
    }
    format_compact_seconds(total_sec)
}

fn format_compact_seconds(total_sec: u64) -> String {
    let days = total_sec / 86_400;
    let rem = total_sec % 86_400;
    let hours = rem / 3_600;
    let rem = rem % 3_600;
    let mins = rem / 60;
    let secs = rem % 60;
    if days > 0 {
        format!("{}d{}h{}m{}s", days, hours, mins, secs)
    } else if hours > 0 {
        format!("{}h{}m{}s", hours, mins, secs)
    } else {
        format!("{}m{}s", mins, secs)
    }
}

#[derive(Default)]
struct TaskProjectRegistry {
    map: HashMap<String, String>,
    order: std::collections::VecDeque<String>,
}

fn task_project_map() -> &'static Mutex<TaskProjectRegistry> {
    static MAP: OnceLock<Mutex<TaskProjectRegistry>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(TaskProjectRegistry::default()))
}

pub fn register_task_project(task_id: &str, project_id: &str) {
    const MAX_ENTRIES: usize = 512;
    if task_id.is_empty() || project_id.is_empty() {
        return;
    }
    if let Ok(mut g) = task_project_map().lock() {
        let existed = g.map.contains_key(task_id);
        g.map.insert(task_id.to_string(), project_id.to_string());
        if !existed {
            g.order.push_back(task_id.to_string());
            while g.order.len() > MAX_ENTRIES {
                if let Some(oldest) = g.order.pop_front() {
                    g.map.remove(&oldest);
                }
            }
        }
    }
}

pub fn resolve_task_project(task_id: &str) -> String {
    task_project_map()
        .lock()
        .ok()
        .and_then(|g| g.map.get(task_id).cloned())
        .unwrap_or_default()
}

pub fn emit_log(app: &AppHandle, task_id: &str, stream: &str, line: &str) {
    let clean = strip_ansi(line.trim_end_matches(['\r', '\n']));
    let project_id = resolve_task_project(task_id);
    let _ = app.emit(
        "task-log",
        &TaskLogEvent {
            task_id: task_id.to_string(),
            project_id,
            stream: stream.to_string(),
            line: clean,
            ts: chrono::Local::now().format("%H:%M:%S").to_string(),
        },
    );
}

pub fn emit_progress(
    app: &AppHandle,
    task_id: &str,
    label: &str,
    stage: &str,
    detail: &str,
    percent: u8,
    indeterminate: bool,
) {
    let project_id = resolve_task_project(task_id);
    let _ = app.emit(
        "task-progress",
        &TaskProgressEvent {
            task_id: task_id.to_string(),
            project_id,
            label: label.to_string(),
            stage: stage.to_string(),
            detail: detail.chars().take(90).collect(),
            percent: percent.min(100),
            indeterminate,
        },
    );
}

/// Maps live build/test log milestones onto a smooth monotonic progress percentage inside [min_pct, max_pct].
fn infer_milestone_progress(line: &str, min_pct: u8, max_pct: u8) -> Option<(u8, &'static str)> {
    let l = line.trim();
    if l.is_empty() {
        return None;
    }
    let span = max_pct.saturating_sub(min_pct) as u32;
    let at = |frac_num: u32, frac_den: u32| -> u8 {
        min_pct.saturating_add(((span * frac_num) / frac_den) as u8)
    };

    if l.contains("记录源码快照") {
        Some((at(1, 10), "记录源码快照"))
    } else if l.contains("跑测试门禁") || l.contains("运行 ShipForge 测试门禁") {
        Some((at(2, 10), "运行测试门禁"))
    } else if l.contains("cargo test") || l.contains("cargo check") {
        Some((at(3, 10), "Rust 测试与零警告检查"))
    } else if l.contains("测试门禁通过") || l.contains("测试门禁全部通过") {
        Some((at(4, 10), "测试门禁通过"))
    } else if l.contains("打包 Python 引擎") || l.contains("编译 Rust 遍历器") {
        Some((at(45, 100), "重建子引擎"))
    } else if l.contains("编译 Rust 外壳") || l.contains("Compiling ") {
        Some((at(55, 100), "编译 Rust 发布代码"))
    } else if l.contains("Finished `release`") || l.contains("Built application at:") {
        Some((at(76, 100), "编译完成，准备打包"))
    } else if l.contains("Running makensis") || l.contains("NSIS") {
        Some((at(82, 100), "生成 NSIS 安装包"))
    } else if l.contains("打包便携版") || l.contains("整理安装包与便携压缩包") {
        Some((at(90, 100), "打包便携压缩包"))
    } else if l.contains("已生成发包清单") {
        Some((at(95, 100), "生成更新清单与校验"))
    } else if l.contains("构建完成") || l.contains("全量构建完成") {
        Some((max_pct, "构建与打包完成"))
    } else {
        None
    }
}

fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        cmd.creation_flags(CREATE_NO_WINDOW);
        let _ = cmd
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
}

/// RAII guard that guarantees `TaskRunner::release_project` runs on EVERY exit path (early `?` error OR thread panic).
pub struct ProjectLockGuard {
    runner: Arc<TaskRunner>,
    project_id: String,
    task_id: String,
}

impl Drop for ProjectLockGuard {
    fn drop(&mut self) {
        self.runner.release_project(&self.project_id, &self.task_id);
    }
}

impl TaskRunner {
    pub fn acquire_project_guard(
        self: &Arc<Self>,
        project_id: &str,
        task_id: &str,
    ) -> Result<ProjectLockGuard> {
        self.try_acquire_project(project_id, task_id)?;
        Ok(ProjectLockGuard {
            runner: Arc::clone(self),
            project_id: project_id.to_string(),
            task_id: task_id.to_string(),
        })
    }

    pub fn try_acquire_project(&self, project_id: &str, task_id: &str) -> Result<()> {
        let mut guard = self.project_busy.lock().map_err(|_| anyhow!("执行锁异常"))?;
        if project_id == "__storage__" && !guard.is_empty() {
            return Err(anyhow!("当前有正在运行的构建或修复任务，请等待完成后再迁移目录或收归工具"));
        }
        if guard.contains_key("__storage__") {
            return Err(anyhow!("正在迁移存储目录或收归工具，请稍候完成后再执行任务"));
        }
        if let Some(existing) = guard.get(project_id) {
            return Err(anyhow!(
                "该项目已有正在执行的任务（{}），请等待完成或先点击「停止」",
                existing
            ));
        }
        guard.insert(project_id.to_string(), task_id.to_string());
        Ok(())
    }

    pub fn release_project(&self, project_id: &str, task_id: &str) {
        if let Ok(mut guard) = self.project_busy.lock() {
            if guard.get(project_id).map(|s| s.as_str()) == Some(task_id) {
                guard.remove(project_id);
            }
        }
        // Always prune any pre-spawn cancellation flag so the map never leaks over long sessions
        if let Ok(mut pguard) = self.pipelines.lock() {
            pguard.remove(task_id);
        }
    }

    pub fn has_any_running(&self) -> bool {
        self.project_busy
            .lock()
            .map(|g| !g.is_empty())
            .unwrap_or(false)
            || self.running.lock().map(|g| !g.is_empty()).unwrap_or(false)
    }

    /// Returns the active task/pipeline ID if `project_id` currently holds the execution lock.
    pub fn is_project_busy(&self, project_id: &str) -> Option<String> {
        self.project_busy
            .lock()
            .ok()
            .and_then(|g| g.get(project_id).cloned())
    }

    pub fn register_pipeline(&self, pipeline_id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        if let Ok(mut guard) = self.pipelines.lock() {
            guard.insert(pipeline_id.to_string(), flag.clone());
        }
        flag
    }

    pub fn unregister_pipeline(&self, pipeline_id: &str) {
        if let Ok(mut guard) = self.pipelines.lock() {
            guard.remove(pipeline_id);
        }
    }

    pub fn cancel(&self, task_id: &str) -> Result<bool> {
        let mut hit = false;
        // Consistent lock order: pipelines -> running -> project_busy (deadlock-free)
        if let Ok(mut pguard) = self.pipelines.lock() {
            if let Some(flag) = pguard.get(task_id) {
                flag.store(true, Ordering::SeqCst);
                hit = true;
            } else if self
                .project_busy
                .lock()
                .map(|b| b.values().any(|id| id == task_id))
                .unwrap_or(false)
            {
                // Standalone task was accepted and is still in the pre-spawn window: arm cancel flag atomically!
                pguard.insert(task_id.to_string(), Arc::new(AtomicBool::new(true)));
                hit = true;
            }
        }
        if let Ok(guard) = self.running.lock() {
            if let Some(rt) = guard.get(task_id) {
                rt.cancelled.store(true, Ordering::SeqCst);
                kill_process_tree(rt.pid);
                hit = true;
            }
        }
        Ok(hit)
    }

    pub fn cancel_all(&self) {
        if let Ok(pguard) = self.pipelines.lock() {
            for flag in pguard.values() {
                flag.store(true, Ordering::SeqCst);
            }
        }
        if let Ok(guard) = self.running.lock() {
            for rt in guard.values() {
                rt.cancelled.store(true, Ordering::SeqCst);
                kill_process_tree(rt.pid);
            }
        }
    }

    pub fn active_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .running
            .lock()
            .map(|g| g.keys().cloned().collect())
            .unwrap_or_default();
        if let Ok(pg) = self.pipelines.lock() {
            for k in pg.keys() {
                if !ids.contains(k) {
                    ids.push(k.clone());
                }
            }
        }
        ids
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_streaming_sync(
        &self,
        app: &AppHandle,
        task_id: &str,
        project: &ProjectConfig,
        action_id: &str,
        label: &str,
        command: &str,
        args: &[String],
        rel_cwd: &str,
        extra_env: &HashMap<String, String>,
        version_hint: &str,
        is_pipeline_substep: bool,
        progress_range: (u8, u8),
    ) -> Result<(bool, i32, u64, bool)> {
        let started = Instant::now();
        register_task_project(task_id, &project.id);
        let (min_pct, max_pct) = progress_range;
        let cwd = if rel_cwd.trim().is_empty() {
            PathBuf::from(&project.root)
        } else {
            let p = PathBuf::from(rel_cwd);
            if p.is_absolute() {
                p
            } else {
                PathBuf::from(&project.root).join(p)
            }
        };

        emit_log(
            app,
            task_id,
            "system",
            &format!("▶ [{}] {} {}", label, command, args.join(" ")),
        );
        emit_log(app, task_id, "system", &format!("  工作目录：{}", cwd.display()));
        emit_progress(app, task_id, label, label, "正在启动进程…", min_pct.max(3), false);

        let mut cmd = build_platform_command(command, args);
        cmd.current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut merged_env = project.env.clone();
        for (k, v) in extra_env {
            merged_env.insert(k.clone(), v.clone());
        }
        let path_prepend = merged_env.remove("PATH_PREPEND");
        // Redirect package/build dependency caches into ShipForge's custom `<dataRoot>\cache\`
        // (unless project.env overrides OR user already set a custom non-C-drive env var!)
        for (k, v) in crate::storage::managed_cache_env_vars() {
            let in_proj = merged_env.keys().any(|ek| ek.eq_ignore_ascii_case(&k));
            if in_proj {
                continue;
            }
            let existing_non_c = std::env::var(&k)
                .ok()
                .map(|val| {
                    let t = val.trim();
                    !t.is_empty()
                        && !t.to_ascii_uppercase().starts_with("C:\\")
                        && !t.to_ascii_uppercase().starts_with("C:/")
                })
                .unwrap_or(false);
            if !existing_non_c {
                let _ = std::fs::create_dir_all(&v);
                cmd.env(&k, &v);
            }
        }
        for (k, v) in &merged_env {
            cmd.env(k, v);
        }

        // Prepend ShipForge managed custom tools directory + project PATH_PREPEND onto PATH!
        let managed_prefix = toolchain::managed_path_prefix();
        let cur_path = std::env::var("PATH").unwrap_or_default();
        let mut path_parts: Vec<String> = Vec::new();
        if !managed_prefix.is_empty() {
            path_parts.push(managed_prefix);
        }
        if let Some(prepend) = path_prepend {
            if !prepend.trim().is_empty() {
                path_parts.push(prepend);
            }
        }
        if !cur_path.is_empty() {
            path_parts.push(cur_path);
        }
        if let Some(linker_dir) = toolchain::fallback_linker_bin_dir() {
            let ld_str = linker_dir.to_string_lossy().to_string();
            if !path_parts.iter().any(|p| p.eq_ignore_ascii_case(&ld_str)) {
                path_parts.push(ld_str);
            }
        }
        cmd.env("PATH", path_parts.join(";"));

        cmd.env("PYTHONUTF8", "1");
        cmd.env("PYTHONIOENCODING", "utf-8");

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let err_msg = format!("✗ 无法启动命令 `{}`：{}", command, e);
                emit_log(app, task_id, "stderr", &err_msg);
                let _ = app.emit(
                    "task-finished",
                    &TaskFinishedEvent {
                        task_id: task_id.to_string(),
                        project_id: project.id.clone(),
                        action_id: action_id.to_string(),
                        label: label.to_string(),
                        success: false,
                        exit_code: -1,
                        duration_ms: started.elapsed().as_millis() as u64,
                        cancelled: false,
                        is_pipeline_substep,
                    },
                );
                return Err(anyhow!(err_msg));
            }
        };
        let pid = child.id();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cur_pct = Arc::new(AtomicU8::new(min_pct.max(3)));

        // Lock order MUST match `cancel()` / `cancel_all()` (`pipelines` -> `running`) so deadlock is impossible.
        // Atomically checks whether this pipeline was already cancelled during the `cmd.spawn()` window!
        let already_cancelled = {
            let pguard = self.pipelines.lock().map_err(|_| anyhow!("锁异常"))?;
            let mut rguard = self.running.lock().map_err(|_| anyhow!("锁异常"))?;
            let flag_set = pguard
                .get(task_id)
                .map(|f| f.load(Ordering::SeqCst))
                .unwrap_or(false);
            if flag_set {
                cancelled.store(true, Ordering::SeqCst);
                true
            } else {
                rguard.insert(
                    task_id.to_string(),
                    RunningTask {
                        pid,
                        cancelled: cancelled.clone(),
                    },
                );
                false
            }
        };
        if already_cancelled {
            kill_process_tree(pid);
        }

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let last_activity_ms = Arc::new(AtomicU64::new(0));
        let hb_stop = Arc::new((Mutex::new(false), Condvar::new()));

        let app_out = app.clone();
        let tid_out = task_id.to_string();
        let lbl_out = label.to_string();
        let pct_out = cur_pct.clone();
        let act_out = last_activity_ms.clone();
        let h_out = thread::spawn(move || {
            if let Some(out) = stdout {
                let mut reader = BufReader::new(out);
                let mut buf = Vec::new();
                while let Ok(n) = reader.read_until(b'\n', &mut buf) {
                    if n == 0 {
                        break;
                    }
                    act_out.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                    let text = decode_win_bytes(&buf);
                    let clean = strip_ansi(text.trim_end_matches(['\r', '\n']));
                    emit_log(&app_out, &tid_out, "stdout", &clean);
                    if let Some((next_p, stage)) = infer_milestone_progress(&clean, min_pct, max_pct)
                    {
                        let prev = pct_out.fetch_max(next_p, Ordering::Relaxed);
                        let p = next_p.max(prev);
                        emit_progress(&app_out, &tid_out, &lbl_out, stage, &clean, p, false);
                    }
                    buf.clear();
                }
            }
        });

        let app_err = app.clone();
        let tid_err = task_id.to_string();
        let lbl_err = label.to_string();
        let pct_err = cur_pct.clone();
        let act_err = last_activity_ms.clone();
        let h_err = thread::spawn(move || {
            if let Some(err) = stderr {
                let mut reader = BufReader::new(err);
                let mut buf = Vec::new();
                while let Ok(n) = reader.read_until(b'\n', &mut buf) {
                    if n == 0 {
                        break;
                    }
                    act_err.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                    let text = decode_win_bytes(&buf);
                    let clean = strip_ansi(text.trim_end_matches(['\r', '\n']));
                    emit_log(&app_err, &tid_err, "stderr", &clean);
                    if let Some((next_p, stage)) = infer_milestone_progress(&clean, min_pct, max_pct)
                    {
                        let prev = pct_err.fetch_max(next_p, Ordering::Relaxed);
                        let p = next_p.max(prev);
                        emit_progress(&app_err, &tid_err, &lbl_err, stage, &clean, p, false);
                    }
                    buf.clear();
                }
            }
        });

        // Silent-compile heartbeat: Rust LTO linking or NSIS compression may produce zero output
        // for 2-3 minutes; emit periodic status ticks every 25s of silence and stop immediately via Condvar.
        let app_hb = app.clone();
        let tid_hb = task_id.to_string();
        let lbl_hb = label.to_string();
        let pct_hb = cur_pct.clone();
        let act_hb = last_activity_ms.clone();
        let stop_hb = hb_stop.clone();
        let cancel_hb = cancelled.clone();
        let h_hb = thread::spawn(move || {
            const HEARTBEAT_SILENCE_MS: u64 = 25_000;
            let mut last_hb_ms: u64 = 0;
            let (lock, cvar) = &*stop_hb;
            let mut stopped = match lock.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            while !*stopped && !cancel_hb.load(Ordering::SeqCst) {
                let res = cvar.wait_timeout(stopped, Duration::from_secs(5));
                stopped = match res {
                    Ok((g, _)) => g,
                    Err(_) => return,
                };
                if *stopped || cancel_hb.load(Ordering::SeqCst) {
                    break;
                }
                let now_ms = started.elapsed().as_millis() as u64;
                let last_ms = act_hb.load(Ordering::Relaxed);
                let quiet_ms = now_ms.saturating_sub(last_ms);
                if quiet_ms >= HEARTBEAT_SILENCE_MS
                    && now_ms.saturating_sub(last_hb_ms) >= HEARTBEAT_SILENCE_MS
                {
                    let ceiling = max_pct.saturating_sub(2).max(min_pct);
                    let prev = pct_hb.load(Ordering::Relaxed);
                    let candidate = if prev < ceiling { prev.saturating_add(1) } else { prev };
                    let old = pct_hb.fetch_max(candidate, Ordering::Relaxed);
                    let next_p = candidate.max(old);
                    let total_lbl = format_duration_ms(now_ms, false);
                    let quiet_lbl = format_duration_ms(quiet_ms, false);
                    let detail = format!(
                        "⏳ 仍在后台编译/链接中（累计 {}，近 {} 静默等待子进程输出）…",
                        total_lbl, quiet_lbl
                    );
                    emit_log(&app_hb, &tid_hb, "system", &detail);
                    emit_progress(&app_hb, &tid_hb, &lbl_hb, "后台编译/链接中", &detail, next_p, false);
                    last_hb_ms = now_ms;
                }
            }
        });

        let wait_res = child.wait();
        {
            let (lock, cvar) = &*hb_stop;
            if let Ok(mut stopped) = lock.lock() {
                *stopped = true;
                cvar.notify_all();
            }
        }
        let _ = h_hb.join();
        let status = wait_res?;

        let was_cancelled = cancelled.load(Ordering::SeqCst);
        if !was_cancelled {
            let _ = h_out.join();
            let _ = h_err.join();
        }

        if let Ok(mut guard) = self.running.lock() {
            guard.remove(task_id);
        }

        // If user cancelled mid-build, taskkill may terminate build.ps1 before its finally block removes `.build.lock`.
        // Automatically archive/clean a dead stale build lock so the next run isn't blocked!
        if was_cancelled && !project.lock_file.trim().is_empty() {
            if let Some(info) = crate::build_guard::inspect_lock(&project.lock_file) {
                if info.exists && !info.alive {
                    if let Ok(cleaned) = crate::build_guard::clean_stale_lock(&project.lock_file) {
                        emit_log(app, task_id, "system", &format!("✓ 中止清理：{}", cleaned));
                    }
                }
            }
        }

        let exit_code = status.code().unwrap_or(-1);
        let success = !was_cancelled && status.success();
        let duration_ms = started.elapsed().as_millis() as u64;
        let dur_lbl = format_duration_ms(duration_ms, true);

        let summary_line = if was_cancelled {
            format!("■ 任务已取消（耗时 {}）", dur_lbl)
        } else if success {
            format!("✓ 执行成功（退出码 0，耗时 {}）", dur_lbl)
        } else {
            format!("✗ 执行失败（退出码 {}，耗时 {}）", exit_code, dur_lbl)
        };
        emit_log(app, task_id, "system", &summary_line);
        if !is_pipeline_substep {
            emit_progress(
                app,
                task_id,
                label,
                if success { "已完成" } else if was_cancelled { "已取消" } else { "执行失败" },
                &summary_line,
                if success { 100 } else { cur_pct.load(Ordering::Relaxed) },
                false,
            );
        }

        append_journal(&serde_json::json!({
            "ts": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            "projectId": project.id,
            "projectRoot": project.root,
            "version": version_hint,
            "mode": action_id,
            "result": if success { "ok" } else if was_cancelled { "cancelled" } else { "failed" },
            "duration": dur_lbl,
            "git": "",
            "note": label
        }));

        let _ = app.emit(
            "task-finished",
            &TaskFinishedEvent {
                task_id: task_id.to_string(),
                project_id: project.id.clone(),
                action_id: action_id.to_string(),
                label: label.to_string(),
                success,
                exit_code,
                duration_ms,
                cancelled: was_cancelled,
                is_pipeline_substep,
            },
        );

        Ok((success, exit_code, duration_ms, was_cancelled))
    }
}

fn ps_quote(arg: &str) -> String {
    if arg.starts_with('-')
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "''"))
}

fn ps_script_ref(script_path: &str) -> String {
    let trimmed = script_path.trim();
    let has_sep = trimmed.contains('\\') || trimmed.contains('/') || trimmed.contains(':');
    if has_sep {
        ps_quote(trimmed)
    } else {
        ps_quote(&format!(".\\{}", trimmed))
    }
}

/// Ensures PowerShell `-File <script>` runs under strict UTF-8 console & pipe encoding so Node/Python/Cargo
/// UTF-8 stdout is NEVER double-mojibaked through Windows CP936!
fn build_platform_command(command: &str, args: &[String]) -> Command {
    #[cfg(windows)]
    {
        let lower = command.to_ascii_lowercase();
        if matches!(lower.as_str(), "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe") {
            if let Some(file_idx) = args.iter().position(|a| a.eq_ignore_ascii_case("-File")) {
                if let Some(script_path) = args.get(file_idx + 1) {
                    let rest: Vec<String> = args[file_idx + 2..]
                        .iter()
                        .map(|a| ps_quote(a))
                        .collect();
                    let invoke_line = format!(
                        "$ErrorActionPreference='Stop'; $utf8=[System.Text.UTF8Encoding]::new($false); [Console]::InputEncoding=$utf8; [Console]::OutputEncoding=$utf8; $OutputEncoding=$utf8; & {} {}",
                        ps_script_ref(script_path),
                        rest.join(" ")
                    );
                    let mut cmd = Command::new(command);
                    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &invoke_line]);
                    return cmd;
                }
            }
        }
        let is_shim = lower.ends_with(".cmd")
            || lower.ends_with(".bat")
            || matches!(
                lower.as_str(),
                "npm" | "pnpm" | "yarn" | "npx" | "pnpx" | "tauri" | "flutter" | "electron-builder"
            );
        if is_shim {
            let mut cmd = Command::new("cmd");
            cmd.arg("/C").arg(command).args(args);
            return cmd;
        }
    }
    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi() {
        assert_eq!(strip_ansi("\x1b[32mOK\x1b[0m build"), "OK build");
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn test_heal_utf8_as_gbk_mojibake() {
        // Real mojibake seen in Screenshot 1 (`──` UTF-8 bytes decoded as GBK -> `鈹€鈹€`)
        assert_eq!(heal_utf8_as_gbk_mojibake("鈹€鈹€"), "──");
        // Legitimate Chinese / Euro strings must NEVER be falsely altered
        assert_eq!(heal_utf8_as_gbk_mojibake("正常中文不受影响"), "正常中文不受影响");
        assert_eq!(heal_utf8_as_gbk_mojibake("价格 100€"), "价格 100€");
        assert_eq!(heal_utf8_as_gbk_mojibake("技术娴熟 瑜伽"), "技术娴熟 瑜伽");
    }

    #[test]
    fn test_ps_script_ref_prefixes_relative_filenames() {
        assert_eq!(ps_script_ref("build.ps1"), "'.\\build.ps1'");
        assert_eq!(ps_script_ref(".\\build.ps1"), "'.\\build.ps1'");
        assert_eq!(ps_script_ref("scripts/build.ps1"), "'scripts/build.ps1'");
        assert_eq!(ps_script_ref("D:\\Dev\\MyApp\\build.ps1"), "'D:\\Dev\\MyApp\\build.ps1'");
    }

    #[test]
    fn test_project_concurrency_guard() {
        let runner = TaskRunner::default();
        assert_eq!(runner.is_project_busy("p1"), None);
        assert!(runner.try_acquire_project("p1", "t1").is_ok());
        assert_eq!(runner.is_project_busy("p1").as_deref(), Some("t1"));
        assert!(runner.try_acquire_project("p1", "t2").is_err());
        assert!(runner.try_acquire_project("p2", "t2").is_ok());
        runner.release_project("p1", "t1");
        assert_eq!(runner.is_project_busy("p1"), None);
        assert!(runner.try_acquire_project("p1", "t3").is_ok());
        // __storage__ lock is atomically exclusive against all project tasks
        assert!(runner.try_acquire_project("__storage__", "m1").is_err());
        runner.release_project("p1", "t3");
        runner.release_project("p2", "t2");
        assert!(runner.try_acquire_project("__storage__", "m1").is_ok());
        assert!(runner.try_acquire_project("p1", "t4").is_err());
        runner.release_project("__storage__", "m1");
        assert!(runner.try_acquire_project("p1", "t4").is_ok());
        // Cancelling a standalone task in the pre-spawn window must arm the cancel flag and clean up on release!
        assert!(runner.cancel("t4").unwrap());
        assert!(runner.pipelines.lock().unwrap().get("t4").unwrap().load(Ordering::SeqCst));
        runner.release_project("p1", "t4");
        assert!(!runner.pipelines.lock().unwrap().contains_key("t4"));
    }

    #[test]
    fn test_format_duration_ms_progressive_rollover() {
        assert_eq!(format_duration_ms(0, true), "0.0s");
        assert_eq!(format_duration_ms(950, true), "1.0s");
        assert_eq!(format_duration_ms(59_949, true), "59.9s");
        // Critical rollover boundary: 59.95s MUST roll to 1m0s, never "60.0s"!
        assert_eq!(format_duration_ms(59_950, true), "1m0s");
        assert_eq!(format_duration_ms(60_000, true), "1m0s");
        assert_eq!(format_duration_ms(65_000, true), "1m5s");
        assert_eq!(format_duration_ms(3_599_000, true), "59m59s");
        assert_eq!(format_duration_ms(3_600_000, true), "1h0m0s");
        assert_eq!(format_duration_ms(3_661_000, true), "1h1m1s");
        assert_eq!(format_duration_ms(86_400_000, true), "1d0h0m0s");
        assert_eq!(format_duration_ms(90_061_000, true), "1d1h1m1s");
        assert_eq!(format_duration_ms(45_900, false), "45s");
        assert_eq!(format_duration_ms(60_000, false), "1m0s");
    }

    #[test]
    fn test_project_lock_guard_releases_on_early_return_and_panic() {
        let runner = Arc::new(TaskRunner::default());
        {
            let _g = runner.acquire_project_guard("p-raii", "t-early").unwrap();
            assert_eq!(runner.is_project_busy("p-raii").as_deref(), Some("t-early"));
            assert!(runner.acquire_project_guard("p-raii", "t-dup").is_err());
        }
        assert_eq!(runner.is_project_busy("p-raii"), None);

        let r2 = Arc::clone(&runner);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _g = r2.acquire_project_guard("p-raii", "t-panic").unwrap();
            assert_eq!(r2.is_project_busy("p-raii").as_deref(), Some("t-panic"));
            panic!("simulated worker crash");
        }));
        assert!(panicked.is_err());
        assert_eq!(
            runner.is_project_busy("p-raii"),
            None,
            "ProjectLockGuard must release lock even if worker thread panics"
        );
    }

    #[test]
    fn test_task_project_registry_fifo_eviction_preserves_active_tasks() {
        let active_tid = format!("active-task-{}", chrono::Local::now().timestamp_nanos_opt().unwrap_or(55));
        register_task_project(&active_tid, "proj-active");
        // Register 300 newer tasks (exceeding old 256 global-clear threshold, staying within 512 FIFO window)
        for i in 0..300 {
            register_task_project(&format!("fifo-fill-{}-{}", active_tid, i), "proj-other");
        }
        assert_eq!(
            resolve_task_project(&active_tid),
            "proj-active",
            "Active task mapping must survive >256 subsequent tasks without global wipeout"
        );
    }
}
