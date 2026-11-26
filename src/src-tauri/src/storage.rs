use crate::models::{ChangeReport, ClassifiedChangeItem, ProjectConfig, StorageInfo, VersionBucket};
use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoragePointer {
    data_root: String,
    updated_at: String,
}

pub fn default_app_data_dir() -> PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(local).join("ShipForge")
    } else {
        std::env::temp_dir().join("ShipForge")
    }
}

pub fn pointer_path() -> PathBuf {
    default_app_data_dir().join("pointer.json")
}

/// Resolves the active storage root directory (custom directory from pointer.json, or default LocalAppData\ShipForge).
/// Under `#[cfg(test)]`, strictly redirects to an isolated per-process temp directory so unit tests
/// can NEVER read, pollute, or corrupt the user's live production ShipForge data root.
pub fn active_data_root() -> PathBuf {
    #[cfg(test)]
    {
        static TEST_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        return TEST_ROOT
            .get_or_init(|| {
                let dir = std::env::temp_dir().join(format!(
                    "shipforge_unit_test_root_{}",
                    std::process::id()
                ));
                let _ = fs::create_dir_all(&dir);
                dir
            })
            .clone();
    }
    #[cfg(not(test))]
    {
        let ptr = pointer_path();
        if ptr.exists() {
            if let Ok(raw) = fs::read_to_string(&ptr) {
                if let Ok(p) =
                    serde_json::from_str::<StoragePointer>(raw.trim_start_matches('\u{feff}'))
                {
                    let candidate = PathBuf::from(p.data_root.trim());
                    if !p.data_root.trim().is_empty() && candidate.is_dir() {
                        return candidate;
                    }
                }
            }
        }
        default_app_data_dir()
    }
}

pub fn config_dir() -> PathBuf {
    let root = active_data_root();
    let sub = root.join("config");
    // Transparent backward-compatibility: if legacy root\config.json exists and config\config.json does not, migrate it cleanly
    let legacy_cfg = root.join("config.json");
    let new_cfg = sub.join("config.json");
    if legacy_cfg.exists() && !new_cfg.exists() {
        let _ = fs::create_dir_all(&sub);
        let _ = fs::rename(&legacy_cfg, &new_cfg);
        let legacy_j = root.join("journal.jsonl");
        let new_j = sub.join("journal.jsonl");
        if legacy_j.exists() && !new_j.exists() {
            let _ = fs::rename(&legacy_j, &new_j);
        }
    }
    sub
}

pub fn config_file_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn journal_file_path() -> PathBuf {
    config_dir().join("journal.jsonl")
}

pub fn tools_dir() -> PathBuf {
    active_data_root().join("tools")
}

pub fn cache_dir() -> PathBuf {
    active_data_root().join("cache")
}

pub fn workspaces_dir() -> PathBuf {
    config_dir().join("workspaces")
}

pub fn vault_file_path() -> PathBuf {
    config_dir().join("vault.bin")
}

pub fn tx_dir() -> PathBuf {
    active_data_root().join("tx")
}

pub fn is_ascii_path(p: &Path) -> bool {
    p.to_string_lossy().is_ascii()
}

fn dir_size_bytes(dir: &Path) -> u64 {
    if !dir.exists() {
        return 0;
    }
    WalkDir::new(dir)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok().map(|m| m.len()))
        .sum()
}

fn dir_has_executable(dir: &Path) -> bool {
    WalkDir::new(dir)
        .max_depth(3)
        .into_iter()
        .flatten()
        .any(|e| {
            e.file_type().is_file()
                && e.path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|ext| {
                        ext.eq_ignore_ascii_case("exe")
                            || ext.eq_ignore_ascii_case("cmd")
                            || ext.eq_ignore_ascii_case("bat")
                    })
                    .unwrap_or(false)
        })
}

pub fn managed_cache_env_vars() -> Vec<(String, String)> {
    let root = active_data_root();
    let cache = root.join("cache");
    let tools = root.join("tools");
    let mut vars = vec![
        ("NPM_CONFIG_CACHE".into(), cache.join("npm").to_string_lossy().to_string()),
        ("npm_config_store_dir".into(), cache.join("pnpm-store").to_string_lossy().to_string()),
        ("PNPM_HOME".into(), tools.join("pnpm").to_string_lossy().to_string()),
        ("PIP_CACHE_DIR".into(), cache.join("pip").to_string_lossy().to_string()),
        ("UV_CACHE_DIR".into(), cache.join("uv").to_string_lossy().to_string()),
        ("TAURI_CACHE_DIR".into(), cache.join("tauri").to_string_lossy().to_string()),
    ];
    // Only redirect CARGO_HOME when the custom root is pure ASCII so GNU dlltool / windres never fail!
    if is_ascii_path(&root) {
        vars.push(("CARGO_HOME".into(), cache.join("cargo").to_string_lossy().to_string()));
    }
    vars
}

pub fn get_storage_info() -> StorageInfo {
    let root = active_data_root();
    let tdir = root.join("tools");
    let cdir = root.join("cache");
    let root_str = root.to_string_lossy().to_string();
    let is_c_drive = root_str
        .to_ascii_uppercase()
        .starts_with("C:\\")
        || root_str.to_ascii_uppercase().starts_with("C:/");
    let managed_tool_count = fs::read_dir(&tdir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir() && dir_has_executable(&e.path()))
                .count()
        })
        .unwrap_or(0);

    StorageInfo {
        data_root: root_str,
        config_dir: config_dir().to_string_lossy().to_string(),
        tools_dir: tdir.to_string_lossy().to_string(),
        cache_dir: cdir.to_string_lossy().to_string(),
        is_ascii: is_ascii_path(&root),
        is_c_drive,
        total_bytes: dir_size_bytes(&root),
        managed_tool_count,
    }
}

fn sha256_path(p: &Path) -> Result<String> {
    let mut f = File::open(p)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect())
}

/// Normalizes Windows paths consistently (strips `\\?\` UNC prefix, normalizes slashes & case)
/// so comparisons work accurately whether a path exists on disk or not.
fn normalize_win_path_key(p: &Path) -> String {
    let resolved = if p.exists() {
        fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    } else {
        p.to_path_buf()
    };
    let mut s = resolved.to_string_lossy().replace('/', "\\");
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        s = stripped.to_string();
    }
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    s.to_ascii_lowercase()
}

pub fn validate_migration_target(old_root: &Path, new_root: &Path) -> Result<()> {
    let new_str = new_root.to_string_lossy().trim().to_string();
    if new_str.is_empty() || !new_root.is_absolute() {
        return Err(anyhow!("目标目录必须是有效的绝对路径"));
    }

    if new_root.parent().is_none() {
        return Err(anyhow!(
            "禁止直接选择磁盘根目录，请指定专用子目录（例如 D:\\Dev\\ShipForgeData）"
        ));
    }

    let old_key = normalize_win_path_key(old_root);
    let new_key = normalize_win_path_key(new_root);

    let forbidden_exact = [
        r"c:\windows",
        r"c:\program files",
        r"c:\program files (x86)",
        r"c:\users",
        r"c:\programdata",
    ];
    for f in forbidden_exact {
        if new_key == f || new_key.starts_with(r"c:\windows\") {
            return Err(anyhow!("禁止迁移到系统关键目录：{}", new_str));
        }
    }

    if old_key == new_key {
        return Err(anyhow!("新目录与当前存储目录相同，无需迁移"));
    }
    if new_key.starts_with(&format!("{}\\", old_key)) {
        return Err(anyhow!("目标目录不能位于当前目录内部（会引发递归复制）"));
    }
    if old_key.starts_with(&format!("{}\\", new_key)) {
        return Err(anyhow!("目标目录不能是当前目录的父目录（会误删父目录内容）"));
    }

    // Refuse migrating into an existing non-empty folder that does not belong to ShipForge
    if new_root.is_dir() && !new_root.join(".shipforge-root").exists() {
        let has_unrelated = fs::read_dir(new_root)
            .map(|mut rd| rd.next().is_some())
            .unwrap_or(false);
        if has_unrelated {
            return Err(anyhow!(
                "目标目录非空且不是 ShipForge 专用目录，请新建空白子目录（例如 {}\\ShipForgeData）以避免混入其他文件",
                new_str.trim_end_matches(['\\', '/'])
            ));
        }
    }

    Ok(())
}

