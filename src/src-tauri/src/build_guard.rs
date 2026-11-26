use crate::models::{BuildJournalEntry, BuildLockInfo, EngineStaleInfo};
use anyhow::{anyhow, Result};
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

#[cfg(windows)]
fn is_process_alive(pid: u32, expected_start_ticks: i64) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return false;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut kernel = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut user = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };

        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);

        if ok == 0 {
            return false;
        }

        let exit_u64 = ((exit.dwHighDateTime as u64) << 32) | (exit.dwLowDateTime as u64);
        if exit_u64 != 0 {
            return false;
        }

        if expected_start_ticks > 0 {
            // Convert Win32 UTC FILETIME (since 1601-01-01) -> .NET local DateTime.Ticks (since 0001-01-01)
            let ft_u64 =
                ((creation.dwHighDateTime as u64) << 32) | (creation.dwLowDateTime as u64);
            let tz_offset_secs = chrono::Local::now().offset().local_minus_utc() as i128;
            let utc_dotnet_ticks = ft_u64 as i128 + 504_911_232_000_000_000i128;
            let local_dotnet_ticks = utc_dotnet_ticks + tz_offset_secs * 10_000_000i128;

            // Match against either local ticks (PowerShell StartTime.Ticks) or UTC ticks within tight 5s tolerance
            let tol = 5i128 * 10_000_000i128;
            let diff_local = (local_dotnet_ticks - expected_start_ticks as i128).abs();
            let diff_utc = (utc_dotnet_ticks - expected_start_ticks as i128).abs();
            return diff_local <= tol || diff_utc <= tol;
        }
        true
    }
}

#[cfg(not(windows))]
fn is_process_alive(_pid: u32, _expected_start_ticks: i64) -> bool {
    false
}

pub fn inspect_lock(lock_path: &str) -> Option<BuildLockInfo> {
    if lock_path.trim().is_empty() {
        return None;
    }
    let p = Path::new(lock_path);
    if !p.exists() {
        return Some(BuildLockInfo {
            path: lock_path.to_string(),
            exists: false,
            alive: false,
            pid: 0,
            session: String::new(),
            version: String::new(),
            mode: String::new(),
            started_at: String::new(),
            host: String::new(),
        });
    }

    let raw = fs::read_to_string(p).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(raw.trim_start_matches('\u{feff}'))
        .unwrap_or_else(|_| serde_json::json!({}));
    let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    let ticks = v.get("startTicks").and_then(|x| x.as_i64()).unwrap_or(0);
    let alive = is_process_alive(pid, ticks);

    Some(BuildLockInfo {
        path: lock_path.to_string(),
        exists: true,
        alive,
        pid,
        session: v
            .get("session")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        version: v
            .get("version")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        mode: v
            .get("mode")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        started_at: v
            .get("startedAt")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        host: v
            .get("host")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

pub fn clean_stale_lock(lock_path: &str) -> Result<String> {
    let info = inspect_lock(lock_path).ok_or_else(|| anyhow!("未配置锁文件路径"))?;
    if !info.exists {
        return Ok("当前没有锁文件".into());
    }
    if info.alive {
        return Err(anyhow!(
            "构建锁仍被活跃进程占用（PID={}，版本={}），禁止强制清理",
            info.pid,
            info.version
        ));
    }
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let stale_name = format!("{}.stale-{}", lock_path, ts);
    fs::rename(lock_path, &stale_name)?;
    Ok(format!("已归档残留死锁为 {}", stale_name))
}

pub fn check_engine_stale(
    root: &Path,
    engine_src_rel: &str,
    engine_exe_rel: &str,
) -> Option<EngineStaleInfo> {
    if engine_src_rel.trim().is_empty() || engine_exe_rel.trim().is_empty() {
        return None;
    }
    let src_dir = root.join(engine_src_rel);
    let exe_path = root.join(engine_exe_rel);
    if !src_dir.exists() {
        return None;
    }
    if !exe_path.exists() {
        return Some(EngineStaleInfo {
            stale: true,
            newer_count: 1,
            newest_file: engine_exe_rel.to_string(),
            engine_exe_exists: false,
        });
    }

    let built_mtime = fs::metadata(&exe_path).ok()?.modified().ok()?;
    let exts = ["py", "spec", "json", "yaml", "yml", "txt", "rs", "toml"];
    let skip = ["数据", "cache", "__pycache__", ".pytest_cache", "target", "dist", "build"];

    let mut newer_count = 0usize;
    let mut newest_time = built_mtime;
    let mut newest_file = String::new();

    for entry in WalkDir::new(&src_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() > 0 && crate::guardrails::is_reparse_point(e.path()) {
                return false;
            }
            let name = e.file_name().to_string_lossy();
            !skip.iter().any(|s| s.eq_ignore_ascii_case(&name))
        })
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let file_name_lower = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if (file_name_lower.starts_with("test_") && file_name_lower.ends_with(".py"))
            || file_name_lower.ends_with("_test.py")
        {
            continue;
        }
        let ext = entry
            .path()
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if !exts.contains(&ext.as_str()) {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                if mtime > built_mtime {
                    newer_count += 1;
                    if mtime > newest_time {
                        newest_time = mtime;
                        newest_file = entry
                            .path()
                            .strip_prefix(root)
                            .unwrap_or(entry.path())
                            .to_string_lossy()
                            .replace('/', "\\");
                    }
                }
            }
        }
    }

    Some(EngineStaleInfo {
        stale: newer_count > 0,
        newer_count,
        newest_file,
        engine_exe_exists: true,
    })
}

