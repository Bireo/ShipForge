use crate::models::{GuardrailSettings, HeadroomSnapshot, PerformanceAdvisory};
use crate::storage;
use anyhow::{anyhow, Result};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// Fixed bounded per-worker streaming buffer (256 KiB).
pub const STREAM_BUF_BYTES: usize = 256 * 1024;

/// Windows file-lock release retry schedule (total ~3.4s).
pub const WIN_LOCK_BACKOFF_MS: [u64; 6] = [50, 120, 250, 500, 1000, 1500];

/// Network retry backoff schedule (3 attempts).
pub const NET_RETRY_BACKOFF_MS: [u64; 3] = [400, 1200, 2800];

/// Compute safe bounded concurrency leaving headroom for OS & desktop responsiveness.
pub fn safe_concurrency(settings: &GuardrailSettings) -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let default_cap = cores.saturating_sub(1).clamp(1, 8);
    let cap = if settings.max_concurrency > 0 {
        settings.max_concurrency.clamp(1, cores.max(1))
    } else {
        default_cap
    };
    let (avail_mem, _) = query_memory_bytes();
    // Under memory pressure (< 1.5 GiB available), throttle workers to avoid OOM / thrashing.
    if avail_mem > 0 && avail_mem < 1536 * 1024 * 1024 {
        cap.min(2)
    } else {
        cap
    }
}

/// Query free and total disk space for the volume containing `path`.
#[cfg(windows)]
pub fn query_disk_space_bytes(path: &Path) -> (u64, u64) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut probe = path.to_path_buf();
    while !probe.exists() {
        if let Some(parent) = probe.parent() {
            probe = parent.to_path_buf();
        } else {
            break;
        }
    }
    let wide: Vec<u16> = probe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free_avail: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_free: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_avail,
            &mut total_bytes,
            &mut total_free,
        )
    };
    if ok != 0 {
        (free_avail, total_bytes)
    } else {
        (0, 0)
    }
}

#[cfg(not(windows))]
pub fn query_disk_space_bytes(_path: &Path) -> (u64, u64) {
    (8 * 1024 * 1024 * 1024, 64 * 1024 * 1024 * 1024)
}

/// Query available and total physical RAM bytes.
#[cfg(windows)]
pub fn query_memory_bytes() -> (u64, u64) {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut st: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    st.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    let ok = unsafe { GlobalMemoryStatusEx(&mut st) };
    if ok != 0 {
        (st.ullAvailPhys, st.ullTotalPhys)
    } else {
        (0, 0)
    }
}

#[cfg(not(windows))]
pub fn query_memory_bytes() -> (u64, u64) {
    (4 * 1024 * 1024 * 1024, 16 * 1024 * 1024 * 1024)
}