/// Safely migrates ShipForge data root (`config/`, `tools/`, `cache/`) to `new_root` with SHA-256 verification,
/// atomically updates pointer.json, and removes old data directory cleanly.
pub fn migrate_storage_root<F>(new_root_str: &str, on_progress: F) -> Result<StorageInfo>
where
    F: FnMut(u8, &str),
{
    let old_root = active_data_root();
    let new_root = PathBuf::from(new_root_str.trim());
    // Ensure legacy layout is normalized into config/ before migrating
    let _ = config_dir();
    migrate_between_roots(&old_root, &new_root, &pointer_path(), &default_app_data_dir(), on_progress)?;
    Ok(get_storage_info())
}

pub fn migrate_between_roots<F>(
    old_root: &Path,
    new_root: &Path,
    ptr_file: &Path,
    default_dir: &Path,
    mut on_progress: F,
) -> Result<()>
where
    F: FnMut(u8, &str),
{
    validate_migration_target(old_root, new_root)?;

    on_progress(5, "校验目标目录可写权限…");
    fs::create_dir_all(new_root).with_context(|| format!("无法创建目标目录：{}", new_root.display()))?;
    let probe = new_root.join(".shipforge_write_probe");
    fs::write(&probe, b"ok").with_context(|| format!("目标目录不可写：{}", new_root.display()))?;
    let _ = fs::remove_file(&probe);

    let subdirs = ["config", "tools", "cache"];
    let mut files_to_copy: Vec<(PathBuf, PathBuf)> = Vec::new();
    for sub in subdirs {
        let src_sub = old_root.join(sub);
        if !src_sub.exists() {
            continue;
        }
        for entry in WalkDir::new(&src_sub).into_iter().flatten() {
            if entry.file_type().is_file() {
                let rel = entry.path().strip_prefix(old_root).unwrap_or(entry.path());
                files_to_copy.push((entry.path().to_path_buf(), new_root.join(rel)));
            }
        }
    }

    let total = files_to_copy.len().max(1);
    on_progress(15, &format!("正在迁移 {} 个文件到新目录…", files_to_copy.len()));

    let mut copied_targets: Vec<PathBuf> = Vec::new();
    for (idx, (src, dst)) in files_to_copy.iter().enumerate() {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Err(e) = fs::copy(src, dst) {
            for c in copied_targets.into_iter().rev() {
                let _ = fs::remove_file(c);
            }
            return Err(anyhow!("复制文件失败（已回滚）：{} ({})", src.display(), e));
        }
        let h_src = sha256_path(src)?;
        let h_dst = sha256_path(dst)?;
        if h_src != h_dst {
            for c in copied_targets.into_iter().rev() {
                let _ = fs::remove_file(c);
            }
            let _ = fs::remove_file(dst);
            return Err(anyhow!("迁移校验失败（SHA-256 不一致，已回滚）：{}", src.display()));
        }
        copied_targets.push(dst.clone());
        let pct = 15 + (((idx + 1) * 65) / total) as u8;
        if idx % 5 == 0 || idx + 1 == total {
            on_progress(pct.min(80), &format!("已校验 {}/{} 个文件…", idx + 1, total));
        }
    }

    for sub in subdirs {
        fs::create_dir_all(new_root.join(sub))?;
    }
    let _ = fs::write(new_root.join(".shipforge-root"), b"shipforge-managed-storage\n");

    on_progress(85, "原子切换存储目录指针…");
    if let Some(parent) = ptr_file.parent() {
        fs::create_dir_all(parent)?;
    }
    let ptr_payload = StoragePointer {
        data_root: new_root.to_string_lossy().to_string(),
        updated_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    };
    let tmp_ptr = ptr_file.with_extension("json.tmp");
    fs::write(&tmp_ptr, serde_json::to_string_pretty(&ptr_payload)?.as_bytes())?;
    let _ = fs::remove_file(ptr_file);
    fs::rename(&tmp_ptr, ptr_file)?;

    on_progress(92, "安全清理并删除旧目录数据…");
    let old_is_default = normalize_win_path_key(old_root) == normalize_win_path_key(default_dir);

    for sub in subdirs {
        let p = old_root.join(sub);
        if p.exists() {
            let _ = fs::remove_dir_all(&p);
        }
    }
    for own_file in ["config.json", "journal.jsonl", "config.json.tmp", ".shipforge-root"] {
        let p = old_root.join(own_file);
        if p.exists() {
            let _ = fs::remove_file(&p);
        }
    }
    if !old_is_default && old_root.exists() {
        let _ = fs::remove_dir(old_root);
    }

    on_progress(100, "目录迁移与旧数据清理完成");
    Ok(())
}

fn is_reparse_point(p: &Path) -> bool {
    #[cfg(windows)]
    {
        if let Ok(meta) = fs::symlink_metadata(p) {
            return (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
        }
    }
    let _ = p;
    false
}

pub fn copy_file_verified(src: &Path, dst: &Path) -> Result<()> {
    if normalize_win_path_key(src) == normalize_win_path_key(dst) {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp_os = dst.as_os_str().to_os_string();
    tmp_os.push(".sf_tmp");
    let tmp = PathBuf::from(tmp_os);
    fs::copy(src, &tmp).with_context(|| format!("复制失败：{}", src.display()))?;
    let h_src = sha256_path(src)?;
    let h_tmp = sha256_path(&tmp)?;
    if h_src != h_tmp {
        let _ = fs::remove_file(&tmp);
        return Err(anyhow!("SHA-256 校验不一致：{}", src.display()));
    }
    let _ = fs::remove_file(dst);
    if let Err(e) = fs::rename(&tmp, dst) {
        let _ = fs::remove_file(&tmp);
        return Err(anyhow!("写入目标文件失败（可能被进程占用）：{} ({})", dst.display(), e));
    }
    Ok(())
}

pub fn copy_tree_verified(src_dir: &Path, dst_dir: &Path) -> Result<usize> {
    if normalize_win_path_key(src_dir) == normalize_win_path_key(dst_dir) {
        return Ok(0);
    }
    let mut copied = 0usize;
    let mut created_files: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(src_dir).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel = entry.path().strip_prefix(src_dir).unwrap_or(entry.path());
            let target = dst_dir.join(rel);
            if let Err(e) = copy_file_verified(entry.path(), &target) {
                for f in created_files.into_iter().rev() {
                    let _ = fs::remove_file(f);
                }
                return Err(e);
            }
            created_files.push(target);
            copied += 1;
        }
    }
    Ok(copied)
}

