use crate::guardrails::atomic_write_bytes;
use crate::models::{ArtifactInfo, ClassifiedArtifact};
use crate::packager::{scan_artifacts, sha256_file};
use crate::storage;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ArtifactMetaRecord {
    stage: String,
    sha256_short: String,
    published_url: String,
    published_provider: String,
    updated_at: String,
    #[serde(default)]
    file_size: u64,
    #[serde(default)]
    file_mtime_unix_ms: u128,
}

fn file_mtime_unix_ms(p: &Path) -> u128 {
    fs::metadata(p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ArtifactMetaStore {
    items: HashMap<String, ArtifactMetaRecord>,
}

fn meta_path(project_id: &str) -> PathBuf {
    storage::project_workspace_dir(project_id).join("artifacts_meta.json")
}

fn load_store(project_id: &str) -> ArtifactMetaStore {
    let p = meta_path(project_id);
    fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_store(project_id: &str, store: &ArtifactMetaStore) -> Result<()> {
    let p = meta_path(project_id);
    let data = serde_json::to_vec_pretty(store)?;
    atomic_write_bytes(&p, &data)
}

pub fn stage_label(stage: &str) -> &'static str {
    match stage {
        "pending_verify" => "待验证",
        "verified" => "已验证",
        "ready" => "可发布",
        "published" => "已上架",
        "archived" => "已归档",
        _ => "待验证",
    }
}

fn stage_rank(stage: &str) -> u8 {
    match stage {
        "pending_verify" => 0,
        "verified" => 1,
        "ready" => 2,
        "published" => 3,
        _ => 0,
    }
}

pub fn mark_artifacts_stage(
    project_id: &str,
    file_names: &[String],
    stage: &str,
    published_url: Option<&str>,
    published_provider: Option<&str>,
) -> Result<()> {
    let allow_downgrade = stage == "pending_verify" || stage == "archived";
    mark_artifacts_stage_with_dir(
        project_id,
        None,
        file_names,
        stage,
        published_url,
        published_provider,
        allow_downgrade,
    )
}

pub fn mark_artifacts_stage_with_dir(
    project_id: &str,
    release_dir: Option<&Path>,
    file_names: &[String],
    stage: &str,
    published_url: Option<&str>,
    published_provider: Option<&str>,
    allow_downgrade: bool,
) -> Result<()> {
    let mut store = load_store(project_id);
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let target_rank = stage_rank(stage);

    for name in file_names {
        let key = name.trim().to_string();
        if key.is_empty() {
            continue;
        }
        let rec = store.items.entry(key.clone()).or_default();

        if stage == "pending_verify" && allow_downgrade {
            rec.stage = "pending_verify".to_string();
            rec.sha256_short.clear();
            rec.published_url.clear();
            rec.published_provider.clear();
            rec.updated_at = now.clone();
            continue;
        }

        // Always recompute real SHA-256 digest when verifying/promoting an artifact strictly confined inside release_dir
        // so rebuilt same-version artifacts never keep a stale hash or stale `published` badge from an earlier build
        let mut content_changed = false;
        if let Some(dir) = release_dir {
            if stage == "verified" || stage == "ready" || stage == "published" {
                if let Ok(path) = crate::guardrails::resolve_confined_path(dir, &key) {
                    if path.is_file() {
                        let prev_sha = rec.sha256_short.clone();
                        if let Ok(full_hash) = sha256_file(&path) {
                            let new_short: String = full_hash.chars().take(12).collect();
                            if !prev_sha.is_empty() && !prev_sha.eq_ignore_ascii_case(&new_short) {
                                content_changed = true;
                            }
                            rec.sha256_short = new_short;
                        }
                        if let Ok(md) = fs::metadata(&path) {
                            rec.file_size = md.len();
                            rec.file_mtime_unix_ms = md
                                .modified()
                                .ok()
                                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_millis())
                                .unwrap_or(0);
                        }
                    }
                }
            }
        }

        if content_changed && stage != "published" {
            // Rebuilt file MUST NOT inherit an old release's `published` stage or URL
            rec.stage = stage.to_string();
            rec.published_url.clear();
            rec.published_provider.clear();
        } else {
            let current_rank = stage_rank(&rec.stage);
            let should_update_stage = allow_downgrade
                || rec.stage.is_empty()
                || stage == "archived"
                || target_rank >= current_rank;
            if should_update_stage {
                rec.stage = stage.to_string();
            }
        }
        rec.updated_at = now.clone();
        if let Some(u) = published_url {
            rec.published_url = u.to_string();
        }
        if let Some(p) = published_provider {
            rec.published_provider = p.to_string();
        }
    }
    save_store(project_id, &store)
}

fn filename_matches_exact_version(fname: &str, ver_clean: &str) -> bool {
    if ver_clean.is_empty() {
        return false;
    }
    let bytes = fname.as_bytes();
    let needle = ver_clean.as_bytes();
    let mut start = 0usize;
    while let Some(rel) = fname[start..].find(ver_clean) {
        let pos = start + rel;
        let end = pos + needle.len();
        let left_ok = pos == 0
            || matches!(bytes[pos - 1], b'_' | b'-' | b' ' | b'v' | b'V');
        let right_ok = if end == bytes.len() {
            true
        } else {
            match bytes[end] {
                b'_' | b' ' => true,
                // `.exe` / `.zip` allowed, but `.1` (4-part version `1.2.0.1`) rejected
                b'.' => !bytes.get(end + 1).is_some_and(|b| b.is_ascii_digit()),
                b'-' => {
                    if ver_clean.contains('-') {
                        true
                    } else {
                        let tail_low = fname[end + 1..].to_ascii_lowercase();
                        let first_tok = tail_low.split(['.', '_', '-']).next().unwrap_or("");
                        !matches!(
                            first_tok,
                            "alpha" | "beta" | "rc" | "pre" | "dev" | "nightly" | "canary" | "preview"
                        )
                    }
                }
                _ => false,
            }
        };
        if left_ok && right_ok {
            return true;
        }
        start = pos + 1;
    }
    false
}

/// Reset local artifact stage from `published` back to `ready` when an online Release is recalled/deleted.
pub fn reset_published_artifacts_for_tag(project_id: &str, tag_name: &str) -> Result<usize> {
    let mut store = load_store(project_id);
    let tag_clean = tag_name.trim();
    let ver_clean = tag_clean.trim_start_matches('v').trim_start_matches('V');
    if tag_clean.is_empty() {
        return Ok(0);
    }
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let mut matched_keys = Vec::new();
    for (fname, rec) in &store.items {
        if rec.stage != "published" {
            continue;
        }
        let url_matches = !rec.published_url.is_empty()
            && rec
                .published_url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .map(|seg| seg.eq_ignore_ascii_case(tag_clean))
                .unwrap_or(false);
        let name_matches = filename_matches_exact_version(fname, ver_clean);
        if url_matches || name_matches {
            matched_keys.push(fname.clone());
        }
    }
    // Also reset manifest/checksum if any release file for this tag matched
    if !matched_keys.is_empty() {
        for special in ["latest.json", "SHA256SUMS.txt"] {
            if let Some(rec) = store.items.get(special) {
                if rec.stage == "published" && !matched_keys.iter().any(|k| k == special) {
                    matched_keys.push(special.to_string());
                }
            }
        }
    }
    for key in &matched_keys {
        if let Some(rec) = store.items.get_mut(key) {
            rec.stage = "ready".to_string();
            rec.published_url.clear();
            rec.published_provider.clear();
            rec.updated_at = now.clone();
        }
    }
    if !matched_keys.is_empty() {
        save_store(project_id, &store)?;
    }
    Ok(matched_keys.len())
}

pub fn classify_artifacts(
    project_id: &str,
    release_dir: &Path,
    current_version: &str,
) -> Vec<ClassifiedArtifact> {
    let raw: Vec<ArtifactInfo> = scan_artifacts(release_dir);
    let store = load_store(project_id);

    let sums_path = release_dir.join("SHA256SUMS.txt");
    let sums_mtime_ms = file_mtime_unix_ms(&sums_path);
    let sums_map: HashMap<String, String> = fs::read_to_string(&sums_path)
        .ok()
        .map(|content| {
            content
                .lines()
                .filter_map(|line| {
                    let t = line.trim();
                    if t.is_empty() {
                        return None;
                    }
                    let mut it = t.splitn(2, |c: char| c.is_whitespace());
                    let hash = it.next()?.trim().to_string();
                    let file = it.next()?.trim().trim_start_matches('*').to_string();
                    if hash.is_empty() || file.is_empty() {
                        return None;
                    }
                    Some((file, hash))
                })
                .collect()
        })
        .unwrap_or_default();

    let latest_path = release_dir.join("latest.json");
    let latest_exists = latest_path.exists();
    let mut manifest_sha_map: HashMap<String, String> = HashMap::new();
    let mut latest_version_on_disk: Option<String> = None;
    if latest_exists {
        if let Ok(raw_latest) = fs::read_to_string(&latest_path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw_latest.trim_start_matches('\u{feff}')) {
                latest_version_on_disk = val
                    .get("version")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().trim_start_matches('v').trim_start_matches('V').to_string())
                    .filter(|s| !s.is_empty());
                let mut record_entry = |obj: &serde_json::Map<String, serde_json::Value>| {
                    if let (Some(f), Some(h)) = (
                        obj.get("file").and_then(|x| x.as_str()),
                        obj.get("sha256").and_then(|x| x.as_str()),
                    ) {
                        if h.len() >= 12 {
                            manifest_sha_map.insert(f.to_string(), h.to_string());
                        }
                    }
                };
                for k in ["installer", "portable"] {
                    if let Some(obj) = val.get(k).and_then(|x| x.as_object()) {
                        record_entry(obj);
                    }
                }
                if let Some(arr) = val.get("extras").and_then(|x| x.as_array()) {
                    for ev in arr {
                        if let Some(obj) = ev.as_object() {
                            record_entry(obj);
                        }
                    }
                }
            }
        }
    }

    raw.into_iter()
        .map(|art| {
            let meta = store.items.get(&art.name);
            let in_sums = sums_map.contains_key(&art.name);

            // Compute fast live short SHA-256 so rebuilt files & updated latest.json/SHA256SUMS.txt
            // NEVER keep stale "published" badges or obsolete publishedUrl links!
            // Uses millisecond-precision mtime + legacy fallback (binary newer than SHA256SUMS.txt)
            // so even same-byte-length rebuilds within the same minute are caught deterministically.
            let art_mtime_ms = file_mtime_unix_ms(Path::new(&art.path));
            let stat_drifted = meta
                .map(|m| {
                    let size_changed = m.file_size > 0 && art.size != m.file_size;
                    let mtime_changed =
                        m.file_mtime_unix_ms > 0 && art_mtime_ms != m.file_mtime_unix_ms;
                    let legacy_newer_than_sums = m.stage == "published"
                        && m.file_size == 0
                        && m.file_mtime_unix_ms == 0
                        && sums_mtime_ms > 0
                        && art_mtime_ms > sums_mtime_ms;
                    size_changed || mtime_changed || legacy_newer_than_sums
                })
                .unwrap_or(false);

            let live_sha_short: Option<String> = if art.kind == "manifest" || art.kind == "checksum" {
                let p = Path::new(&art.path);
                if p.is_file() && art.size <= 1024 * 1024 {
                    sha256_file(p).ok().map(|h| h.chars().take(12).collect())
                } else {
                    None
                }
            } else if stat_drifted {
                // On-disk size or mtime changed since last stage record: verify real file hash
                // directly so stale SHA256SUMS.txt cannot mask a rebuilt binary!
                let p = Path::new(&art.path);
                if p.is_file() {
                    sha256_file(p).ok().map(|h| h.chars().take(12).collect())
                } else {
                    None
                }
            } else {
                sums_map
                    .get(&art.name)
                    .or_else(|| manifest_sha_map.get(&art.name))
                    .map(|h| h.chars().take(12).collect::<String>())
            };

            let hash_drifted = match (meta, live_sha_short.as_deref()) {
                (Some(m), Some(live)) if !m.sha256_short.is_empty() => {
                    !live.eq_ignore_ascii_case(&m.sha256_short)
                }
                _ => false,
            };

            // Also invalidate stale published metadata on latest.json/SHA256SUMS.txt when their
            // recorded publishedUrl belongs to an older release tag (GitHub/Gitee/Gitea `/tag/` OR GitLab `/releases/`).
            let manifest_tag_drifted = if (art.kind == "manifest" || art.kind == "checksum") && !art.is_archived {
                meta.map(|m| {
                    if m.stage != "published" || m.published_url.trim().is_empty() {
                        return false;
                    }
                    let target_ver = latest_version_on_disk
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(current_version)
                        .trim()
                        .trim_start_matches('v')
                        .trim_start_matches('V');
                    if target_ver.is_empty() || target_ver == "0.0.0" {
                        return false;
                    }
                    let url_clean = m
                        .published_url
                        .trim()
                        .split(['?', '#'])
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let extracted_tag = if let Some(pos) = url_clean.rfind("/tag/") {
                        Some(&url_clean[pos + 5..])
                    } else if let Some(pos) = url_clean.rfind("/releases/") {
                        Some(&url_clean[pos + 10..])
                    } else {
                        None
                    };
                    extracted_tag
                        .map(|raw_t| {
                            let t = raw_t.trim_matches('/').trim_start_matches('v');
                            !t.is_empty()
                                && t != "download"
                                && t != target_ver.to_ascii_lowercase()
                        })
                        .unwrap_or(false)
                })
                .unwrap_or(false)
            } else {
                false
            };

            let meta_stale = hash_drifted || manifest_tag_drifted;

            let sha_short = live_sha_short
                .or_else(|| {
                    if meta_stale {
                        None
                    } else {
                        meta.map(|m| m.sha256_short.clone()).filter(|s| !s.is_empty())
                    }
                })
                .unwrap_or_default();

            let inferred = infer_default_stage(&art, current_version, in_sums, latest_exists);
            let stage = if art.is_archived {
                "archived".to_string()
            } else if meta_stale {
                inferred
            } else if let Some(m) = meta {
                if m.stage.is_empty() {
                    inferred
                } else if m.stage == "verified" && inferred == "ready" {
                    // Never keep a verified current-version artifact downgraded when latest.json + SHA256SUMS.txt exist
                    "ready".to_string()
                } else {
                    m.stage.clone()
                }
            } else {
                inferred
            };

            ClassifiedArtifact {
                stage_label: stage_label(&stage).to_string(),
                stage,
                sha256_short: sha_short,
                published_url: if meta_stale {
                    String::new()
                } else {
                    meta.map(|m| m.published_url.clone()).unwrap_or_default()
                },
                published_provider: if meta_stale {
                    String::new()
                } else {
                    meta.map(|m| m.published_provider.clone()).unwrap_or_default()
                },
                updated_at: if meta_stale {
                    art.modified.clone()
                } else {
                    meta.map(|m| m.updated_at.clone())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| art.modified.clone())
                },
                artifact: art,
            }
        })
        .collect()
}