fn drive_label(path: &Path) -> String {
    let s = path.to_string_lossy();
    let trimmed = s.strip_prefix(r"\\?\").unwrap_or(&s);
    if trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':' {
        trimmed[..2].to_ascii_uppercase()
    } else {
        trimmed.chars().take(24).collect()
    }
}

/// Build a full system safety headroom snapshot for a target directory and estimated output size.
pub fn compute_headroom(
    target_path: &Path,
    estimated_output_bytes: u64,
    settings: &GuardrailSettings,
) -> HeadroomSnapshot {
    let (disk_free, disk_total) = query_disk_space_bytes(target_path);
    let min_bytes = settings
        .min_free_disk_mb
        .clamp(256, 10_000_000)
        .saturating_mul(1024 * 1024);
    let mult = settings.disk_headroom_multiplier.clamp(1, 10);
    let required = min_bytes.max(estimated_output_bytes.saturating_mul(mult));
    let disk_ok = disk_total == 0 || disk_free >= required;

    let (mem_avail, mem_total) = query_memory_bytes();
    let mem_ok = mem_total == 0 || mem_avail >= 1024 * 1024 * 1024;

    let cpu_cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let safe_conc = safe_concurrency(settings);

    let cache_dir = storage::cache_dir();
    let cache_on_c = cache_dir
        .to_string_lossy()
        .to_ascii_uppercase()
        .starts_with("C:");

    HeadroomSnapshot {
        target_drive: drive_label(target_path),
        disk_free_bytes: disk_free,
        disk_total_bytes: disk_total,
        disk_required_bytes: required,
        disk_ok,
        mem_avail_bytes: mem_avail,
        mem_total_bytes: mem_total,
        mem_ok,
        cpu_cores,
        safe_concurrency: safe_conc,
        cache_on_c_drive: cache_on_c,
    }
}

/// Enforce disk space safety buffer before mutating or heavy I/O operations.
pub fn ensure_disk_headroom(
    target_path: &Path,
    estimated_output_bytes: u64,
    settings: &GuardrailSettings,
) -> Result<HeadroomSnapshot> {
    let snap = compute_headroom(target_path, estimated_output_bytes, settings);
    if !snap.disk_ok {
        return Err(anyhow!(
            "磁盘 {} 剩余空间不足（当前可用 {:.2} GB，安全缓冲要求至少 {:.2} GB）。请清理磁盘或迁移存储目录后重试。",
            snap.target_drive,
            snap.disk_free_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            snap.disk_required_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
        ));
    }
    Ok(snap)
}

/// Produce pragmatic performance and stability advisories from headroom & context.
pub fn build_headroom_advisories(snap: &HeadroomSnapshot) -> Vec<PerformanceAdvisory> {
    let mut out = Vec::new();
    if !snap.disk_ok {
        out.push(PerformanceAdvisory {
            level: "warn".into(),
            title: format!("磁盘 {} 余量低于安全线", snap.target_drive),
            detail: format!(
                "当前可用 {:.2} GB，低于 {:.2} GB 缓冲要求，建议归档旧版本产物或将缓存目录迁至非系统盘。",
                snap.disk_free_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                snap.disk_required_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
            ),
        });
    } else if snap.disk_total_bytes > 0 && snap.disk_free_bytes < 5 * 1024 * 1024 * 1024 {
        out.push(PerformanceAdvisory {
            level: "info".into(),
            title: format!("磁盘 {} 空间偏紧", snap.target_drive),
            detail: format!(
                "剩余 {:.2} GB，连续多版本构建时建议定期清理旧包或开启历史自动裁剪。",
                snap.disk_free_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
            ),
        });
    }
    if !snap.mem_ok {
        out.push(PerformanceAdvisory {
            level: "warn".into(),
            title: "可用内存偏低，已自动限制并发度".into(),
            detail: format!(
                "当前可用内存 {:.2} GB，已自动收敛至 {} 线程以防编译或压缩期触发系统卡顿。",
                snap.mem_avail_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                snap.safe_concurrency
            ),
        });
    }
    if snap.cache_on_c_drive {
        out.push(PerformanceAdvisory {
            level: "info".into(),
            title: "依赖缓存当前位于 C 盘".into(),
            detail: "可在「版本与设置 → 存储目录」将托管工具与构建缓存一键迁至非系统盘，减少 C 盘膨胀。".into(),
        });
    }
    out
}

/// Check if a relative path matches a known sensitive credential / key / private config pattern.
pub fn detect_sensitive_reason(rel_path: &str) -> Option<&'static str> {
    let norm = rel_path.replace('\\', "/");
    let lower = norm.to_ascii_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(&lower);

    if file_name == ".env"
        || (file_name.starts_with(".env.")
            && !file_name.ends_with(".example")
            && !file_name.ends_with(".sample")
            && !file_name.ends_with(".template"))
    {
        return Some("环境变量密钥文件 (.env)");
    }
    if file_name.ends_with(".pem")
        || file_name.ends_with(".key")
        || file_name.ends_with(".p12")
        || file_name.ends_with(".pfx")
        || file_name.ends_with(".jks")
        || file_name.ends_with(".keystore")
    {
        return Some("私钥或证书文件");
    }
    if file_name == "id_rsa"
        || file_name == "id_ed25519"
        || file_name == "id_ecdsa"
        || file_name == ".npmrc"
        || file_name == ".pypirc"
        || file_name == ".netrc"
        || file_name == "_netrc"
        || file_name == "credentials.json"
        || file_name == "service-account.json"
        || file_name == "vault.bin"
    {
        return Some("凭据或认证配置文件");
    }
    if lower == ".git" || lower.starts_with(".git/") || lower.contains("/.git/") {
        return Some("Git 内部元数据目录 (.git)");
    }
    None
}