/// Retargets an existing/dangling NTFS junction to `target_dir`.
/// Returns `Ok(true)` if the junction was actually updated, `Ok(false)` if already pointing to `target_dir`.
#[cfg(windows)]
fn retarget_junction_safe(junction_dir: &Path, target_dir: &Path) -> Result<bool> {
    if !is_reparse_point(junction_dir) {
        return Ok(false);
    }
    if let Ok(canon) = fs::canonicalize(junction_dir) {
        if normalize_win_path_key(&canon) == normalize_win_path_key(target_dir) {
            return Ok(false);
        }
    }
    // On Windows, fs::remove_dir removes ONLY the junction entry itself (never target contents)
    fs::remove_dir(junction_dir)?;
    let mut cmd = Command::new("cmd");
    cmd.creation_flags(CREATE_NO_WINDOW);
    let ok = cmd
        .args([
            "/C",
            "mklink",
            "/J",
            &junction_dir.to_string_lossy(),
            &target_dir.to_string_lossy(),
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        Ok(true)
    } else {
        Err(anyhow!("重建目录联接失败"))
    }
}

#[cfg(windows)]
fn replace_dir_with_junction_safe(old_dir: &Path, target_dir: &Path) -> bool {
    if !old_dir.is_dir() || is_reparse_point(old_dir) {
        return false;
    }
    let backup = old_dir.with_file_name(format!(
        "{}.sf_migrating_{}",
        old_dir.file_name().and_then(|s| s.to_str()).unwrap_or("old"),
        chrono::Local::now().timestamp_millis()
    ));
    if fs::rename(old_dir, &backup).is_err() {
        return false;
    }
    let mut cmd = Command::new("cmd");
    cmd.creation_flags(CREATE_NO_WINDOW);
    let ok = cmd
        .args([
            "/C",
            "mklink",
            "/J",
            &old_dir.to_string_lossy(),
            &target_dir.to_string_lossy(),
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if ok && old_dir.join("makensis.exe").is_file() {
        let _ = fs::remove_dir_all(&backup);
        true
    } else {
        // Atomic rollback if junction creation failed (e.g. non-NTFS volume)
        let _ = fs::remove_dir(old_dir);
        let _ = fs::rename(&backup, old_dir);
        false
    }
}

/// Consolidates relocatable C-drive build/packaging tools (NSIS, cargo-tauri, uv, gh) and initializes
/// redirected dependency cache directories inside `<active_data_root>\{tools,cache}\`.
pub fn consolidate_tools_and_caches<F>(
    extra_project_roots: &[String],
    mut on_progress: F,
) -> Result<(StorageInfo, Vec<String>)>
where
    F: FnMut(u8, &str),
{
    let root = active_data_root();
    let tdir = root.join("tools");
    fs::create_dir_all(&tdir)?;

    on_progress(10, "初始化受管依赖缓存目录（Cargo / npm / pnpm / pip / uv / Tauri）…");
    for (_, dir_str) in managed_cache_env_vars() {
        let _ = fs::create_dir_all(&dir_str);
    }

    let mut summary: Vec<String> = Vec::new();
    let local_app = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let user_profile = std::env::var("USERPROFILE").unwrap_or_default();

    // 1. Adopt NSIS from %LOCALAPPDATA%\tauri\NSIS -> <dataRoot>\tools\nsis (+ safe 0-byte NTFS Junction to reclaim C: space!)
    // Also retargets an existing/dangling junction if the user migrated A -> B -> C!
    on_progress(25, "检查并迁移 NSIS 打包器组件…");
    let nsis_dst = tdir.join("nsis");
    if !local_app.is_empty() {
        let nsis_src = PathBuf::from(&local_app).join(r"tauri\NSIS");
        if is_reparse_point(&nsis_src) && nsis_complete(&nsis_dst) {
            #[cfg(windows)]
            if matches!(retarget_junction_safe(&nsis_src, &nsis_dst), Ok(true)) {
                summary.push(format!("NSIS 联接已自动重指向至 {}", nsis_dst.display()));
            }
        } else if nsis_src.join("makensis.exe").is_file()
            && nsis_src.join("Stubs").is_dir()
            && nsis_src.join("Include").is_dir()
            && !is_reparse_point(&nsis_src)
            && normalize_win_path_key(&nsis_src) != normalize_win_path_key(&nsis_dst)
        {
            match copy_tree_verified(&nsis_src, &nsis_dst) {
                Ok(n) if nsis_complete(&nsis_dst) => {
                    #[cfg(windows)]
                    let reclaimed = replace_dir_with_junction_safe(&nsis_src, &nsis_dst);
                    #[cfg(not(windows))]
                    let reclaimed = false;
                    if reclaimed {
                        summary.push(format!("NSIS 已迁入并释放 C 盘旧副本（{} 个文件，已建 0 字节联接）", n));
                    } else {
                        summary.push(format!("NSIS 已同步到受管工具目录（{} 个文件）", n));
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    summary.push(format!("NSIS 同步跳过：{}", e));
                }
            }
        }
    }

    // 2. Adopt cargo-tauri.exe dynamically from %USERPROFILE%\.cargo\bin, CARGO_HOME\bin, or imported projects' tools\
    on_progress(50, "检查并收归 Tauri CLI (cargo-tauri.exe)…");
    let tauri_dst = tdir.join(r"tauri\cargo-tauri.exe");
    if !tauri_dst.is_file() {
        let mut tauri_cands: Vec<PathBuf> = Vec::new();
        if !user_profile.is_empty() {
            tauri_cands.push(PathBuf::from(&user_profile).join(r".cargo\bin\cargo-tauri.exe"));
        }
        if let Ok(ch) = std::env::var("CARGO_HOME") {
            tauri_cands.push(PathBuf::from(ch).join(r"bin\cargo-tauri.exe"));
        }
        for pr in extra_project_roots {
            tauri_cands.push(PathBuf::from(pr).join(r"tools\cargo-tauri.exe"));
        }
        if let Some(src) = tauri_cands.into_iter().find(|p| p.is_file()) {
            if copy_file_verified(&src, &tauri_dst).is_ok() {
                summary.push(format!("Tauri CLI 已收归至 {}", tauri_dst.display()));
            }
        }
    }

    // 3. Adopt standalone uv.exe if found outside <dataRoot>\tools
    on_progress(70, "检查并收归便携 CLI 工具（uv / gh）…");
    let uv_dst = tdir.join(r"uv\uv.exe");
    if !uv_dst.is_file() {
        let mut uv_cands = Vec::new();
        if !user_profile.is_empty() {
            uv_cands.push(PathBuf::from(&user_profile).join(r".local\bin\uv.exe"));
            uv_cands.push(PathBuf::from(&user_profile).join(r".cargo\bin\uv.exe"));
        }
        if let Some(src) = crate::toolchain::find_binary_native(&["uv.exe"], &uv_cands) {
            if !normalize_win_path_key(&src).starts_with(&normalize_win_path_key(&tdir))
                && copy_file_verified(&src, &uv_dst).is_ok()
            {
                summary.push(format!("uv 已收归至 {}", uv_dst.display()));
            }
        }
    }

    // 4. Adopt standalone gh.exe if found outside <dataRoot>\tools
    let gh_dst = tdir.join(r"gh\bin\gh.exe");
    if !gh_dst.is_file() {
        let gh_cands = vec![PathBuf::from(r"C:\Program Files\GitHub CLI\gh.exe")];
        if let Some(src) = crate::toolchain::find_binary_native(&["gh.exe"], &gh_cands) {
            if !normalize_win_path_key(&src).starts_with(&normalize_win_path_key(&tdir))
                && copy_file_verified(&src, &gh_dst).is_ok()
            {
                summary.push(format!("GitHub CLI (gh) 已收归至 {}", gh_dst.display()));
            }
        }
    }

    on_progress(100, "工具收归与依赖缓存重定向完成");
    Ok((get_storage_info(), summary))
}

fn nsis_complete(dir: &Path) -> bool {
    dir.join("makensis.exe").is_file() && dir.join("Stubs").is_dir() && dir.join("Include").is_dir()
}

// =========================================================================
// =========================================================================
// Project Identity, Workspace Isolation & Change Classification
// =========================================================================

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceOwnerMeta {
    project_id: String,
    normalized_root: String,
    updated_at: String,
}

/// Canonical normalized root key for cross-module deduplication and isolation checks.
pub fn normalize_root_key(p: &Path) -> String {
    normalize_win_path_key(p)
}

fn short_sha256_hex(input: &str, hex_chars: usize) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut out = String::with_capacity(hex_chars);
    for b in digest.iter() {
        out.push_str(&format!("{:02x}", b));
        if out.len() >= hex_chars {
            out.truncate(hex_chars);
            break;
        }
    }
    out
}

/// Collision-free, Windows-NTFS-safe directory name for a project workspace.
/// - Clean lowercase ASCII IDs (`[a-z0-9_-]+`, not reserved DOS device names) keep exact legacy paths (`tidyassistant`, `shipforge`).
/// - Any ID containing uppercase, `.`, non-ASCII, special chars, or reserved names gets a deterministic SHA-256 suffix
///   so `MyApp` vs `myapp` and `my.app` vs `my_app` NEVER collide on disk!
pub fn project_workspace_key(project_id: &str) -> String {
    let trimmed = project_id.trim();
    if trimmed.is_empty() {
        return "default".to_string();
    }
    let is_clean_legacy = trimmed
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
        && !crate::guardrails::is_win_reserved_name(trimmed);
    if is_clean_legacy {
        return trimmed.to_string();
    }
    let prefix: String = trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(36)
        .collect();
    let prefix = prefix.trim_matches('_');
    let safe_prefix = if prefix.is_empty() || crate::guardrails::is_win_reserved_name(prefix) {
        "proj"
    } else {
        prefix
    };
    format!("{}-{}", safe_prefix, short_sha256_hex(trimmed, 8))
}

/// Single canonical workspace directory for a project (shared by version buckets, artifact metadata & owner guard).
pub fn project_workspace_dir(project_id: &str) -> PathBuf {
    workspaces_dir().join(project_workspace_key(project_id))
}

fn owner_meta_path(project_id: &str) -> PathBuf {
    project_workspace_dir(project_id).join("workspace_owner.json")
}

/// Bind a workspace directory to a specific normalized project root path.
pub fn bind_workspace_owner(project_id: &str, root: &Path) {
    let dir = project_workspace_dir(project_id);
    let _ = fs::create_dir_all(&dir);
    let meta = WorkspaceOwnerMeta {
        project_id: project_id.to_string(),
        normalized_root: normalize_root_key(root),
        updated_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&meta) {
        let _ = crate::guardrails::atomic_write_bytes(&owner_meta_path(project_id), &bytes);
    }
}

/// Returns true if `workspaces/<key>` already exists on disk and belongs to a *different* project root
/// (preventing a newly imported project with a recycled folder name from inheriting an older deleted project's buckets/metadata!).
pub fn workspace_owned_by_other_root(project_id: &str, candidate_root: &Path) -> bool {
    let dir = project_workspace_dir(project_id);
    if !dir.exists() {
        return false;
    }
    let want_root = normalize_root_key(candidate_root);
    let owner_p = owner_meta_path(project_id);
    if let Ok(raw) = fs::read_to_string(&owner_p) {
        if let Ok(meta) = serde_json::from_str::<WorkspaceOwnerMeta>(raw.trim_start_matches('\u{feff}'))
        {
            return !meta.normalized_root.is_empty() && meta.normalized_root != want_root;
        }
    }
    // Legacy workspace directory exists without owner file: if it already contains version buckets or artifact metadata,
    // treat it as occupied by an earlier unknown project so newly imported projects never inherit orphaned state!
    dir.join("versions").exists() || dir.join("artifacts_meta.json").exists()
}

fn sanitize_version_key(ver: &str) -> String {
    let trimmed = ver.trim();
    if trimmed.is_empty() {
        return "0.0.0".to_string();
    }
    let is_clean_semver = trimmed
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
        && !trimmed.contains("..")
        && !crate::guardrails::is_win_reserved_name(trimmed);
    if is_clean_semver {
        return trimmed.to_string();
    }
    let prefix: String = trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let prefix = prefix.replace("..", "_");
    let safe_prefix = if prefix.is_empty() || crate::guardrails::is_win_reserved_name(&prefix) {
        "ver"
    } else {
        prefix.trim_matches('.')
    };
    format!("{}-{}", safe_prefix, short_sha256_hex(trimmed, 8))
}

fn bucket_dir(project_id: &str) -> PathBuf {
    project_workspace_dir(project_id).join("versions")
}

fn bucket_path(project_id: &str, version: &str) -> PathBuf {
    bucket_dir(project_id).join(format!("{}.json", sanitize_version_key(version)))
}

pub fn load_version_bucket(project_id: &str, version: &str) -> VersionBucket {
    let p = bucket_path(project_id, version);
    if p.exists() {
        if let Ok(raw) = fs::read_to_string(&p) {
            if let Ok(b) = serde_json::from_str::<VersionBucket>(raw.trim_start_matches('\u{feff}'))
            {
                return b;
            }
        }
    }
    VersionBucket {
        version: version.to_string(),
        base_git_sha: String::new(),
        last_built_git_sha: String::new(),
        last_built_fingerprint: String::new(),
        last_built_at: String::new(),
        build_count: 0,
        notes_draft: String::new(),
        changed_files: vec![],
        artifact_hashes: HashMap::new(),
        sealed: false,
        updated_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    }
}

pub fn save_version_bucket(project_id: &str, bucket: &VersionBucket) -> Result<()> {
    let dir = bucket_dir(project_id);
    fs::create_dir_all(&dir)?;
    let path = bucket_path(project_id, &bucket.version);
    let data = serde_json::to_vec_pretty(bucket)?;
    crate::guardrails::atomic_write_bytes(&path, &data)?;
    Ok(())
}

pub fn list_version_buckets(project_id: &str) -> Vec<VersionBucket> {
    let dir = bucket_dir(project_id);
    let Ok(rd) = fs::read_dir(&dir) else {
        return vec![];
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Ok(raw) = fs::read_to_string(&p) {
            if let Ok(b) = serde_json::from_str::<VersionBucket>(raw.trim_start_matches('\u{feff}'))
            {
                out.push(b);
            }
        }
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

/// Normalizes a project-configured relative or absolute path (e.g., `./release/`, `D:\App\release`)
/// into a lowercase forward-slash relative path inside `root`. Returns `None` if empty or outside `root`.
pub fn normalize_project_rel_path(root: &Path, raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    let p = Path::new(trimmed);
    if p.is_absolute() {
        let root_key = normalize_root_key(root);
        let p_key = normalize_root_key(p);
        if root_key.is_empty() || p_key == root_key {
            return None;
        }
        let prefix = format!("{}\\", root_key);
        if let Some(rest) = p_key.strip_prefix(&prefix) {
            let rel = rest.replace('\\', "/").trim_matches('/').to_string();
            return if rel.is_empty() { None } else { Some(rel) };
        }
        return None;
    }
    let mut norm = trimmed.replace('\\', "/");
    while let Some(rest) = norm.strip_prefix("./") {
        norm = rest.to_string();
    }
    let norm = norm.trim_matches('/').to_ascii_lowercase();
    if norm.is_empty() || norm == "." || norm.starts_with("..") {
        None
    } else {
        Some(norm)
    }
}

/// Extracts unquoted effective paths from a `git status --porcelain` path entry
/// (handling renames/copies like `old.rs -> new.rs` and quoted paths `"a b.rs" -> "c d.rs"`).
pub fn extract_effective_git_paths(raw_path: &str) -> Vec<String> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return vec![];
    }
    let unquote = |s: &str| {
        s.trim()
            .trim_matches('"')
            .replace('\\', "/")
            .trim_start_matches("./")
            .to_string()
    };
    if let Some((old_p, new_p)) = trimmed.split_once(" -> ") {
        let a = unquote(old_p);
        let b = unquote(new_p);
        let mut out = Vec::new();
        if !a.is_empty() {
            out.push(a);
        }
        if !b.is_empty() {
            out.push(b);
        }
        out
    } else {
        let p = unquote(trimmed);
        if p.is_empty() { vec![] } else { vec![p] }
    }
}

/// Returns true if `rel_path` inside `project_root` belongs exclusively to a different nested registered project.
/// For Git renames (`old -> new`), returns true ONLY when BOTH endpoints reside inside nested sibling project(s).
pub fn is_under_other_project(
    project_root: &Path,
    rel_path: &str,
    other_project_roots: &[PathBuf],
) -> bool {
    if other_project_roots.is_empty() {
        return false;
    }
    let paths = extract_effective_git_paths(rel_path);
    if paths.is_empty() {
        return false;
    }
    let self_key = normalize_root_key(project_root);
    let self_prefix = format!("{}\\", self_key);
    let nested_others: Vec<String> = other_project_roots
        .iter()
        .map(|o| normalize_root_key(o))
        .filter(|ok| !ok.is_empty() && ok != &self_key && ok.starts_with(&self_prefix))
        .collect();
    if nested_others.is_empty() {
        return false;
    }
    paths.iter().all(|p| {
        let full_key = normalize_root_key(&project_root.join(p));
        nested_others.iter().any(|ok| {
            let other_prefix = format!("{}\\", ok);
            full_key == *ok || full_key.starts_with(&other_prefix)
        })
    })
}

/// Semantically classify a changed file path relative to a project into:
/// `engine` | `config` | `docs` | `artifact` | `source` | `other`.
pub fn classify_change_path(rel_path: &str, project: &ProjectConfig) -> &'static str {
    let paths = extract_effective_git_paths(rel_path);
    // For renames (`old -> new`), classify by the destination path.
    let effective = paths.last().map(|s| s.as_str()).unwrap_or(rel_path);
    let norm = effective.trim().replace('\\', "/");
    let mut norm_trimmed = norm.as_str();
    while let Some(rest) = norm_trimmed.strip_prefix("./") {
        norm_trimmed = rest;
    }
    let low = norm_trimmed.to_ascii_lowercase();
    let file_name = low.rsplit('/').next().unwrap_or(&low);
    let proj_root = Path::new(&project.root);

    // 1. Sub-engine source directory
    if let Some(eng_dir) = normalize_project_rel_path(proj_root, &project.engine_src_dir) {
        if low == eng_dir || low.starts_with(&format!("{}/", eng_dir)) {
            return "engine";
        }
    }

    // 2. Release / build artifacts
    if let Some(rel_dir) = normalize_project_rel_path(proj_root, &project.release_dir) {
        if low == rel_dir || low.starts_with(&format!("{}/", rel_dir)) {
            return "artifact";
        }
    }

    // 3. Explicit version files & build/manifest configuration
    for vf in &project.version_files {
        if let Some(vp) = normalize_project_rel_path(proj_root, &vf.path) {
            if low == vp {
                return "config";
            }
        }
    }
    if low.starts_with(".github/")
        || matches!(
            file_name,
            "cargo.toml"
                | "cargo.lock"
                | "package.json"
                | "package-lock.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | "tauri.conf.json"
                | "pyproject.toml"
                | "requirements.txt"
                | "go.mod"
                | "go.sum"
                | "pubspec.yaml"
                | "pom.xml"
                | "build.gradle"
                | "build.gradle.kts"
                | "cmakelists.txt"
                | "makefile"
                | "build.ps1"
                | "publish-release.ps1"
                | "tsconfig.json"
                | "vite.config.js"
                | "vite.config.ts"
                | ".gitignore"
        )
        || file_name.ends_with(".csproj")
        || file_name.ends_with(".sln")
        || file_name.ends_with(".nsi")
        || file_name.ends_with(".wxs")
    {
        return "config";
    }

    // 4. Documentation & changelog
    let cl_path = project
        .changelog_path
        .trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase();
    if (!cl_path.is_empty() && low == cl_path)
        || low.starts_with("docs/")
        || low.starts_with("doc/")
        || matches!(
            file_name,
            "readme.md" | "changelog.md" | "license" | "license.md" | "license.txt"
        )
        || file_name.ends_with(".md")
        || file_name.ends_with(".rst")
    {
        return "docs";
    }

    // 5. Source code
    let ext = file_name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    if matches!(
        ext,
        "rs" | "js"
            | "mjs"
            | "cjs"
            | "ts"
            | "tsx"
            | "jsx"
            | "vue"
            | "svelte"
            | "css"
            | "scss"
            | "less"
            | "html"
            | "htm"
            | "py"
            | "pyw"
            | "go"
            | "cs"
            | "fs"
            | "vb"
            | "dart"
            | "c"
            | "cc"
            | "cpp"
            | "cxx"
            | "h"
            | "hpp"
            | "java"
            | "kt"
            | "kts"
            | "swift"
            | "m"
            | "mm"
            | "php"
            | "rb"
            | "lua"
            | "sql"
            | "sh"
            | "ps1"
            | "bat"
            | "cmd"
            | "wgsl"
            | "glsl"
    ) {
        return "source";
    }

    "other"
}

fn format_change_summary(counts: &HashMap<String, usize>, artifact_changed: bool) -> String {
    let order = [
        ("source", "源码"),
        ("engine", "子引擎"),
        ("config", "配置/版本"),
        ("docs", "文档"),
        ("other", "资源"),
    ];
    let mut parts = Vec::new();
    for (k, lbl) in order {
        let n = counts.get(k).copied().unwrap_or(0);
        if n > 0 {
            parts.push(format!("{} {} 项", lbl, n));
        }
    }
    if artifact_changed || counts.get("artifact").copied().unwrap_or(0) > 0 {
        parts.push("产物已变动".into());
    }
    parts.join(" · ")
}

/// Persistent per-project non-Git filesystem baseline snapshot path.
fn fs_baseline_path(project_id: &str) -> PathBuf {
    project_workspace_dir(project_id).join("fs_baseline.json")
}

/// Loads the persisted non-Git file metadata baseline (last build / seal / initial sync) for a project.
pub fn load_fs_baseline(project_id: &str) -> Option<HashMap<String, (u64, u64, u32)>> {
    let p = fs_baseline_path(project_id);
    let raw = fs::read_to_string(&p).ok()?;
    serde_json::from_str::<HashMap<String, (u64, u64, u32)>>(&raw).ok()
}

/// Atomically persists the non-Git file metadata baseline for a project.
pub fn save_fs_baseline(project_id: &str, snap: &HashMap<String, (u64, u64, u32)>) {
    let p = fs_baseline_path(project_id);
    if let Ok(bytes) = serde_json::to_vec(snap) {
        let _ = crate::guardrails::atomic_write_bytes(&p, &bytes);
    }
}

/// Removes persisted non-Git baseline (used when a project's root directory is changed).
pub fn clear_fs_baseline(project_id: &str) {
    let _ = fs::remove_file(fs_baseline_path(project_id));
}

/// Fast bounded filesystem snapshot for non-Git projects (e.g., local projects without `.git`).
/// Strictly skips build caches, release output, built executables (`launch_exe` / `engine_exe_path`),
/// build lock/journal files, OS/compiler temp noise, symlinks/junctions, and any nested sibling projects.
pub fn scan_non_git_workspace(
    root: &Path,
    project: &ProjectConfig,
    other_project_roots: &[PathBuf],
) -> (String, HashMap<String, (u64, u64, u32)>) {
    const MAX_FILES: usize = 8_000;
    const MAX_DEPTH: usize = 8;
    const SKIP_DIRS: &[&str] = &[
        ".git",
        "node_modules",
        "target",
        "dist",
        "build",
        ".build",
        "__pycache__",
        ".venv",
        "venv",
        "bin",
        "obj",
        ".idea",
        ".vscode",
        ".vs",
        ".cargo",
        ".next",
        ".nuxt",
        "coverage",
    ];
    const SKIP_FILE_NAMES: &[&str] = &[
        "thumbs.db",
        ".ds_store",
        "desktop.ini",
        ".shipforge.lock",
    ];
    const SKIP_EXTS: &[&str] = &[
        "pdb", "ilk", "exp", "obj", "o", "rlib", "rmeta", "tmp", "bak", "log", "swp", "swo",
    ];

    let rel_dir_clean = normalize_project_rel_path(root, &project.release_dir);
    let mut skip_rel_files: Vec<String> = Vec::new();
    for cand in [
        &project.launch_exe,
        &project.engine_exe_path,
        &project.lock_file,
        &project.journal_file,
    ] {
        if let Some(r) = normalize_project_rel_path(root, cand) {
            skip_rel_files.push(r);
        }
    }

    let self_key = normalize_root_key(root);
    let self_prefix = format!("{}\\", self_key);
    let nested_other_keys: Vec<String> = other_project_roots
        .iter()
        .map(|p| normalize_root_key(p))
        .filter(|k| !k.is_empty() && k != &self_key && k.starts_with(&self_prefix))
        .collect();

    let mut entries: Vec<(String, u64, u64, u32)> = Vec::new();
    let iter = WalkDir::new(root)
        .max_depth(MAX_DEPTH)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            if crate::guardrails::is_reparse_point(e.path()) {
                return false;
            }
            if e.file_type().is_dir() {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                if SKIP_DIRS.contains(&name.as_str()) {
                    return false;
                }
                if let Ok(rel) = e.path().strip_prefix(root) {
                    let rel_s = rel.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
                    if let Some(ref rd) = rel_dir_clean {
                        if rel_s == *rd || rel_s.starts_with(&format!("{}/", rd)) {
                            return false;
                        }
                    }
                }
                if !nested_other_keys.is_empty() {
                    let ek = normalize_root_key(e.path());
                    if nested_other_keys.iter().any(|ok| &ek == ok) {
                        return false;
                    }
                }
            }
            true
        });

    for entry in iter.flatten() {
        if entries.len() >= MAX_FILES {
            break;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel_s = rel.to_string_lossy().replace('\\', "/");
        if rel_s.is_empty() {
            continue;
        }
        let low = rel_s.to_ascii_lowercase();
        let fname = low.rsplit('/').next().unwrap_or(&low);
        if SKIP_FILE_NAMES.contains(&fname)
            || fname.starts_with(".shipforge")
            || skip_rel_files.iter().any(|s| s == &low)
        {
            continue;
        }
        if let Some((_, ext)) = fname.rsplit_once('.') {
            if SKIP_EXTS.contains(&ext) {
                continue;
            }
        }
        if let Ok(meta) = entry.metadata() {
            let size = meta.len();
            let (secs, nanos) = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| (d.as_secs(), d.subsec_nanos()))
                .unwrap_or((0, 0));
            entries.push((rel_s, size, secs, nanos));
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    let mut map = HashMap::with_capacity(entries.len());
    for (rel_s, size, secs, nanos) in entries {
        hasher.update(rel_s.as_bytes());
        hasher.update(b":");
        hasher.update(size.to_le_bytes());
        hasher.update(secs.to_le_bytes());
        hasher.update(nanos.to_le_bytes());
        hasher.update(b";");
        map.insert(rel_s, (size, secs, nanos));
    }
    let digest = hasher.finalize();
    let fp: String = digest.iter().take(12).map(|b| format!("{:02x}", b)).collect();
    (fp, map)
}

/// Diff two non-Git file metadata snapshots to produce exact added/modified/deleted file paths.
pub fn diff_non_git_snapshots(
    prev: &HashMap<String, (u64, u64, u32)>,
    curr: &HashMap<String, (u64, u64, u32)>,
) -> Vec<(String, String)> {
    let mut diffs = Vec::new();
    for (path, meta) in curr {
        match prev.get(path) {
            None => diffs.push(("A".to_string(), path.clone())),
            Some(old) if old != meta => diffs.push(("M".to_string(), path.clone())),
            _ => {}
        }
    }
    for path in prev.keys() {
        if !curr.contains_key(path) {
            diffs.push(("D".to_string(), path.clone()));
        }
    }
    diffs.sort_by(|a, b| a.1.cmp(&b.1));
    diffs
}

pub fn compute_artifact_fingerprint(artifact_summary: &str) -> String {
    short_sha256_hex(artifact_summary, 24)
}

#[allow(dead_code)]
pub fn compute_workspace_fingerprint(
    version: &str,
    head_sha: &str,
    dirty_files: &[String],
    root: &Path,
    artifact_summary: &str,
) -> String {
    compute_workspace_fingerprint_ext(version, head_sha, dirty_files, root, artifact_summary, None)
}

pub fn compute_workspace_fingerprint_ext(
    version: &str,
    head_sha: &str,
    dirty_files: &[String],
    root: &Path,
    artifact_summary: &str,
    non_git_source_fp: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(b"|");
    hasher.update(head_sha.as_bytes());
    hasher.update(b"|");
    if let Some(ng_fp) = non_git_source_fp {
        hasher.update(b"fs:");
        hasher.update(ng_fp.as_bytes());
        hasher.update(b"|");
    }
    let mut sorted = dirty_files.to_vec();
    sorted.sort();
    for f in &sorted {
        hasher.update(f.as_bytes());
        if let Ok(meta) = fs::metadata(root.join(f)) {
            hasher.update(meta.len().to_le_bytes());
            if let Ok(m) = meta.modified() {
                if let Ok(d) = m.duration_since(std::time::UNIX_EPOCH) {
                    hasher.update(d.as_secs().to_le_bytes());
                    hasher.update(d.subsec_nanos().to_le_bytes());
                }
            }
        }
        hasher.update(b";");
    }
    hasher.update(b"|");
    hasher.update(artifact_summary.as_bytes());
    let digest = hasher.finalize();
    digest.iter().take(12).map(|b| format!("{:02x}", b)).collect()
}

#[allow(clippy::too_many_arguments)]
pub fn build_change_report_classified(
    project: &ProjectConfig,
    root: &Path,
    current_version: &str,
    head_sha: &str,
    scan_mode: &str,
    raw_changes: Vec<(String, String)>,
    non_git_source_fp: Option<&str>,
    engine_stale: bool,
    artifact_count: usize,
    artifact_summary: &str,
    prev_fingerprint: Option<&str>,
    prev_version: Option<&str>,
    prev_artifact_fp: Option<&str>,
) -> ChangeReport {
    let dirty_files: Vec<String> = raw_changes.iter().map(|(_, p)| p.clone()).collect();
    let artifact_fingerprint = compute_artifact_fingerprint(artifact_summary);
    let source_fingerprint = if let Some(ng) = non_git_source_fp {
        ng.to_string()
    } else {
        compute_workspace_fingerprint_ext(current_version, head_sha, &dirty_files, root, "", None)
    };
    let fingerprint = compute_workspace_fingerprint_ext(
        current_version,
        head_sha,
        &dirty_files,
        root,
        artifact_summary,
        non_git_source_fp,
    );

    let mut classified_changes: Vec<ClassifiedChangeItem> = Vec::with_capacity(raw_changes.len());
    let mut category_counts: HashMap<String, usize> = HashMap::new();
    for (status, path) in &raw_changes {
        let cat = classify_change_path(path, project);
        *category_counts.entry(cat.to_string()).or_insert(0) += 1;
        classified_changes.push(ClassifiedChangeItem {
            path: path.clone(),
            status: status.clone(),
            category: cat.to_string(),
        });
    }

    let artifact_changed = prev_artifact_fp
        .map(|pa| !pa.is_empty() && pa != artifact_fingerprint)
        .unwrap_or(false);
    let change_summary = format_change_summary(&category_counts, artifact_changed);

    let mut bucket = load_version_bucket(&project.id, current_version);

    let bucket_existed = bucket_path(&project.id, current_version).exists();
    let mut bucket_dirty = !bucket_existed;
    if bucket.base_git_sha.is_empty() && !head_sha.is_empty() {
        bucket.base_git_sha = head_sha.to_string();
        bucket_dirty = true;
    }
    // CRITICAL VERSION ISOLATION RULE:
    // Sealed version buckets are IMMUTABLE snapshots; post-seal uncommitted edits MUST NOT overwrite `bucket.changed_files`!
    if !bucket.sealed && bucket.changed_files != dirty_files {
        bucket.changed_files = dirty_files.clone();
        bucket_dirty = true;
    }
    if bucket_dirty {
        bucket.updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let _ = save_version_bucket(&project.id, &bucket);
    }

    let changed_since_last_build = !bucket.last_built_fingerprint.is_empty()
        && (bucket.last_built_fingerprint != fingerprint
            || (!bucket.last_built_git_sha.is_empty()
                && !head_sha.is_empty()
                && bucket.last_built_git_sha != head_sha));

    let version_changed = prev_version
        .map(|pv| !pv.is_empty() && pv != current_version)
        .unwrap_or(false);
    let has_external_change = prev_fingerprint
        .map(|pf| !pf.is_empty() && pf != fingerprint)
        .unwrap_or(false);

    let consecutive_warning = if bucket.sealed && (!dirty_files.is_empty() || changed_since_last_build)
    {
        if !change_summary.is_empty() {
            format!(
                "v{} 已封版，检测到后续新改动（{}），请递增版本号以免混淆更新数据",
                current_version, change_summary
            )
        } else if !dirty_files.is_empty() {
            format!(
                "v{} 已封版，检测到后续新改动（{} 个未提交文件），请递增版本号以免混淆更新数据",
                current_version,
                dirty_files.len()
            )
        } else {
            format!(
                "v{} 已封版，检测到源码或产物指纹相对上次构建已变化，请递增版本号以免混淆更新数据",
                current_version
            )
        }
    } else if changed_since_last_build {
        format!(
            "检测到连续修改：v{} 已构建 {} 次，此后源码或产物再次变动{}，建议递增版本保持版本隔离",
            current_version,
            bucket.build_count,
            if change_summary.is_empty() {
                String::new()
            } else {
                format!("（{}）", change_summary)
            }
        )
    } else {
        String::new()
    };

    ChangeReport {
        project_id: project.id.clone(),
        project_name: project.name.clone(),
        scan_mode: scan_mode.to_string(),
        current_version: current_version.to_string(),
        head_sha: head_sha.to_string(),
        dirty_count: dirty_files.len(),
        dirty_files,
        classified_changes,
        category_counts,
        change_summary,
        artifact_changed,
        source_fingerprint,
        artifact_fingerprint,
        engine_stale,
        artifact_count,
        fingerprint,
        version_changed,
        has_external_change,
        changed_since_last_build,
        consecutive_warning,
        bucket,
        deploy_readiness: None,
    }
}

#[allow(dead_code, clippy::too_many_arguments)]
pub fn build_change_report(
    project_id: &str,
    root: &Path,
    current_version: &str,
    head_sha: &str,
    dirty_files: Vec<String>,
    engine_stale: bool,
    artifact_count: usize,
    artifact_summary: &str,
    prev_fingerprint: Option<&str>,
    prev_version: Option<&str>,
) -> ChangeReport {
    let dummy = ProjectConfig {
        id: project_id.to_string(),
        name: project_id.to_string(),
        root: root.to_string_lossy().to_string(),
        kind: String::new(),
        adapter_label: String::new(),
        primary_language: String::new(),
        detected_languages: vec![],
        shell_type: String::new(),
        product_name: String::new(),
        artifact_prefix: String::new(),
        release_dir: "release".into(),
        changelog_path: "CHANGELOG.md".into(),
        github_repo: String::new(),
        repo_binding: Default::default(),
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
    let raw_changes = dirty_files.into_iter().map(|p| ("M".to_string(), p)).collect();
    build_change_report_classified(
        &dummy,
        root,
        current_version,
        head_sha,
        "git",
        raw_changes,
        None,
        engine_stale,
        artifact_count,
        artifact_summary,
        prev_fingerprint,
        prev_version,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_migration_target_blocks_unsafe_paths() {
        let old = Path::new(r"D:\Dev\ShipForgeData");
        assert!(validate_migration_target(old, Path::new(r"D:\")).is_err());
        assert!(validate_migration_target(old, Path::new(r"C:\Windows\Temp\sf")).is_err());
        assert!(validate_migration_target(old, Path::new(r"D:\Dev\ShipForgeData")).is_err());
        assert!(validate_migration_target(old, Path::new(r"D:\Dev\ShipForgeData\sub")).is_err());
        assert!(validate_migration_target(old, Path::new(r"D:\Dev")).is_err());
        assert!(validate_migration_target(old, Path::new(r"E:\Work\ShipForgeData")).is_ok());
    }

    #[test]
    fn test_version_isolation_buckets_do_not_bleed() {
        let pid = format!("test-iso-{}", chrono::Local::now().timestamp_nanos_opt().unwrap_or(5));
        let mut b1 = load_version_bucket(&pid, "1.0.0");
        b1.notes_draft = "v1.0.0 isolated notes".into();
        b1.build_count = 1;
        b1.last_built_fingerprint = "fp-100".into();
        save_version_bucket(&pid, &b1).unwrap();

        let mut b2 = load_version_bucket(&pid, "1.0.1");
        b2.notes_draft = "v1.0.1 isolated notes".into();
        save_version_bucket(&pid, &b2).unwrap();

        assert_eq!(load_version_bucket(&pid, "1.0.0").notes_draft, "v1.0.0 isolated notes");
        assert_eq!(load_version_bucket(&pid, "1.0.1").notes_draft, "v1.0.1 isolated notes");
        assert_eq!(load_version_bucket(&pid, "1.0.1").build_count, 0);

        let _ = fs::remove_dir_all(workspaces_dir().join(project_workspace_key(&pid)));
    }

    #[test]
    fn test_migrate_between_roots_verifies_sha256_and_preserves_unrelated_files() {
        let base = std::env::temp_dir().join(format!(
            "shipforge-mig-test-{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(9)
        ));
        let old_root = base.join("old-root");
        let new_root = base.join("new-root");
        let ptr_file = base.join("bootstrap").join("pointer.json");
        let default_dir = base.join("default-dir");

        // Populate ShipForge subtrees
        fs::create_dir_all(old_root.join("config")).unwrap();
        fs::create_dir_all(old_root.join("tools").join("nsis")).unwrap();
        fs::create_dir_all(old_root.join("cache")).unwrap();
        fs::write(old_root.join("config").join("config.json"), b"{\"version\":1}").unwrap();
        fs::write(old_root.join("tools").join("nsis").join("makensis.exe"), b"MZ-dummy-binary").unwrap();
        fs::write(old_root.join("cache").join("tmp.dat"), b"cache-bytes").unwrap();

        let mut progress_steps = Vec::new();
        migrate_between_roots(&old_root, &new_root, &ptr_file, &default_dir, |p, _| {
            progress_steps.push(p);
        })
        .unwrap();

        // 1. All files arrived intact with sentinel + pointer updated
        assert_eq!(fs::read(new_root.join("config").join("config.json")).unwrap(), b"{\"version\":1}");
        assert_eq!(fs::read(new_root.join("tools").join("nsis").join("makensis.exe")).unwrap(), b"MZ-dummy-binary");
        assert!(new_root.join(".shipforge-root").exists());
        let ptr_txt = fs::read_to_string(&ptr_file).unwrap();
        assert!(ptr_txt.contains("new-root"));
        // 2. Empty old_root was cleanly removed
        assert!(!old_root.exists());
        assert_eq!(*progress_steps.last().unwrap(), 100);

        // 3. Migrate again when an unrelated user file exists inside current root -> user file MUST survive!
        let third_root = base.join("third-root");
        fs::write(new_root.join("user_precious_notes.txt"), b"DO NOT DELETE").unwrap();
        migrate_between_roots(&new_root, &third_root, &ptr_file, &default_dir, |_, _| {}).unwrap();
        assert!(third_root.join("config").join("config.json").exists());
        assert!(!new_root.join("config").exists());
        assert!(!new_root.join("tools").exists());
        assert_eq!(fs::read(new_root.join("user_precious_notes.txt")).unwrap(), b"DO NOT DELETE");

        // 4. Attempting to migrate into a non-empty unrelated folder must be rejected
        let non_empty = base.join("unrelated-nonempty");
        fs::create_dir_all(&non_empty).unwrap();
        fs::write(non_empty.join("foo.txt"), b"hello").unwrap();
        assert!(validate_migration_target(&third_root, &non_empty).is_err());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_copy_tree_verified_and_managed_cache_env_vars() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_tree_test_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(17)
        ));
        let src = base.join("src_nsis");
        let dst = base.join("dst_nsis");
        fs::create_dir_all(src.join("Stubs")).unwrap();
        fs::create_dir_all(src.join("Include")).unwrap();
        fs::write(src.join("makensis.exe"), b"MZ-nsis-binary").unwrap();
        fs::write(src.join("Stubs").join("zlib"), b"stub-bytes").unwrap();
        fs::write(src.join("Include").join("MUI2.nsh"), b"!macro MUI").unwrap();

        let count = copy_tree_verified(&src, &dst).unwrap();
        assert_eq!(count, 3);
        assert!(nsis_complete(&dst));
        assert!(dir_has_executable(&dst));

        let empty_dir = base.join("empty_tool");
        fs::create_dir_all(&empty_dir).unwrap();
        assert!(!dir_has_executable(&empty_dir));

        let env_vars = managed_cache_env_vars();
        assert!(env_vars.iter().any(|(k, _)| k == "NPM_CONFIG_CACHE"));
        assert!(env_vars.iter().any(|(k, _)| k == "PIP_CACHE_DIR"));
        assert!(env_vars.iter().any(|(k, _)| k == "UV_CACHE_DIR"));

        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(windows)]
    #[test]
    fn test_ntfs_junction_replace_and_retarget_a_to_b_to_c() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_junc_test_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(19)
        ));
        let dir_a = base.join("c_cache").join("NSIS");
        let dir_b = base.join("root_b").join("tools").join("nsis");
        let dir_c = base.join("root_c").join("tools").join("nsis");

        fs::create_dir_all(dir_a.join("Stubs")).unwrap();
        fs::create_dir_all(dir_a.join("Include")).unwrap();
        fs::write(dir_a.join("makensis.exe"), b"MZ-nsis-v3").unwrap();

        // Step 1: Migrate A -> B and replace A with a 0-byte NTFS Junction
        copy_tree_verified(&dir_a, &dir_b).unwrap();
        assert!(replace_dir_with_junction_safe(&dir_a, &dir_b));
        assert!(is_reparse_point(&dir_a));
        assert_eq!(fs::read(dir_a.join("makensis.exe")).unwrap(), b"MZ-nsis-v3");

        // Idempotent retarget check when already aligned -> Ok(false)
        assert_eq!(retarget_junction_safe(&dir_a, &dir_b).unwrap(), false);

        // Step 2: Migrate B -> C and DELETE B (A becomes dangling!), then retarget A -> C
        copy_tree_verified(&dir_b, &dir_c).unwrap();
        fs::write(dir_c.join("makensis.exe"), b"MZ-nsis-v3-on-c").unwrap();
        fs::remove_dir_all(&dir_b).unwrap();
        assert!(!dir_a.join("makensis.exe").exists());

        assert_eq!(retarget_junction_safe(&dir_a, &dir_c).unwrap(), true);
        assert!(is_reparse_point(&dir_a));
        assert_eq!(fs::read(dir_a.join("makensis.exe")).unwrap(), b"MZ-nsis-v3-on-c");

        // Clean up junction safely before remove_dir_all
        let _ = fs::remove_dir(&dir_a);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_workspace_key_collision_resistance_and_owner_isolation() {
        // Legacy clean lowercase IDs remain stable
        assert_eq!(project_workspace_key("tidyassistant"), "tidyassistant");
        assert_eq!(project_workspace_key("shipforge"), "shipforge");

        // Case / punctuation / reserved-name collisions MUST resolve to distinct workspace keys
        assert_ne!(project_workspace_key("MyApp"), project_workspace_key("myapp"));
        assert_ne!(project_workspace_key("my.app"), project_workspace_key("my_app"));
        assert_ne!(project_workspace_key("CON"), "CON");
        assert_ne!(project_workspace_key("nul"), "nul");

        // Workspace owner binding prevents newly imported projects from inheriting orphaned workspaces
        let ts = chrono::Local::now().timestamp_nanos_opt().unwrap_or(99);
        let pid = format!("test-owner-iso-{}", ts);
        let root_a = std::env::temp_dir().join(format!("sf_owner_a_{}", ts));
        let root_b = std::env::temp_dir().join(format!("sf_owner_b_{}", ts));
        fs::create_dir_all(&root_a).unwrap();
        fs::create_dir_all(&root_b).unwrap();

        bind_workspace_owner(&pid, &root_a);
        assert!(!workspace_owned_by_other_root(&pid, &root_a));
        assert!(workspace_owned_by_other_root(&pid, &root_b));

        let _ = fs::remove_dir_all(project_workspace_dir(&pid));
        let _ = fs::remove_dir_all(&root_a);
        let _ = fs::remove_dir_all(&root_b);
    }

    #[test]
    fn test_nested_subproject_isolation_and_git_rename_handling() {
        let ts = chrono::Local::now().timestamp_nanos_opt().unwrap_or(101);
        let root_a = std::env::temp_dir().join(format!("sf_mono_{}", ts));
        let root_b = root_a.join("packages").join("sub_app");
        fs::create_dir_all(root_b.join("src")).unwrap();
        fs::create_dir_all(root_a.join("src")).unwrap();

        let others = vec![root_b.clone()];
        assert!(is_under_other_project(&root_a, "packages/sub_app/src/main.rs", &others));
        assert!(!is_under_other_project(&root_a, "src/main.rs", &others));
        // Pure rename inside sub-project -> excluded
        assert!(is_under_other_project(
            &root_a,
            "packages/sub_app/src/old.rs -> packages/sub_app/src/new.rs",
            &others
        ));
        // Cross-boundary rename touching parent root -> MUST NOT be dropped
        assert!(!is_under_other_project(
            &root_a,
            "src/shared.rs -> packages/sub_app/src/shared.rs",
            &others
        ));
        // Quoted Git path inside sub-project -> excluded
        assert!(is_under_other_project(
            &root_a,
            "\"packages/sub_app/src/中文 文件.rs\"",
            &others
        ));

        let _ = fs::remove_dir_all(&root_a);
    }

    #[test]
    fn test_semantic_change_classification_and_sealed_immutability() {
        let ts = chrono::Local::now().timestamp_nanos_opt().unwrap_or(102);
        let root = std::env::temp_dir().join(format!("sf_cls_{}", ts));
        fs::create_dir_all(root.join("src")).unwrap();
        let pid = format!("test-cls-iso-{}", ts);

        let mut proj = ProjectConfig::default();
        proj.id = pid.clone();
        proj.name = "DemoApp".into();
        proj.root = root.to_string_lossy().to_string();
        proj.release_dir = "./release".into();
        proj.engine_src_dir = "./src/engine".into();
        proj.changelog_path = "CHANGELOG.md".into();
        proj.version_files.push(crate::models::VersionFileSpec {
            path: "src/custom_ver.txt".into(),
            format: "regex".into(),
            pattern: r"v(\d+\.\d+\.\d+)".into(),
        });

        assert_eq!(classify_change_path("src/engine/worker.py", &proj), "engine");
        assert_eq!(classify_change_path("release/app-v1.0.0.exe", &proj), "artifact");
        assert_eq!(classify_change_path("src/custom_ver.txt", &proj), "config");
        assert_eq!(classify_change_path("Cargo.toml", &proj), "config");
        assert_eq!(classify_change_path("docs/guide.md", &proj), "docs");
        assert_eq!(classify_change_path("docs/old.md -> src/main.rs", &proj), "source");
        assert_eq!(classify_change_path("\"src/中文 模块.ts\"", &proj), "source");
        assert_eq!(classify_change_path("assets/logo.png", &proj), "other");

        // Seal v1.0.0 with initial file list
        let mut b = load_version_bucket(&pid, "1.0.0");
        b.sealed = true;
        b.build_count = 1;
        b.last_built_fingerprint = "fp-old".into();
        b.changed_files = vec!["src/initial.rs".into()];
        save_version_bucket(&pid, &b).unwrap();

        // Post-seal edits MUST NOT overwrite sealed bucket's `changed_files` on disk!
        let rep = build_change_report_classified(
            &proj,
            &root,
            "1.0.0",
            "abc1234",
            "git",
            vec![("M".into(), "src/post_seal.rs".into())],
            None,
            false,
            0,
            "",
            None,
            None,
            None,
        );
        assert_eq!(rep.bucket.changed_files, vec!["src/initial.rs".to_string()]);
        assert!(rep.consecutive_warning.contains("源码 1 项"));

        // Red-Team R2: when sealed + fingerprint changed + empty dirty_files, NEVER print "0 个未提交文件"
        let rep_empty = build_change_report_classified(
            &proj,
            &root,
            "1.0.0",
            "abc1234",
            "filesystem",
            vec![],
            Some("new-fp"),
            false,
            0,
            "",
            None,
            None,
            None,
        );
        assert!(!rep_empty.consecutive_warning.contains("0 个未提交文件"));
        assert!(rep_empty.consecutive_warning.contains("指纹"));

        let _ = fs::remove_dir_all(project_workspace_dir(&pid));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_non_git_workspace_scan_filters_noise_and_nested_projects() {
        let ts = chrono::Local::now().timestamp_nanos_opt().unwrap_or(103);
        let root = std::env::temp_dir().join(format!("sf_nongit_{}", ts));
        let nested = root.join("nested_other");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("release")).unwrap();
        fs::create_dir_all(&nested).unwrap();

        fs::write(root.join("src").join("app.rs"), "fn main() {}").unwrap();
        fs::write(root.join("App.exe"), b"MZ-old").unwrap();
        fs::write(root.join("debug.pdb"), b"pdb").unwrap();
        fs::write(root.join("release").join("pkg.zip"), b"zip").unwrap();
        fs::write(nested.join("other.rs"), "fn other() {}").unwrap();

        let mut proj = ProjectConfig::default();
        proj.id = format!("test-nongit-{}", ts);
        proj.root = root.to_string_lossy().to_string();
        proj.release_dir = "./release".into();
        proj.launch_exe = "App.exe".into();

        let others = vec![nested.clone()];
        let (fp1, snap1) = scan_non_git_workspace(&root, &proj, &others);
        assert!(snap1.contains_key("src/app.rs"));
        assert!(!snap1.contains_key("App.exe"), "launch_exe must be excluded from source scan");
        assert!(!snap1.contains_key("debug.pdb"), ".pdb must be excluded");
        assert!(!snap1.contains_key("release/pkg.zip"), "release_dir must be excluded");
        assert!(!snap1.contains_key("nested_other/other.rs"), "nested project must be excluded");

        // Modifying excluded noise & nested project MUST NOT change fingerprint
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(root.join("App.exe"), b"MZ-rebuilt-bigger").unwrap();
        fs::write(nested.join("other.rs"), "fn other() { println!(\"changed\"); }").unwrap();
        let (fp2, snap2) = scan_non_git_workspace(&root, &proj, &others);
        assert_eq!(fp1, fp2);
        assert!(diff_non_git_snapshots(&snap1, &snap2).is_empty());

        // Modifying real source & adding new file MUST be detected accurately
        fs::write(root.join("src").join("app.rs"), "fn main() { println!(\"v2\"); }").unwrap();
        fs::write(root.join("src").join("new.rs"), "pub fn x() {}").unwrap();
        let (fp3, snap3) = scan_non_git_workspace(&root, &proj, &others);
        assert_ne!(fp1, fp3);
        let diffs = diff_non_git_snapshots(&snap1, &snap3);
        assert_eq!(
            diffs,
            vec![
                ("M".to_string(), "src/app.rs".to_string()),
                ("A".to_string(), "src/new.rs".to_string()),
            ]
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_cfg_test_active_data_root_is_isolated_from_production() {
        let root = active_data_root();
        let tmp = std::env::temp_dir();
        assert!(
            root.starts_with(&tmp),
            "Unit tests must NEVER touch live user ShipForge data root, got: {:?}",
            root
        );
    }
}