fn infer_default_stage(
    art: &ArtifactInfo,
    current_version: &str,
    has_checksum: bool,
    latest_exists: bool,
) -> String {
    if art.kind == "manifest" || art.kind == "checksum" {
        return "ready".to_string();
    }
    let matches_ver =
        !current_version.is_empty() && art.version.as_deref() == Some(current_version);
    if has_checksum && latest_exists && matches_ver {
        "ready".to_string()
    } else if has_checksum {
        "verified".to_string()
    } else {
        "pending_verify".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mark_verified_computes_sha256_and_never_downgrades_ready_or_published() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_art_stage_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(7)
        ));
        fs::create_dir_all(&base).unwrap();
        let pid = format!(
            "test-art-{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(11)
        );
        let cur_zip = base.join("Demo_1.2.0_x64-portable.zip");
        let old_zip = base.join("Demo_1.1.0_x64-portable.zip");
        fs::write(&cur_zip, b"current binary content").unwrap();
        fs::write(&old_zip, b"older binary content").unwrap();
        let cur_hash = sha256_file(&cur_zip).unwrap();
        fs::write(
            base.join("SHA256SUMS.txt"),
            format!("{}  Demo_1.2.0_x64-portable.zip\n", cur_hash),
        )
        .unwrap();
        fs::write(base.join("latest.json"), r#"{"version":"1.2.0"}"#).unwrap();

        // 1. Initial classification: 1.2.0 is ready, 1.1.0 is pending_verify
        let list0 = classify_artifacts(&pid, &base, "1.2.0");
        let cur0 = list0.iter().find(|c| c.artifact.name == "Demo_1.2.0_x64-portable.zip").unwrap();
        let old0 = list0.iter().find(|c| c.artifact.name == "Demo_1.1.0_x64-portable.zip").unwrap();
        assert_eq!(cur0.stage, "ready");
        assert_eq!(old0.stage, "pending_verify");
        assert!(old0.sha256_short.is_empty());

        // 2. Re-verifying 1.2.0 MUST NOT downgrade ready -> verified; marking 1.1.0 verified computes sha256_short
        mark_artifacts_stage_with_dir(
            &pid,
            Some(&base),
            &[
                "Demo_1.2.0_x64-portable.zip".into(),
                "Demo_1.1.0_x64-portable.zip".into(),
            ],
            "verified",
            None,
            None,
            false,
        )
        .unwrap();
        let list1 = classify_artifacts(&pid, &base, "1.2.0");
        let cur1 = list1.iter().find(|c| c.artifact.name == "Demo_1.2.0_x64-portable.zip").unwrap();
        let old1 = list1.iter().find(|c| c.artifact.name == "Demo_1.1.0_x64-portable.zip").unwrap();
        assert_eq!(cur1.stage, "ready", "1.2.0 must stay ready after SHA-256 verification");
        assert_eq!(old1.stage, "verified");
        assert_eq!(old1.sha256_short.len(), 12);

        // 3. Mark published, then verify again -> must stay published; recall -> resets back to ready
        mark_artifacts_stage_with_dir(
            &pid,
            Some(&base),
            &["Demo_1.2.0_x64-portable.zip".into(), "latest.json".into()],
            "published",
            Some("https://github.com/acme/demo/releases/tag/v1.2.0"),
            Some("github"),
            false,
        )
        .unwrap();
        mark_artifacts_stage_with_dir(
            &pid,
            Some(&base),
            &["Demo_1.2.0_x64-portable.zip".into()],
            "verified",
            None,
            None,
            false,
        )
        .unwrap();
        let list2 = classify_artifacts(&pid, &base, "1.2.0");
        assert_eq!(
            list2.iter().find(|c| c.artifact.name == "Demo_1.2.0_x64-portable.zip").unwrap().stage,
            "published"
        );

        let reset = reset_published_artifacts_for_tag(&pid, "v1.2.0").unwrap();
        assert_eq!(reset, 2);
        let list3 = classify_artifacts(&pid, &base, "1.2.0");
        let cur3 = list3.iter().find(|c| c.artifact.name == "Demo_1.2.0_x64-portable.zip").unwrap();
        assert_eq!(cur3.stage, "ready");
        assert!(cur3.published_url.is_empty());

        let _ = fs::remove_dir_all(storage::workspaces_dir().join(&pid));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_redteam_rebuilt_hash_refresh_and_exact_version_recall() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_art_rt_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(93)
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let pid = format!("rt_art_{}", chrono::Local::now().timestamp_millis());

        let f_stable = base.join("Demo_1.2.0_x64-setup.exe");
        let f_beta = base.join("Demo_1.2.0-beta.1_x64-setup.exe");
        let f_four = base.join("Demo_1.2.0.1_x64-setup.exe");
        fs::write(&f_stable, b"build-v1").unwrap();
        fs::write(&f_beta, b"beta-v1").unwrap();
        fs::write(&f_four, b"four-part").unwrap();

        mark_artifacts_stage_with_dir(
            &pid,
            Some(&base),
            &[
                "Demo_1.2.0_x64-setup.exe".into(),
                "Demo_1.2.0-beta.1_x64-setup.exe".into(),
                "Demo_1.2.0.1_x64-setup.exe".into(),
            ],
            "published",
            None,
            Some("github"),
            false,
        )
        .unwrap();
        let h1 = classify_artifacts(&pid, &base, "1.2.0")
            .into_iter()
            .find(|c| c.artifact.name == "Demo_1.2.0_x64-setup.exe")
            .unwrap()
            .sha256_short;

        // Rebuild same filename with different bytes -> re-marking MUST refresh sha256_short!
        fs::write(&f_stable, b"build-v2-rebuilt-content").unwrap();
        mark_artifacts_stage_with_dir(
            &pid,
            Some(&base),
            &["Demo_1.2.0_x64-setup.exe".into()],
            "verified",
            None,
            None,
            false,
        )
        .unwrap();
        let rebuilt_item = classify_artifacts(&pid, &base, "1.2.0")
            .into_iter()
            .find(|c| c.artifact.name == "Demo_1.2.0_x64-setup.exe")
            .unwrap();
        assert_ne!(h1, rebuilt_item.sha256_short, "rebuilt artifact must refresh sha256_short");
        assert_eq!(rebuilt_item.stage, "verified", "rebuilt artifact must demote stale published stage");
        assert!(rebuilt_item.published_url.is_empty(), "rebuilt artifact must clear stale published_url");

        // Re-publish rebuilt v1.2.0 so recall test verifies exact-version matching
        mark_artifacts_stage_with_dir(
            &pid,
            Some(&base),
            &["Demo_1.2.0_x64-setup.exe".into()],
            "published",
            Some("https://github.com/o/r/releases/tag/v1.2.0"),
            Some("github"),
            false,
        )
        .unwrap();

        // Recall v1.2.0 must ONLY reset Demo_1.2.0_x64-setup.exe, NOT -beta.1 or 1.2.0.1!
        let reset = reset_published_artifacts_for_tag(&pid, "v1.2.0").unwrap();
        assert_eq!(reset, 1);
        let after = classify_artifacts(&pid, &base, "1.2.0");
        assert_eq!(
            after.iter().find(|c| c.artifact.name == "Demo_1.2.0_x64-setup.exe").unwrap().stage,
            "ready"
        );
        assert_eq!(
            after.iter().find(|c| c.artifact.name == "Demo_1.2.0-beta.1_x64-setup.exe").unwrap().stage,
            "published"
        );
        assert_eq!(
            after.iter().find(|c| c.artifact.name == "Demo_1.2.0.1_x64-setup.exe").unwrap().stage,
            "published"
        );

        let _ = fs::remove_dir_all(storage::workspaces_dir().join(&pid));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_stale_published_manifest_and_rebuilt_binary_auto_demote_in_classify() {
        let ts = chrono::Local::now().timestamp_nanos_opt().unwrap_or(55);
        let pid = format!("test-stale-pub-{}", ts);
        let base = std::env::temp_dir().join(format!("shipforge_stale_pub_{}", ts));
        fs::create_dir_all(&base).unwrap();

        let bin = base.join("Demo_1.0.0_x64-setup.exe");
        let latest = base.join("latest.json");
        let sums = base.join("SHA256SUMS.txt");
        fs::write(&bin, b"v1-published-binary").unwrap();
        let sha1 = sha256_file(&bin).unwrap();
        fs::write(&sums, format!("{}  Demo_1.0.0_x64-setup.exe\n", sha1)).unwrap();
        fs::write(&latest, r#"{"version":"1.0.0"}"#).unwrap();

        // Mark all published on GitLab (URL uses /-/releases/v1.0.0, not /tag/)
        mark_artifacts_stage_with_dir(
            &pid,
            Some(&base),
            &[
                "Demo_1.0.0_x64-setup.exe".into(),
                "latest.json".into(),
                "SHA256SUMS.txt".into(),
            ],
            "published",
            Some("https://gitlab.com/org/repo/-/releases/v1.0.0"),
            Some("gitlab"),
            false,
        )
        .unwrap();

        // Rewrite latest.json for v1.0.1, rewrite SHA256SUMS, AND rebuild binary WITHOUT updating SHA256SUMS!
        fs::write(&latest, r#"{"version":"1.0.1"}"#).unwrap();
        fs::write(&bin, b"v1-rebuilt-longer-binary-content").unwrap();

        let items = classify_artifacts(&pid, &base, "1.0.1");
        let lat_item = items.iter().find(|c| c.artifact.name == "latest.json").unwrap();
        assert_eq!(lat_item.stage, "ready", "updated latest.json must demote from stale published");
        assert!(lat_item.published_url.is_empty(), "stale published_url must be cleared");

        let bin_item = items.iter().find(|c| c.artifact.name == "Demo_1.0.0_x64-setup.exe").unwrap();
        assert_ne!(bin_item.stage, "published", "rebuilt binary with changed size/hash must auto-demote even if SHA256SUMS.txt is stale");
        assert!(bin_item.published_url.is_empty());

        let _ = fs::remove_dir_all(storage::workspaces_dir().join(&pid));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_round2_same_size_same_minute_rebuild_and_legacy_meta_demote() {
        let ts = chrono::Local::now().timestamp_nanos_opt().unwrap_or(77);
        let pid = format!("test-r2-stale-{}", ts);
        let base = std::env::temp_dir().join(format!("shipforge_r2_stale_{}", ts));
        fs::create_dir_all(&base).unwrap();

        let bin = base.join("SameSize_2.0.0_x64-setup.exe");
        fs::write(&bin, b"AAAAAAAA11111111").unwrap(); // 16 bytes
        let sha1 = sha256_file(&bin).unwrap();
        fs::write(
            base.join("SHA256SUMS.txt"),
            format!("{}  SameSize_2.0.0_x64-setup.exe\n", sha1),
        )
        .unwrap();

        mark_artifacts_stage_with_dir(
            &pid,
            Some(&base),
            &["SameSize_2.0.0_x64-setup.exe".into()],
            "published",
            Some("https://github.com/o/r/releases/tag/v2.0.0"),
            Some("github"),
            false,
        )
        .unwrap();

        // Sleep 25ms so NTFS mtime_unix_ms advances even within the exact same wall-clock minute,
        // then overwrite with DIFFERENT content of the EXACT SAME 16-byte length!
        std::thread::sleep(std::time::Duration::from_millis(25));
        fs::write(&bin, b"BBBBBBBB22222222").unwrap(); // exact same 16 bytes!

        let items = classify_artifacts(&pid, &base, "2.0.0");
        let item = items
            .iter()
            .find(|c| c.artifact.name == "SameSize_2.0.0_x64-setup.exe")
            .unwrap();
        assert_ne!(
            item.stage, "published",
            "same-size same-minute rebuild must still auto-demote via ms-precision mtime"
        );
        assert!(item.published_url.is_empty());

        // Legacy metadata record (fileSize=0, fileMtimeUnixMs=0) with binary newer than SHA256SUMS.txt
        let mut store = load_store(&pid);
        if let Some(rec) = store.items.get_mut("SameSize_2.0.0_x64-setup.exe") {
            rec.stage = "published".into();
            rec.sha256_short = sha1.chars().take(12).collect();
            rec.published_url = "https://github.com/o/r/releases/tag/v2.0.0".into();
            rec.file_size = 0;
            rec.file_mtime_unix_ms = 0;
        }
        save_store(&pid, &store).unwrap();

        let items_legacy = classify_artifacts(&pid, &base, "2.0.0");
        let item_legacy = items_legacy
            .iter()
            .find(|c| c.artifact.name == "SameSize_2.0.0_x64-setup.exe")
            .unwrap();
        assert_ne!(
            item_legacy.stage, "published",
            "legacy metadata must still auto-demote when binary is newer than SHA256SUMS.txt"
        );

        let _ = fs::remove_dir_all(storage::workspaces_dir().join(&pid));
        let _ = fs::remove_dir_all(&base);
    }
}