/// Returns true if a path component matches a Windows reserved DOS device name (CON, NUL, PRN, AUX, COM1..9, LPT1..9)
/// or has illegal Windows trailing dots/spaces.
pub fn is_win_reserved_name(name: &str) -> bool {
    if name.ends_with(' ') || name.ends_with('.') {
        return true;
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// Detect whether `path` is a symlink or Windows NTFS junction/reparse point without following it.
pub fn is_reparse_point(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Validate that a string is strictly a single safe filename (no separators, traversal, drive/ADS colons, reserved names, or control chars).
pub fn validate_single_filename(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return Err(anyhow!("无效的文件名：{}", raw));
    }
    if trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains(':')
        || trimmed.contains('\0')
        || trimmed.contains("..")
        || trimmed.chars().any(|c| c.is_control())
        || is_win_reserved_name(trimmed)
    {
        return Err(anyhow!("文件名包含非法路径、保留设备名或控制字符：{}", raw));
    }
    Ok(trimmed.to_string())
}

/// Ensure a relative path resolves strictly inside `root` without `..` traversal, ADS `:`, reserved device names, or symlink escape
/// (even when the target file does not yet exist on disk).
pub fn resolve_confined_path(root: &Path, rel: &str) -> Result<PathBuf> {
    let trimmed = rel.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("路径不能为空"));
    }
    if trimmed.contains('\0') || trimmed.contains(':') || trimmed.chars().any(|c| c.is_control()) {
        return Err(anyhow!("检测到非法路径字符：{}", trimmed));
    }
    let p = Path::new(trimmed);
    if p.is_absolute() {
        return Err(anyhow!("不允许使用绝对路径：{}", trimmed));
    }
    for comp in p.components() {
        match comp {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow!("检测到越界路径：{}", trimmed));
            }
            Component::Normal(seg) => {
                if let Some(s) = seg.to_str() {
                    if is_win_reserved_name(s) {
                        return Err(anyhow!("检测到系统保留设备名或非法结尾：{}", trimmed));
                    }
                }
            }
            _ => {}
        }
    }
    let joined = root.join(p);
    if let Ok(c_root) = root.canonicalize() {
        if let Ok(c_joined) = joined.canonicalize() {
            if !c_joined.starts_with(&c_root) {
                return Err(anyhow!("路径符号链接逃逸项目目录：{}", trimmed));
            }
        } else {
            // Target does not exist yet: walk ancestors to verify deepest existing directory cannot escape via symlink/junction
            for anc in joined.ancestors().skip(1) {
                if anc.exists() {
                    if let Ok(c_anc) = anc.canonicalize() {
                        if !c_anc.starts_with(&c_root) {
                            return Err(anyhow!("目标父目录符号链接逃逸项目目录：{}", trimmed));
                        }
                    }
                    break;
                }
            }
        }
    }
    Ok(joined)
}