fn normalize_journal_root(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let s = trimmed
        .strip_prefix(r"\\?\")
        .unwrap_or(trimmed)
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_string();
    #[cfg(windows)]
    {
        s.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        s
    }
}

pub fn read_journal_entries(
    project_journal: &str,
    project_root: &str,
    shipforge_journal: &Path,
    project_id: &str,
    limit: usize,
) -> Vec<BuildJournalEntry> {
    let mut list: Vec<BuildJournalEntry> = Vec::new();
    let norm_root = normalize_journal_root(project_root);

    if !project_journal.trim().is_empty() {
        let p = Path::new(project_journal);
        if p.exists() {
            let pj_norm = normalize_journal_root(project_journal);
            let is_private_journal = !norm_root.is_empty()
                && (pj_norm == norm_root || pj_norm.starts_with(&format!("{}\\", norm_root)));
            collect_jsonl(
                p,
                "project",
                None,
                Some(&norm_root),
                is_private_journal,
                &mut list,
            );
        }
    }

    // Fallback: D:\Builds\builds.jsonl filtered strictly by normalized project root
    if list.is_empty() {
        let global_builds = Path::new(r"D:\Builds\builds.jsonl");
        if global_builds.exists() {
            collect_jsonl(global_builds, "global", None, Some(&norm_root), false, &mut list);
        }
    }

    if shipforge_journal.exists() {
        collect_jsonl(
            shipforge_journal,
            "shipforge",
            Some(project_id),
            Some(&norm_root),
            true,
            &mut list,
        );
    }

    list.sort_by(|a, b| b.ts.cmp(&a.ts));
    list.truncate(limit);
    list
}

fn collect_jsonl(
    path: &Path,
    source: &str,
    filter_project_id: Option<&str>,
    filter_root: Option<&str>,
    allow_missing_root: bool,
    out: &mut Vec<BuildJournalEntry>,
) {
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(t) else {
            continue;
        };
        let pid = v
            .get("projectId")
            .or_else(|| v.get("project_id"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        if let Some(expected) = filter_project_id {
            if pid.as_deref() != Some(expected) {
                continue;
            }
        }
        if let Some(expected_root) = filter_root {
            if !expected_root.is_empty() {
                let raw_r = v
                    .get("projectRoot")
                    .or_else(|| v.get("root"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let norm_r = normalize_journal_root(raw_r);
                if norm_r.is_empty() {
                    if !allow_missing_root {
                        continue;
                    }
                } else if norm_r != expected_root {
                    continue;
                }
            }
        }
        out.push(BuildJournalEntry {
            ts: v
                .get("ts")
                .or_else(|| v.get("startedAt"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            version: v
                .get("version")
                .and_then(|x| x.as_str())
                .unwrap_or("-")
                .to_string(),
            mode: v
                .get("mode")
                .or_else(|| v.get("action"))
                .and_then(|x| x.as_str())
                .unwrap_or("-")
                .to_string(),
            result: v
                .get("result")
                .and_then(|x| x.as_str())
                .unwrap_or("ok")
                .to_string(),
            duration: v
                .get("duration")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            git: v
                .get("git")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            note: v
                .get("note")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            source: source.to_string(),
            project_id: pid,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_process_alive_with_tight_tolerance() {
        let my_pid = std::process::id();
        assert!(is_process_alive(my_pid, 0));
        // Wrong startTicks (1 hour off) must be rejected as PID reuse!
        assert!(!is_process_alive(my_pid, 1000));
        // Non-existent PID
        assert!(!is_process_alive(0, 0));
    }

    #[test]
    fn test_check_engine_stale_ignores_python_tests_and_markdown() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_engine_stale_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(11)
        ));
        let eng_src = base.join("src").join("engine");
        let eng_out = base.join("dist_engine");
        fs::create_dir_all(&eng_src).unwrap();
        fs::create_dir_all(&eng_out).unwrap();
        fs::write(eng_src.join("tidy.py"), "print('v1')").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(25));
        fs::write(eng_out.join("engine.exe"), b"exe").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(25));

        // Modifying test_*.py, *_test.py, or README.md must NOT mark engine stale
        fs::write(eng_src.join("test_engine_rpc.py"), "assert True").unwrap();
        fs::write(eng_src.join("helper_test.py"), "assert True").unwrap();
        fs::write(eng_src.join("README.md"), "# notes").unwrap();
        let clean = check_engine_stale(&base, "src/engine", "dist_engine/engine.exe").unwrap();
        assert!(!clean.stale, "test/md files must not mark engine stale: {:?}", clean);

        // Modifying actual runtime source MUST mark engine stale
        fs::write(eng_src.join("tidy.py"), "print('v2')").unwrap();
        let dirty = check_engine_stale(&base, "src/engine", "dist_engine/engine.exe").unwrap();
        assert!(dirty.stale);
        assert_eq!(dirty.newer_count, 1);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_journal_cross_project_isolation_and_shared_journal_guard() {
        let ts = chrono::Local::now().timestamp_nanos_opt().unwrap_or(77);
        let base = std::env::temp_dir().join(format!("sf_journal_iso_{}", ts));
        let root_a = base.join("AppA");
        let root_b = base.join("AppB");
        fs::create_dir_all(&root_a).unwrap();
        fs::create_dir_all(&root_b).unwrap();

        let shared_j = base.join("shared_builds.jsonl");
        let sf_j = base.join("shipforge_journal.jsonl");
        let lines = format!(
            concat!(
                "{{\"ts\":\"2026-01-01 10:00:00\",\"version\":\"1.0.0\",\"root\":\"{}\",\"status\":\"ok\"}}\n",
                "{{\"ts\":\"2026-01-01 10:05:00\",\"version\":\"2.0.0\",\"root\":\"{}\",\"status\":\"ok\"}}\n",
                "{{\"ts\":\"2026-01-01 10:10:00\",\"version\":\"9.9.9\",\"status\":\"ok\"}}\n"
            ),
            root_a.to_string_lossy().replace('\\', "\\\\"),
            root_b.to_string_lossy().replace('\\', "\\\\"),
        );
        fs::write(&shared_j, lines).unwrap();

        // ShipForge journal has projectId + projectRoot; recycled ID from another root must be filtered out!
        let sf_lines = format!(
            concat!(
                "{{\"ts\":\"2026-01-01 11:00:00\",\"projectId\":\"app\",\"projectRoot\":\"{}\",\"version\":\"1.0.1\",\"status\":\"ok\"}}\n",
                "{{\"ts\":\"2026-01-01 11:05:00\",\"projectId\":\"app\",\"projectRoot\":\"{}\",\"version\":\"2.0.1\",\"status\":\"ok\"}}\n"
            ),
            root_a.to_string_lossy().replace('\\', "\\\\"),
            root_b.to_string_lossy().replace('\\', "\\\\"),
        );
        fs::write(&sf_j, sf_lines).unwrap();

        let entries_a = read_journal_entries(
            &shared_j.to_string_lossy(),
            &root_a.to_string_lossy(),
            &sf_j,
            "app",
            20,
        );
        let versions_a: Vec<&str> = entries_a.iter().map(|e| e.version.as_str()).collect();
        assert!(versions_a.contains(&"1.0.0"), "AppA must see its own shared entry");
        assert!(versions_a.contains(&"1.0.1"), "AppA must see its own shipforge entry");
        assert!(!versions_a.contains(&"2.0.0"), "AppB shared entry must NEVER leak into AppA");
        assert!(!versions_a.contains(&"2.0.1"), "AppB recycled-id shipforge entry must NEVER leak into AppA");
        assert!(!versions_a.contains(&"9.9.9"), "Untagged entry in external shared journal must NEVER leak into AppA");

        let _ = fs::remove_dir_all(&base);
    }
}