/// Execute a file-system mutation with Windows file-lock backoff retries.
pub fn retry_on_win_lock<T, F>(mut op: F) -> std::io::Result<T>
where
    F: FnMut() -> std::io::Result<T>,
{
    let mut last_err = None;
    for delay in std::iter::once(0u64).chain(WIN_LOCK_BACKOFF_MS.iter().copied()) {
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        match op() {
            Ok(v) => return Ok(v),
            Err(e) => {
                let kind = e.kind();
                let raw = e.raw_os_error().unwrap_or(0);
                // Windows ERROR_SHARING_VIOLATION (32), ERROR_LOCK_VIOLATION (33), ERROR_ACCESS_DENIED (5)
                let transient = kind == std::io::ErrorKind::PermissionDenied
                    || raw == 32
                    || raw == 33
                    || raw == 5;
                if !transient {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "文件锁定重试耗尽")))
}

/// Atomically write bytes via a sibling `.tmp.<pid>.<nanos>` file + lock-aware rename.
pub fn atomic_write_bytes(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let stamp = chrono::Local::now().timestamp_nanos_opt().unwrap_or(0);
    let fname = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let tmp = path.with_file_name(format!(".{}.sf-tmp.{}.{}", fname, std::process::id(), stamp));
    std::fs::write(&tmp, data)?;
    let res = retry_on_win_lock(|| std::fs::rename(&tmp, path));
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!("原子写入 {} 失败：{}", path.display(), e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensitive_file_detection() {
        assert!(detect_sensitive_reason(".env").is_some());
        assert!(detect_sensitive_reason("config/.env.production").is_some());
        assert!(detect_sensitive_reason(".env.example").is_none());
        assert!(detect_sensitive_reason("certs/server.pem").is_some());
        assert!(detect_sensitive_reason("keys\\app.KEY").is_some());
        assert!(detect_sensitive_reason(".git/config").is_some());
        assert!(detect_sensitive_reason("src/main.rs").is_none());
    }

    #[test]
    fn test_confined_path_blocks_traversal() {
        let root = std::env::temp_dir();
        assert!(resolve_confined_path(&root, "../etc/passwd").is_err());
        assert!(resolve_confined_path(&root, "C:\\Windows\\System32").is_err());
        assert!(resolve_confined_path(&root, "foo:bar.txt").is_err());
        assert!(resolve_confined_path(&root, "latest.json::$DATA").is_err());
        assert!(resolve_confined_path(&root, "sub/file.txt").is_ok());

        assert!(validate_single_filename("demo.zip").is_ok());
        assert!(validate_single_filename("../evil.zip").is_err());
        assert!(validate_single_filename("sub/evil.zip").is_err());
        assert!(validate_single_filename("file:ads").is_err());
        assert!(validate_single_filename(".").is_err());
    }

    #[test]
    fn test_atomic_write_overwrites_existing_cleanly() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_atomic_rt_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(5)
        ));
        std::fs::create_dir_all(&base).unwrap();
        let target = base.join("config.json");
        atomic_write_bytes(&target, b"{\"v\":1}").unwrap();
        atomic_write_bytes(&target, b"{\"v\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"v\":2}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_round2_reserved_names_overflow_and_junction_escape() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_r2_guard_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(41)
        ));
        let proj = base.join("proj");
        let outside = base.join("outside");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();

        // 1. Windows reserved DOS device names & trailing dot/space segment tricks blocked
        for bad in ["CON", "NUL", "PRN.txt", "aux.json", "COM1", "lpt9.zip", "foo."] {
            assert!(
                validate_single_filename(bad).is_err(),
                "expected validate_single_filename to reject {:?}",
                bad
            );
            assert!(
                resolve_confined_path(&proj, bad).is_err(),
                "expected resolve_confined_path to reject {:?}",
                bad
            );
        }
        assert!(resolve_confined_path(&proj, "bad_dir /file.txt").is_err());

        // 2. Extreme u64::MAX min_free_disk_mb must NOT overflow or panic
        let extreme = GuardrailSettings {
            min_free_disk_mb: u64::MAX,
            disk_headroom_multiplier: u64::MAX,
            ..GuardrailSettings::default()
        };
        let snap = compute_headroom(&proj, u64::MAX, &extreme);
        assert!(snap.disk_required_bytes >= 256 * 1024 * 1024);

        // 3. Windows NTFS Junction escape blocked even for NON-EXISTENT target file!
        #[cfg(windows)]
        {
            let junc = proj.join("junc_out");
            let status = std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &junc.to_string_lossy(),
                    &outside.to_string_lossy(),
                ])
                .status()
                .unwrap();
            if status.success() {
                assert!(is_reparse_point(&junc));
                assert!(resolve_confined_path(&proj, "junc_out/secret.txt").is_err());
                assert!(
                    resolve_confined_path(&proj, "junc_out/nonexistent_new.md").is_err(),
                    "non-existent target inside NTFS junction must still be blocked"
                );
                let _ = std::fs::remove_dir(&junc);
            }
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_headroom_and_concurrency_bounds() {
        let settings = GuardrailSettings::default();
        let c = safe_concurrency(&settings);
        assert!(c >= 1 && c <= 8);
        let snap = compute_headroom(&std::env::temp_dir(), 1024 * 1024, &settings);
        assert!(snap.cpu_cores >= 1);
        assert!(snap.disk_required_bytes >= 2048 * 1024 * 1024);
    }
}
