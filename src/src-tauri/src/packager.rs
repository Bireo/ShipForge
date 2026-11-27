use crate::models::{
    ArtifactInfo, ChecksumVerifyItem, ManifestRequest, ManifestResult, PackageRequest,
    PackageResult,
};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local};
use flate2::write::GzEncoder;
use flate2::Compression;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const IO_BUF_SIZE: usize = 256 * 1024;

fn artifact_ver_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Match X.Y.Z plus optional genuine SemVer prerelease (never greedily swallowing `-x64`, `-win64`, `-setup.exe`, `.zip`!)
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(\d+\.\d+\.\d+(?:-(?:alpha|beta|rc|pre|dev|nightly|canary|preview|snapshot)(?:[.-][0-9a-z]+)*)?)",
        )
        .unwrap()
    })
}

/// Extracts the clean SemVer embedded in a release artifact filename (e.g.
/// `MyApp-2.0.0-win64.zip` -> `2.0.0`, `QDisk_1.46.7-beta.1_x64-setup.exe` -> `1.46.7-beta.1`,
/// `MyApp-1.2.3-beta.1.zip.part001` -> `1.2.3-beta.1`).
pub fn extract_artifact_version(file_name: &str) -> Option<String> {
    let mut stem = file_name.trim().to_string();
    // Strip trailing split-volume extensions (`.part001`, `.001`) and archive/installer extensions iteratively
    loop {
        let low = stem.to_ascii_lowercase();
        let mut stripped = false;
        if let Some((prefix, tail)) = low.rsplit_once('.') {
            let is_part = tail.starts_with("part")
                && tail.len() > 4
                && tail[4..].chars().all(|c| c.is_ascii_digit());
            let is_num_vol = tail.len() == 3 && tail.chars().all(|c| c.is_ascii_digit());
            if is_part || is_num_vol {
                stem.truncate(prefix.len());
                stripped = true;
            }
        }
        if !stripped {
            for ext in [
                ".tar.gz", ".tar.xz", ".tar.bz2", ".exe", ".msi", ".zip", ".7z", ".dmg", ".deb",
                ".rpm", ".appimage", ".sha256", ".txt", ".json",
            ] {
                if low.ends_with(ext) {
                    let keep = stem.len() - ext.len();
                    stem.truncate(keep);
                    stripped = true;
                    break;
                }
            }
        }
        if !stripped {
            break;
        }
    }
    let caps = artifact_ver_re().captures(&stem)?;
    let mut ver = caps.get(1)?.as_str().to_string();
    // Trim trailing packaging/arch tags if they were hyphen-chained to a prerelease (e.g. `1.0.0-beta.1-x64-setup`)
    let arch_tags = [
        "-setup", "-portable", "-x64", "-x86_64", "-arm64", "-win64", "-win32", "-windows",
        "-linux", "-macos", "-split", "-installer",
    ];
    loop {
        let low = ver.to_ascii_lowercase();
        let mut trimmed = false;
        for tag in arch_tags {
            if low.ends_with(tag) {
                ver.truncate(ver.len() - tag.len());
                trimmed = true;
                break;
            }
        }
        if !trimmed {
            break;
        }
    }
    let ver = ver.trim_end_matches(['.', '-']).to_string();
    if ver.is_empty() { None } else { Some(ver) }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f =
        File::open(path).with_context(|| format!("无法打开文件计算哈希：{}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; IO_BUF_SIZE];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{:02x}", b)).collect())
}

/// Computes SHA-256 hashes for multiple files in parallel (bounded to max 4 concurrent disk readers),
/// preserving input order while saturating multi-core SHA-NI / NVMe throughput.
pub fn sha256_files_parallel(paths: &[PathBuf]) -> Vec<Result<String>> {
    if paths.len() <= 1 {
        return paths.iter().map(|p| sha256_file(p)).collect();
    }
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Result<String>>>> =
        Mutex::new((0..paths.len()).map(|_| None).collect());
    let workers = paths.len().min(4);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let idx = next.fetch_add(1, Ordering::Relaxed);
                if idx >= paths.len() {
                    break;
                }
                let r = sha256_file(&paths[idx]);
                if let Ok(mut guard) = results.lock() {
                    guard[idx] = Some(r);
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .map(|slot| slot.unwrap_or_else(|| Err(anyhow!("并行哈希工作线程异常中止"))))
        .collect()
}

struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    fn finish_hex(mut self) -> io::Result<String> {
        self.inner.flush()?;
        let digest = self.hasher.finalize();
        Ok(digest.iter().map(|b| format!("{:02x}", b)).collect())
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Files already in compressed formats waste CPU under Deflate without shrinking; store them directly in ZIP.
fn is_already_compressed_ext(arc_name: &str) -> bool {
    let ext = Path::new(arc_name)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "zip"
            | "7z"
            | "rar"
            | "gz"
            | "tgz"
            | "xz"
            | "bz2"
            | "zst"
            | "br"
            | "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "gif"
            | "avif"
            | "mp4"
            | "mkv"
            | "webm"
            | "mp3"
            | "aac"
            | "flac"
            | "woff2"
            | "pdf"
    )
}

fn copy_with_buf<R: Read, W: Write>(reader: &mut R, writer: &mut W, buf: &mut [u8]) -> io::Result<u64> {
    let mut total = 0u64;
    loop {
        let n = reader.read(buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        total += n as u64;
    }
    Ok(total)
}

fn classify_artifact(name: &str) -> (&'static str, Option<String>) {
    let lower = name.to_ascii_lowercase();
    let version = extract_artifact_version(name);

    let is_split_part = lower
        .rsplit_once(".part")
        .map(|(_, digits)| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false);

    let kind = if lower == "latest.json" || lower.ends_with("manifest.json") {
        "manifest"
    } else if lower.contains("sha256") || lower.ends_with(".sha256") {
        "checksum"
    } else if lower.ends_with("-setup.exe")
        || lower.ends_with(".msi")
        || lower.ends_with(".msix")
        || lower.ends_with(".appx")
        || lower.ends_with(".dmg")
        || lower.ends_with(".pkg")
        || lower.ends_with(".deb")
        || lower.ends_with(".rpm")
        || lower.ends_with(".appimage")
        || lower.ends_with(".apk")
        || lower.ends_with(".aab")
        || lower.ends_with(".exe")
    {
        "installer"
    } else if is_split_part
        || lower.ends_with(".zip")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || lower.ends_with(".tar.xz")
        || lower.ends_with(".tar.zst")
        || lower.ends_with(".7z")
        || lower.ends_with(".jar")
        || lower.ends_with(".whl")
        || lower.ends_with(".nupkg")
        || lower.ends_with(".vsix")
    {
        "portable"
    } else {
        "other"
    };
    (kind, version)
}

pub fn scan_artifacts(release_dir: &Path) -> Vec<ArtifactInfo> {
    if !release_dir.exists() {
        return vec![];
    }
    let mut out = Vec::new();
    collect_dir_artifacts(release_dir, false, &mut out);
    let archive_dir = release_dir.join("archive");
    if archive_dir.is_dir() {
        collect_dir_artifacts(&archive_dir, true, &mut out);
    }
    out.sort_by(|a, b| {
        a.is_archived
            .cmp(&b.is_archived)
            .then_with(|| b.modified.cmp(&a.modified))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn collect_dir_artifacts(dir: &Path, is_archived: bool, out: &mut Vec<ArtifactInfo>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let lower_name = name.to_ascii_lowercase();
        if lower_name.ends_with(".shipforge-tmp")
            || lower_name.ends_with(".tmp")
            || lower_name.ends_with(".crdownload")
            || lower_name.ends_with(".part.tmp")
        {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified = meta
            .modified()
            .ok()
            .map(|t| {
                let dt: DateTime<Local> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_default();
        let (kind, version) = classify_artifact(&name);
        out.push(ArtifactInfo {
            name,
            path: path.to_string_lossy().to_string(),
            size: meta.len(),
            modified,
            kind: kind.into(),
            version,
            is_archived,
        });
    }
}

/// Case-insensitive wildcard matcher supporting `*` and `?`
fn wildcard_match_ci(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (None, 0usize);

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_p = Some(pi);
            pi += 1;
            star_t = ti;
        } else if let Some(sp) = star_p {
            pi = sp + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub fn matches_exclude(rel: &str, name: &str, excludes: &[String]) -> bool {
    let rel_norm = rel.replace('\\', "/");
    for pat in excludes {
        let p = pat.trim().replace('\\', "/");
        if p.is_empty() {
            continue;
        }
        if p.contains('*') || p.contains('?') {
            if wildcard_match_ci(&p, name) || wildcard_match_ci(&p, &rel_norm) {
                return true;
            }
        } else {
            let p_low = p.to_lowercase();
            let n_low = name.to_lowercase();
            let r_low = rel_norm.to_lowercase();
            if n_low == p_low
                || r_low == p_low
                || r_low.ends_with(&format!("/{}", p_low))
                || r_low.contains(&format!("/{}/", p_low))
                || r_low.starts_with(&format!("{}/", p_low))
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
pub fn pack_portable(root: &Path, release_dir: &Path, req: &PackageRequest) -> Result<PackageResult> {
    pack_portable_with_progress(root, release_dir, req, |_, _| {})
}

pub fn pack_portable_with_progress<F>(
    root: &Path,
    release_dir: &Path,
    req: &PackageRequest,
    mut on_progress: F,
) -> Result<PackageResult>
where
    F: FnMut(u8, &str),
{
    let start = Instant::now();
    on_progress(4, "扫描待打包文件…");
    if req.sources.is_empty() {
        return Err(anyhow!("请至少选择一个要打包的文件或目录"));
    }
    let mut clean_name = crate::guardrails::validate_single_filename(req.output_name.trim())
        .map_err(|_| anyhow!("输出文件名无效或包含路径分隔符：{}", req.output_name.trim()))?;
    let bad_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*', '%', '&', '^', ';', '`', '!'];
    if clean_name.is_empty()
        || clean_name == "."
        || clean_name == ".."
        || clean_name.chars().any(|c| c.is_control() || bad_chars.contains(&c))
    {
        return Err(anyhow!("输出文件名包含非法或脚本保留字符：{}", clean_name));
    }
    if req.format.eq_ignore_ascii_case("targz") {
        if !clean_name.to_ascii_lowercase().ends_with(".tar.gz") {
            clean_name.push_str(".tar.gz");
        }
    } else if !clean_name.to_ascii_lowercase().ends_with(".zip") {
        clean_name.push_str(".zip");
    }

    fs::create_dir_all(release_dir)?;
    let out_path = release_dir.join(&clean_name);
    let tmp_name = format!(
        "{}.{}.shipforge-tmp",
        clean_name,
        chrono::Local::now().timestamp_millis()
    );
    let tmp_path = release_dir.join(&tmp_name);

    let canonical_out = fs::canonicalize(&out_path).ok();
    let canonical_rel = fs::canonicalize(release_dir).ok();

    let mut seen_arc: HashSet<String> = HashSet::new();
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    for src in &req.sources {
        let trimmed = src.trim();
        if trimmed.is_empty() {
            continue;
        }
        let full = crate::guardrails::resolve_confined_path(root, trimmed)?;
        if !full.exists() {
            return Err(anyhow!("待打包路径不存在：{}", trimmed));
        }
        let base_name = full
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();

        if full.is_file() {
            if is_self_output(&full, &out_path, canonical_out.as_deref()) {
                continue;
            }
            if !matches_exclude(&base_name, &base_name, &req.excludes)
                && seen_arc.insert(base_name.to_lowercase())
            {
                files.push((full, base_name));
            }
        } else {
            let canonical_root = fs::canonicalize(root).ok();
            for entry in WalkDir::new(&full)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| !crate::guardrails::is_reparse_point(e.path()))
                .flatten()
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let ep = entry.path();
                if let (Some(ref croot), Ok(cep)) = (&canonical_root, fs::canonicalize(ep)) {
                    if !cep.starts_with(croot) {
                        continue;
                    }
                }
                let fname = entry.file_name().to_string_lossy();
                if fname.ends_with(".shipforge-tmp")
                    || is_self_output(ep, &out_path, canonical_out.as_deref())
                {
                    continue;
                }
                if let (Some(ref crel), Ok(cep)) = (&canonical_rel, fs::canonicalize(ep)) {
                    if cep.starts_with(crel) && fname.eq_ignore_ascii_case(&clean_name) {
                        continue;
                    }
                }
                let rel_inside = ep
                    .strip_prefix(&full)
                    .unwrap_or(ep)
                    .to_string_lossy()
                    .replace('\\', "/");
                let arc_path = format!("{}/{}", base_name, rel_inside);
                if matches_exclude(&arc_path, &fname, &req.excludes) {
                    continue;
                }
                if seen_arc.insert(arc_path.to_lowercase()) {
                    files.push((ep.to_path_buf(), arc_path));
                }
            }
        }
    }

    let inject_readme = match req.readme_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => {
            let safe_readme = crate::guardrails::validate_single_filename(name)
                .map_err(|_| anyhow!("附带说明文件名非法（禁止包含路径或 ..）：{}", name))?;
            let body = req.readme_content.as_deref().unwrap_or("").trim();
            if body.is_empty() || seen_arc.contains(&safe_readme.to_lowercase()) {
                None
            } else {
                Some((safe_readme, body.to_string()))
            }
        }
        None => None,
    };

    if !req.allow_sensitive {
        let mut blocked: Vec<String> = Vec::new();
        for (_, arc) in &files {
            if let Some(reason) = crate::guardrails::detect_sensitive_reason(arc) {
                blocked.push(format!("{} ({})", arc, reason));
                if blocked.len() >= 5 {
                    break;
                }
            }
        }
        if !blocked.is_empty() {
            return Err(anyhow!(
                "安全拦截：待打包内容包含敏感文件，已阻止写入公开压缩包：{}。请将其加入排除规则，或确认无风险后勾选允许包含敏感文件。",
                blocked.join("；")
            ));
        }
    }

    if files.is_empty() && inject_readme.is_none() {
        return Err(anyhow!("过滤后没有可写入压缩包的文件"));
    }

    let total_files = files.len().max(1);
    let mut raw_bytes: u64 = 0;
    let mut file_count: usize = 0;

    let write_res: Result<Option<String>> = (|| {
        let mut copy_buf = vec![0u8; IO_BUF_SIZE];
        if req.format.eq_ignore_ascii_case("targz") {
            let f = File::create(&tmp_path)?;
            let bw = BufWriter::with_capacity(IO_BUF_SIZE, HashingWriter::new(f));
            let enc = GzEncoder::new(bw, Compression::new(5));
            let mut tar = tar::Builder::new(enc);
            for (idx, (src, arc_name)) in files.iter().enumerate() {
                let meta = fs::metadata(src)?;
                raw_bytes += meta.len();
                file_count += 1;
                tar.append_path_with_name(src, arc_name)?;
                if idx % 8 == 0 || idx + 1 == total_files {
                    let pct = 10 + (((idx + 1) * 78) / total_files) as u8;
                    on_progress(pct.min(88), &format!("压缩 {}/{}：{}", idx + 1, total_files, arc_name));
                }
            }
            if let Some((ref rname, ref rbody)) = inject_readme {
                let bytes = rbody.as_bytes();
                let mut hdr = tar::Header::new_gnu();
                hdr.set_size(bytes.len() as u64);
                hdr.set_mode(0o644);
                hdr.set_cksum();
                tar.append_data(&mut hdr, rname, bytes)?;
                raw_bytes += bytes.len() as u64;
                file_count += 1;
            }
            on_progress(90, "封装 GZip 归档…");
            let enc = tar.into_inner()?;
            let bw = enc.finish()?;
            let hw = bw.into_inner().map_err(|e| e.into_error())?;
            let hex = hw.finish_hex()?;
            Ok(Some(hex))
        } else {
            let f = File::create(&tmp_path)?;
            let bw = BufWriter::with_capacity(IO_BUF_SIZE, f);
            let mut zip = ZipWriter::new(bw);
            let opts_deflate = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .compression_level(Some(5));
            let opts_stored = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (idx, (src, arc_name)) in files.iter().enumerate() {
                let opts = if is_already_compressed_ext(arc_name) {
                    opts_stored
                } else {
                    opts_deflate
                };
                zip.start_file(arc_name, opts)?;
                let mut sf = File::open(src)?;
                let copied = copy_with_buf(&mut sf, &mut zip, &mut copy_buf)?;
                raw_bytes += copied;
                file_count += 1;
                if idx % 8 == 0 || idx + 1 == total_files {
                    let pct = 10 + (((idx + 1) * 78) / total_files) as u8;
                    on_progress(pct.min(88), &format!("压缩 {}/{}：{}", idx + 1, total_files, arc_name));
                }
            }
            if let Some((ref rname, ref rbody)) = inject_readme {
                zip.start_file(rname, opts_deflate)?;
                zip.write_all(rbody.as_bytes())?;
                raw_bytes += rbody.len() as u64;
                file_count += 1;
            }
            on_progress(90, "写入 ZIP 索引目录…");
            let mut bw = zip.finish()?;
            bw.flush()?;
            Ok(None)
        }
    })();

    let inline_sha = match write_res {
        Ok(s) => s,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }
    };

    on_progress(94, "计算 SHA-256 并落盘…");
    let _ = fs::remove_file(&out_path);
    fs::rename(&tmp_path, &out_path)?;

    let compressed_bytes = fs::metadata(&out_path)?.len();
    let sha256 = match inline_sha {
        Some(h) => h,
        None => sha256_file(&out_path)?,
    };

    let mut split_parts: Vec<String> = Vec::new();
    let mut split_manifest_path = String::new();
    if let Some(limit_mb) = req.split_size_mb {
        if limit_mb > 0 {
            let chunk_bytes = limit_mb.saturating_mul(1024 * 1024);
            if compressed_bytes > chunk_bytes {
                on_progress(96, &format!("按 {} MB 切分分卷包…", limit_mb));
                let (parts, man_path) =
                    split_archive_into_volumes(&out_path, release_dir, &clean_name, chunk_bytes, &sha256)?;
                split_parts = parts;
                split_manifest_path = man_path.to_string_lossy().to_string();
            }
        }
    }

    on_progress(100, &format!("打包完成：{}", clean_name));

    Ok(PackageResult {
        output_path: out_path.to_string_lossy().to_string(),
        file_name: clean_name,
        file_count,
        raw_bytes,
        compressed_bytes,
        sha256,
        duration_ms: start.elapsed().as_millis() as u64,
        split_parts,
        split_manifest_path,
    })
}

/// Stream-split an archive file into fixed-size `.001`, `.002`, ... volumes with a JSON checksum manifest & merge script.
pub fn split_archive_into_volumes(
    archive_path: &Path,
    release_dir: &Path,
    base_name: &str,
    chunk_bytes: u64,
    full_sha256: &str,
) -> Result<(Vec<String>, PathBuf)> {
    if chunk_bytes == 0 {
        return Err(anyhow!("分卷大小必须大于 0"));
    }
    crate::guardrails::validate_single_filename(base_name)?;
    // Purge stale old split parts `<base_name>.NNN` from previous larger builds
    let part_prefix = format!("{}.", base_name);
    if let Ok(rd) = fs::read_dir(release_dir) {
        for entry in rd.flatten() {
            let fname = entry.file_name().to_string_lossy().to_string();
            if let Some(suffix) = fname.strip_prefix(&part_prefix) {
                if suffix.len() == 3 && suffix.chars().all(|c| c.is_ascii_digit()) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
    let mut reader = File::open(archive_path)?;
    let total_size = reader.metadata()?.len();
    if total_size == 0 {
        return Err(anyhow!("待分卷文件为空（0 字节），无法执行分卷切分"));
    }
    let expected_parts = total_size.div_ceil(chunk_bytes);
    if expected_parts > 999 {
        return Err(anyhow!(
            "当前分卷大小将产生 {} 个分卷（超过上限 999），请调大单卷体积",
            expected_parts
        ));
    }
    let mut buf = vec![0u8; crate::guardrails::STREAM_BUF_BYTES];
    let mut part_idx: usize = 1;
    let mut parts_meta = Vec::new();
    let mut part_names = Vec::new();

    let mut remaining_total = total_size;
    while remaining_total > 0 {
        let part_name = format!("{}.{:03}", base_name, part_idx);
        let part_path = release_dir.join(&part_name);
        let tmp_part = release_dir.join(format!(".{}.sf-part-tmp", part_name));
        let target_len = remaining_total.min(chunk_bytes);
        let mut written_this_part: u64 = 0;

        let part_sha = {
            let f = File::create(&tmp_part)?;
            let mut hw = HashingWriter::new(BufWriter::with_capacity(256 * 1024, f));
            while written_this_part < target_len {
                let to_read = ((target_len - written_this_part) as usize).min(buf.len());
                let n = reader.read(&mut buf[..to_read])?;
                if n == 0 {
                    break;
                }
                hw.write_all(&buf[..n])?;
                written_this_part += n as u64;
            }
            hw.finish_hex()?
        };

        crate::guardrails::retry_on_win_lock(|| fs::rename(&tmp_part, &part_path))?;
        remaining_total = remaining_total.saturating_sub(written_this_part);
        parts_meta.push(serde_json::json!({
            "index": part_idx,
            "file": part_name,
            "size": written_this_part,
            "sha256": part_sha,
        }));
        part_names.push(part_name);
        part_idx += 1;
    }

    let manifest_name = format!("{}.split.json", base_name);
    let manifest_path = release_dir.join(&manifest_name);
    let manifest_json = serde_json::json!({
        "file": base_name,
        "totalBytes": total_size,
        "sha256": full_sha256,
        "chunkBytes": chunk_bytes,
        "parts": parts_meta,
    });
    crate::guardrails::atomic_write_bytes(
        &manifest_path,
        serde_json::to_string_pretty(&manifest_json)?.as_bytes(),
    )?;

    // Generate double-clickable Windows reassembly helper script
    let merge_expr = part_names
        .iter()
        .map(|p| format!("\"{}\"", p))
        .collect::<Vec<_>>()
        .join("+");
    let cmd_script = format!(
        "@echo off\r\nchcp 65001 >nul\r\necho 正在合并还原 {} ...\r\ncopy /b {} \"{}\"\r\nif errorlevel 1 (\r\n  echo 合并失败，请确认所有分卷文件在同一目录。\r\n  pause\r\n  exit /b 1\r\n)\r\necho 合并完成！完整文件 SHA-256 应为：{}\r\npause\r\n",
        base_name, merge_expr, base_name, full_sha256
    );
    let merge_script_path = release_dir.join(format!("合并还原_{}.cmd", base_name));
    let _ = crate::guardrails::atomic_write_bytes(&merge_script_path, cmd_script.as_bytes());

    Ok((part_names, manifest_path))
}

fn is_self_output(candidate: &Path, out_path: &Path, canonical_out: Option<&Path>) -> bool {
    if candidate == out_path {
        return true;
    }
    if let (Some(co), Ok(cc)) = (canonical_out, fs::canonicalize(candidate)) {
        if cc == co {
            return true;
        }
    }
    false
}

pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

pub fn generate_manifest(release_dir: &Path, req: &ManifestRequest) -> Result<ManifestResult> {
    if !release_dir.exists() {
        return Err(anyhow!("找不到发布目录：{}", release_dir.display()));
    }
    crate::version::validate_semver(req.version.trim())?;

    let base_url = match req.url_mode.as_str() {
        "local" => format!("http://127.0.0.1:{}/", req.local_port),
        "github" => {
            let repo = req.github_repo.trim().trim_matches('/');
            if !repo.contains('/') {
                return Err(anyhow!("GitHub 仓库格式应为 owner/repo：{}", repo));
            }
            format!(
                "https://github.com/{}/releases/download/v{}/",
                repo,
                req.version.trim()
            )
        }
        "custom" => {
            let mut b = req.base_url.trim().to_string();
            if b.is_empty() {
                return Err(anyhow!("自定义模式必须填写有效的 https:// 或 http:// 下载基地址"));
            }
            if !b.starts_with("https://") && !b.starts_with("http://") {
                return Err(anyhow!("自定义 BaseUrl 必须以 https:// 或 http:// 开头：{}", b));
            }
            if !b.ends_with('/') {
                b.push('/');
            }
            b
        }
        _ => String::new(),
    };

    let format_url = |fname: &str| -> String {
        if base_url.is_empty() {
            fname.to_string()
        } else {
            format!("{}{}", base_url, encode_uri_component(fname))
        }
    };

    let mut manifest = serde_json::Map::new();
    manifest.insert("schemaVersion".into(), serde_json::json!(1));
    manifest.insert("version".into(), serde_json::json!(req.version.trim()));
    manifest.insert("channel".into(), serde_json::json!(req.channel.trim()));
    manifest.insert(
        "pubDate".into(),
        serde_json::json!(Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()),
    );
    manifest.insert("minVersion".into(), serde_json::json!(req.min_version.trim()));
    manifest.insert("notes".into(), serde_json::json!(req.notes));

    // Pre-validate confinement/existence and compute all required artifact SHA-256 hashes concurrently!
    let inst_name = req.installer_file.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let port_name = req.portable_file.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let mut to_hash: Vec<(String, PathBuf)> = Vec::new();
    let mut seen_hash_keys: HashSet<String> = HashSet::new();
    let mut resolved_paths: HashMap<String, PathBuf> = HashMap::new();

    if let Some(inst) = inst_name {
        crate::guardrails::validate_single_filename(inst)?;
        let p = crate::guardrails::resolve_confined_path(release_dir, inst)?;
        if !p.is_file() {
            return Err(anyhow!("找不到安装包文件：{}", inst));
        }
        resolved_paths.insert(inst.to_string(), p.clone());
        if seen_hash_keys.insert(inst.to_string()) {
            to_hash.push((inst.to_string(), p));
        }
    }
    if let Some(port) = port_name {
        crate::guardrails::validate_single_filename(port)?;
        let p = crate::guardrails::resolve_confined_path(release_dir, port)?;
        if !p.is_file() {
            return Err(anyhow!("找不到便携包文件：{}", port));
        }
        resolved_paths.insert(port.to_string(), p.clone());
        if seen_hash_keys.insert(port.to_string()) {
            to_hash.push((port.to_string(), p));
        }
    }
    let mut extra_valid: Vec<String> = Vec::new();
    for extra in &req.extra_files {
        let name = extra.trim();
        if name.is_empty() || inst_name == Some(name) || port_name == Some(name) {
            continue;
        }
        crate::guardrails::validate_single_filename(name)?;
        let p = crate::guardrails::resolve_confined_path(release_dir, name)?;
        if p.is_file() {
            extra_valid.push(name.to_string());
            resolved_paths.insert(name.to_string(), p.clone());
            if seen_hash_keys.insert(name.to_string()) {
                to_hash.push((name.to_string(), p));
            }
        }
    }
    if to_hash.is_empty() {
        return Err(anyhow!("请至少选择一个安装包或便携包文件"));
    }

    let hash_paths: Vec<PathBuf> = to_hash.iter().map(|(_, p)| p.clone()).collect();
    let hash_results = sha256_files_parallel(&hash_paths);
    let mut sha_map: HashMap<String, String> = HashMap::new();
    for ((name, _), res) in to_hash.into_iter().zip(hash_results.into_iter()) {
        sha_map.insert(name, res?);
    }

    let mut sums: Vec<String> = Vec::new();
    if let Some(inst) = inst_name {
        let p = &resolved_paths[inst];
        let sha = sha_map[inst].clone();
        let size = fs::metadata(p)?.len();
        let url = format_url(inst);
        let entry = serde_json::json!({
            "file": inst,
            "url": url,
            "size": size,
            "sha256": sha,
        });
        manifest.insert("installer".into(), entry.clone());
        manifest.insert(
            "platforms".into(),
            serde_json::json!({
                "windows-x86_64": entry
            }),
        );
        sums.push(format!("{}  {}", sha, inst));
    }

    if let Some(port) = port_name {
        let p = &resolved_paths[port];
        let sha = sha_map[port].clone();
        let size = fs::metadata(p)?.len();
        let url = format_url(port);
        manifest.insert(
            "portable".into(),
            serde_json::json!({
                "file": port,
                "url": url,
                "size": size,
                "sha256": sha,
            }),
        );
        sums.push(format!("{}  {}", sha, port));
    }

    let mut extra_entries: Vec<serde_json::Value> = Vec::new();
    for name in extra_valid {
        if let Some(sha) = sha_map.get(&name) {
            let size = resolved_paths
                .get(&name)
                .and_then(|p| fs::metadata(p).ok())
                .map(|m| m.len())
                .unwrap_or(0);
            extra_entries.push(serde_json::json!({
                "file": name,
                "url": format_url(&name),
                "size": size,
                "sha256": sha,
            }));
            sums.push(format!("{}  {}", sha, name));
        }
    }
    if !extra_entries.is_empty() {
        manifest.insert("extras".into(), serde_json::Value::Array(extra_entries));
    }

    let latest_path = release_dir.join("latest.json");
    let sums_path = release_dir.join("SHA256SUMS.txt");
    let latest_json = format!("{}\n", serde_json::to_string_pretty(&manifest)?);
    let sums_text = format!("{}\n", sums.join("\n"));

    crate::guardrails::atomic_write_bytes(&latest_path, latest_json.as_bytes())?;
    crate::guardrails::atomic_write_bytes(&sums_path, sums_text.as_bytes())?;

    Ok(ManifestResult {
        latest_path: latest_path.to_string_lossy().to_string(),
        sums_path: sums_path.to_string_lossy().to_string(),
        latest_json,
        sums_text,
    })
}

/// Reads the normalized version string (without leading `v`) from `release_dir/latest.json`, if present and valid.
pub fn read_manifest_version(release_dir: &Path) -> Option<String> {
    let latest_path = release_dir.join("latest.json");
    if !latest_path.is_file() {
        return None;
    }
    let raw = fs::read_to_string(&latest_path).ok()?;
    let val: serde_json::Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;
    let ver = val
        .get("version")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .unwrap_or("")
        .trim_start_matches('v')
        .trim_start_matches('V')
        .to_string();
    if ver.is_empty() {
        None
    } else {
        Some(ver)
    }
}

/// Validates `release_dir/latest.json` for a metadata-only sync onto an ALREADY-PUBLISHED Release
/// (without re-uploading binaries). Enforces version alignment, public http(s) URLs, 64-char SHA-256,
/// and verifies every referenced binary already exists on the remote Release (or locally with matching size).
pub fn validate_manifest_for_notes_sync(
    release_dir: &Path,
    expected_tag: &str,
    remote_asset_names: &[String],
) -> Result<()> {
    let latest_path = release_dir.join("latest.json");
    let raw = fs::read_to_string(&latest_path).context("无法读取本地 latest.json 进行同步前校验")?;
    let val: serde_json::Value =
        serde_json::from_str(raw.trim_start_matches('\u{feff}')).context("本地 latest.json 格式损坏")?;

    let expected_ver = expected_tag
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    let mver = val
        .get("version")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .unwrap_or("")
        .trim_start_matches('v')
        .trim_start_matches('V');
    if mver.is_empty() {
        return Err(anyhow!(
            "安全拦截：本地 latest.json 缺少有效的 version 字段，无法同步"
        ));
    }
    if !expected_ver.is_empty() && !mver.eq_ignore_ascii_case(expected_ver) {
        return Err(anyhow!(
            "安全拦截：本地 latest.json 版本为 v{}，与目标 Release {} 不一致，已阻止覆盖",
            mver,
            expected_tag
        ));
    }

    let check_entry = |label: &str, obj: &serde_json::Map<String, serde_json::Value>| -> Result<()> {
        let url = obj.get("url").and_then(|x| x.as_str()).unwrap_or("").trim();
        let url_low = url.to_ascii_lowercase();
        if url_low.is_empty() {
            return Err(anyhow!(
                "安全拦截：latest.json 中的 {} 缺少下载地址 (url)，无法同步",
                label
            ));
        }
        if url_low.contains("127.0.0.1")
            || url_low.contains("localhost")
            || url_low.contains("::1")
            || url_low.contains("0.0.0.0")
            || url_low.starts_with("file:")
            || (!url_low.starts_with("https://") && !url_low.starts_with("http://"))
        {
            return Err(anyhow!(
                "安全拦截：latest.json 中的 {} 下载地址必须为公网 http(s):// 地址（{}）",
                label,
                url
            ));
        }
        let valid_sha = obj
            .get("sha256")
            .and_then(|x| x.as_str())
            .map(|s| s.trim())
            .map(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
            .unwrap_or(false);
        if !valid_sha {
            return Err(anyhow!(
                "安全拦截：latest.json 中的 {} 缺少合法的 64 位 SHA-256 摘要，无法同步",
                label
            ));
        }
        if let Some(fname) = obj.get("file").and_then(|x| x.as_str()) {
            crate::guardrails::validate_single_filename(fname)
                .map_err(|e| anyhow!("安全拦截：latest.json 引用了非法文件名 {}（{}）", fname, e))?;
            if let Some(fver) = extract_artifact_version(fname) {
                if !mver.is_empty() && !fver.eq_ignore_ascii_case(mver) {
                    return Err(anyhow!(
                        "安全拦截：latest.json 声明版本为 v{}，但 {} 指向 v{} 产物，已阻止覆盖",
                        mver,
                        fname,
                        fver
                    ));
                }
            }
            if !remote_asset_names.is_empty() {
                let exists_remote = remote_asset_names
                    .iter()
                    .any(|n| n.eq_ignore_ascii_case(fname));
                if !exists_remote {
                    return Err(anyhow!(
                        "安全拦截：latest.json 引用的产物 {} 尚未存在于线上 Release {} 的资产中，请先上传安装包后再同步清单",
                        fname,
                        expected_tag
                    ));
                }
            } else if let Ok(confined) = crate::guardrails::resolve_confined_path(release_dir, fname) {
                if confined.is_file() {
                    if let Some(es) = obj.get("size").and_then(|x| x.as_u64()) {
                        let actual_size = fs::metadata(&confined)?.len();
                        if es != actual_size {
                            return Err(anyhow!(
                                "安全拦截：{} 的本地体积（{} B）与 latest.json 记录（{} B）不一致，请重新生成清单",
                                fname,
                                actual_size,
                                es
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    };

    let mut entry_count = 0usize;
    for key in ["installer", "portable"] {
        if let Some(obj) = val.get(key).and_then(|x| x.as_object()) {
            entry_count += 1;
            check_entry(key, obj)?;
        }
    }
    if let Some(plats) = val.get("platforms").and_then(|x| x.as_object()) {
        for (plat_key, pv) in plats {
            if let Some(obj) = pv.as_object() {
                entry_count += 1;
                check_entry(&format!("platforms.{}", plat_key), obj)?;
            }
        }
    }
    if let Some(extras) = val.get("extras").and_then(|x| x.as_array()) {
        for (idx, ev) in extras.iter().enumerate() {
            if let Some(obj) = ev.as_object() {
                entry_count += 1;
                check_entry(&format!("extras[{}]", idx), obj)?;
            }
        }
    }
    if entry_count == 0 {
        return Err(anyhow!(
            "安全拦截：latest.json 缺少任何有效的产物条目，禁止同步空清单"
        ));
    }
    Ok(())
}

/// Red-team pre-publish safety gate: prevents publishing a broken, size-mismatched, or localhost-polluted latest.json / SHA256SUMS.txt!
pub fn validate_manifest_for_publish(
    release_dir: &Path,
    expected_tag: &str,
    selected_files: &[String],
) -> Result<()> {
    let uploading_latest = selected_files.iter().any(|f| {
        Path::new(f)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|n| n.eq_ignore_ascii_case("latest.json"))
            .unwrap_or(false)
    });
    let uploading_sums = selected_files.iter().any(|f| {
        Path::new(f)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|n| n.eq_ignore_ascii_case("SHA256SUMS.txt"))
            .unwrap_or(false)
    });
    if !uploading_latest && !uploading_sums {
        return Ok(());
    }

    let selected_names: Vec<String> = selected_files
        .iter()
        .filter_map(|f| {
            Path::new(f)
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect();

    // Collect (label, file_name, confined_path, expected_sha, expected_size) so each unique file is hashed once
    let mut checks: Vec<(String, String, PathBuf, Option<String>, Option<u64>)> = Vec::new();

    if uploading_latest {
        let latest_path = release_dir.join("latest.json");
        let raw = fs::read_to_string(&latest_path).context("无法读取 latest.json 进行发版前校验")?;
        let val: serde_json::Value =
            serde_json::from_str(raw.trim_start_matches('\u{feff}')).context("latest.json 格式损坏")?;

        // 1. Version must be present and align with release tag
        let expected_ver = expected_tag
            .trim()
            .trim_start_matches('v')
            .trim_start_matches('V');
        let mver = val
            .get("version")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .unwrap_or("");
        if mver.is_empty() {
            return Err(anyhow!(
                "安全拦截：latest.json 缺少有效的 version 字段，请重新生成清单后再发版"
            ));
        }
        if !expected_ver.is_empty() && mver != expected_ver {
            return Err(anyhow!(
                "安全拦截：latest.json 内版本为 {}，与目标标签 {} 不一致，请重新生成清单后再发版",
                mver,
                expected_tag
            ));
        }

        // 2. Reject empty/invalid-scheme/loopback URLs, missing assets, missing sha256, and size/SHA-256 drift
        let mut collect_entry = |label: &str, obj: &serde_json::Map<String, serde_json::Value>| -> Result<()> {
            let url = obj.get("url").and_then(|x| x.as_str()).unwrap_or("").trim();
            let url_low = url.to_ascii_lowercase();
            if url_low.is_empty() {
                return Err(anyhow!(
                    "安全拦截：latest.json 中的 {} 缺少下载地址 (url)，请重新生成清单",
                    label
                ));
            }
            if url_low.contains("127.0.0.1")
                || url_low.contains("localhost")
                || url_low.contains("::1")
                || url_low.contains("0.0.0.0")
                || url_low.starts_with("file:")
                || (!url_low.starts_with("https://") && !url_low.starts_with("http://"))
            {
                return Err(anyhow!(
                    "安全拦截：latest.json 中的 {} 下载地址必须为正式公网 http(s):// 地址，禁止相对路径或本机回环（{}）",
                    label,
                    url
                ));
            }
            let expected_sha = obj
                .get("sha256")
                .and_then(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()));
            if expected_sha.is_none() {
                return Err(anyhow!(
                    "安全拦截：latest.json 中的 {} 缺少合法的 64 位 SHA-256 摘要，请重新生成清单",
                    label
                ));
            }
            if let Some(fname) = obj.get("file").and_then(|x| x.as_str()) {
                crate::guardrails::validate_single_filename(fname)
                    .map_err(|e| anyhow!("安全拦截：latest.json 引用了非法文件名 {}（{}）", fname, e))?;
                // Prevent version-poisoned manifests (e.g., latest.json claims v1.46.13 but points to *_1.46.12_*.exe)
                if let Some(fver) = extract_artifact_version(fname) {
                    if !mver.is_empty() && fver != mver {
                        return Err(anyhow!(
                            "安全拦截：latest.json 声明版本为 v{}，但 {} 指向的是 v{} 产物，禁止发布错配清单",
                            mver,
                            fname,
                            fver
                        ));
                    }
                }
                let confined = crate::guardrails::resolve_confined_path(release_dir, fname)
                    .map_err(|e| anyhow!("安全拦截：latest.json 文件路径越界 {}（{}）", fname, e))?;
                if !confined.is_file() {
                    return Err(anyhow!(
                        "安全拦截：latest.json 引用的产物 {} 在本地不存在，请重新打包并生成清单",
                        fname
                    ));
                }
                if !selected_names.iter().any(|n| n == fname) {
                    return Err(anyhow!(
                        "安全拦截：latest.json 引用了 {}，但该文件未被勾选随 Release 上传",
                        fname
                    ));
                }
                let expected_size = obj.get("size").and_then(|x| x.as_u64());
                let actual_size = fs::metadata(&confined)?.len();
                if let Some(es) = expected_size {
                    if es != actual_size {
                        return Err(anyhow!(
                            "安全拦截：{} 的本地实际体积（{} B）与 latest.json 记录（{} B）不一致，请重新生成清单",
                            fname,
                            actual_size,
                            es
                        ));
                    }
                }
                checks.push((label.to_string(), fname.to_string(), confined, expected_sha, expected_size));
            }
            Ok(())
        };

        let mut entry_count = 0usize;
        for key in ["installer", "portable"] {
            if let Some(obj) = val.get(key).and_then(|x| x.as_object()) {
                entry_count += 1;
                collect_entry(key, obj)?;
            }
        }
        if let Some(plats) = val.get("platforms").and_then(|x| x.as_object()) {
            for (plat_key, pv) in plats {
                if let Some(obj) = pv.as_object() {
                    entry_count += 1;
                    collect_entry(&format!("platforms.{}", plat_key), obj)?;
                }
            }
        }
        if let Some(extras) = val.get("extras").and_then(|x| x.as_array()) {
            for (idx, ev) in extras.iter().enumerate() {
                if let Some(obj) = ev.as_object() {
                    entry_count += 1;
                    collect_entry(&format!("extras[{}]", idx), obj)?;
                }
            }
        }
        if entry_count == 0 {
            return Err(anyhow!(
                "安全拦截：latest.json 缺少任何有效的 installer / portable / platforms / extras 产物条目，禁止发布空清单"
            ));
        }
    }

    if uploading_sums {
        let sums_path = release_dir.join("SHA256SUMS.txt");
        if sums_path.is_file() {
            let raw = fs::read_to_string(&sums_path).context("无法读取 SHA256SUMS.txt 进行发版前校验")?;
            for line in raw.lines() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                let mut parts = t.splitn(2, |c: char| c.is_whitespace());
                let expected_sha = parts.next().unwrap_or("").trim().to_ascii_lowercase();
                let fname = parts.next().unwrap_or("").trim().trim_start_matches('*');
                if expected_sha.is_empty() || fname.is_empty() {
                    continue;
                }
                crate::guardrails::validate_single_filename(fname)
                    .map_err(|e| anyhow!("安全拦截：SHA256SUMS.txt 包含非法文件名 {}（{}）", fname, e))?;
                let confined = crate::guardrails::resolve_confined_path(release_dir, fname)
                    .map_err(|e| anyhow!("安全拦截：SHA256SUMS.txt 文件路径越界 {}（{}）", fname, e))?;
                // Only strictly check files that are being uploaded or present on disk
                if selected_names.iter().any(|n| n == fname) {
                    if !confined.is_file() {
                        return Err(anyhow!("安全拦截：SHA256SUMS.txt 引用的 {} 不存在", fname));
                    }
                    checks.push((
                        "SHA256SUMS.txt".to_string(),
                        fname.to_string(),
                        confined,
                        Some(expected_sha),
                        None,
                    ));
                }
            }
        }
    }

    let mut unique_files: Vec<(String, PathBuf)> = Vec::new();
    let mut seen_u: HashSet<String> = HashSet::new();
    for (_, fname, confined, exp, _) in &checks {
        if exp.is_some() && seen_u.insert(fname.clone()) {
            unique_files.push((fname.clone(), confined.clone()));
        }
    }
    let paths: Vec<PathBuf> = unique_files.iter().map(|(_, p)| p.clone()).collect();
    let hashed = sha256_files_parallel(&paths);
    let mut sha_map: HashMap<String, String> = HashMap::new();
    for ((fname, _), res) in unique_files.into_iter().zip(hashed.into_iter()) {
        sha_map.insert(fname, res?);
    }

    for (src_label, fname, _, expected_sha, _) in checks {
        if let Some(expected_sha) = expected_sha {
            let actual_sha = sha_map.get(&fname).map(|s| s.as_str()).unwrap_or("");
            if !actual_sha.eq_ignore_ascii_case(&expected_sha) {
                return Err(anyhow!(
                    "安全拦截：{} 的实际 SHA-256 与 {} 记录不一致，请重新生成清单",
                    fname,
                    if src_label == "SHA256SUMS.txt" { "SHA256SUMS.txt" } else { "latest.json" }
                ));
            }
        }
    }

    Ok(())
}

pub fn verify_checksums(release_dir: &Path) -> Result<Vec<ChecksumVerifyItem>> {
    let sums_path = release_dir.join("SHA256SUMS.txt");
    // (expected_hash, display_name, confined_target_opt, exists, is_invalid_path)
    let mut entries: Vec<(String, String, Option<PathBuf>, bool, bool)> = Vec::new();
    let mut listed_lower = std::collections::HashSet::new();

    if sums_path.exists() {
        let raw = fs::read_to_string(&sums_path)?;
        for line in raw.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            let mut parts = t.splitn(2, |c: char| c.is_whitespace());
            let expected = parts.next().unwrap_or("").trim().to_ascii_lowercase();
            let file = parts.next().unwrap_or("").trim().trim_start_matches('*');
            if expected.is_empty() || file.is_empty() {
                continue;
            }
            listed_lower.insert(file.to_ascii_lowercase());
            match crate::guardrails::resolve_confined_path(release_dir, file) {
                Ok(target) => {
                    let exists = target.is_file();
                    entries.push((expected, file.to_string(), Some(target), exists, false));
                }
                Err(_) => {
                    // Reject path traversal / absolute / symlink escapes without ever hashing outside release_dir!
                    entries.push((expected, file.to_string(), None, false, true));
                }
            }
        }
    }

    // Also inspect active deliverable artifacts not present in SHA256SUMS.txt (e.g. older versions or pre-manifest builds)
    let unlisted_artifacts: Vec<(String, PathBuf)> = scan_artifacts(release_dir)
        .into_iter()
        .filter(|a| {
            !a.is_archived
                && a.kind != "manifest"
                && a.kind != "checksum"
                && !listed_lower.contains(&a.name.to_ascii_lowercase())
        })
        .map(|a| {
            let p = PathBuf::from(&a.path);
            (a.name, p)
        })
        .collect();

    if entries.is_empty() && unlisted_artifacts.is_empty() {
        if !sums_path.exists() {
            return Err(anyhow!("release 目录中不存在 SHA256SUMS.txt，且未发现可核验产物"));
        }
        return Ok(vec![]);
    }

    let mut to_hash: Vec<PathBuf> = entries
        .iter()
        .filter_map(|(_, _, p_opt, ex, invalid)| {
            if !*invalid && *ex {
                p_opt.clone()
            } else {
                None
            }
        })
        .collect();
    for (_, p) in &unlisted_artifacts {
        to_hash.push(p.clone());
    }
    let mut hashed_iter = sha256_files_parallel(&to_hash).into_iter();

    let mut results = Vec::with_capacity(entries.len() + unlisted_artifacts.len());
    for (expected, file, _, exists, invalid) in entries {
        if invalid {
            results.push(ChecksumVerifyItem {
                file,
                expected,
                actual: String::new(),
                status: "invalid".into(),
            });
            continue;
        }
        if !exists {
            results.push(ChecksumVerifyItem {
                file,
                expected,
                actual: String::new(),
                status: "missing".into(),
            });
            continue;
        }
        let actual = hashed_iter
            .next()
            .unwrap_or_else(|| Err(anyhow!("哈希结果数量异常")))?;
        let status = if actual == expected { "ok" } else { "mismatch" };
        results.push(ChecksumVerifyItem {
            file,
            expected,
            actual,
            status: status.into(),
        });
    }

    for (file, _) in unlisted_artifacts {
        let actual = hashed_iter
            .next()
            .unwrap_or_else(|| Err(anyhow!("哈希结果数量异常")))?;
        results.push(ChecksumVerifyItem {
            file,
            expected: String::new(),
            actual,
            status: "unlisted".into(),
        });
    }

    Ok(results)
}

pub fn archive_artifact(release_dir: &Path, file_name: &str) -> Result<String> {
    let clean = crate::guardrails::validate_single_filename(file_name)?;
    let src = crate::guardrails::resolve_confined_path(release_dir, &clean)?;
    if !src.is_file() {
        return Err(anyhow!("找不到可归档的文件：{}", clean));
    }
    let arc_dir = release_dir.join("archive");
    fs::create_dir_all(&arc_dir)?;
    let dst = arc_dir.join(&clean);
    crate::guardrails::retry_on_win_lock(|| fs::rename(&src, &dst))
        .map_err(|e| anyhow!("归档文件 {} 失败：{}", clean, e))?;
    Ok(dst.to_string_lossy().to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PrePart {
    Num(u64),
    Alpha(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SemverSortKey {
    major: u64,
    minor: u64,
    patch: u64,
    is_stable: bool,
    pre: Vec<PrePart>,
}

impl Ord for SemverSortKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch, self.is_stable)
            .cmp(&(other.major, other.minor, other.patch, other.is_stable))
            .then_with(|| self.pre.cmp(&other.pre))
    }
}

impl PartialOrd for SemverSortKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn semver_sort_key(ver: &str) -> SemverSortKey {
    let clean = ver.trim().split('+').next().unwrap_or(ver);
    let (base, pre_raw) = match clean.split_once('-') {
        Some((b, p)) => (b, Some(p)),
        None => (clean, None),
    };
    let mut p = base.split('.');
    let major = p.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let minor = p.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let patch = p.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let (is_stable, pre) = match pre_raw {
        None | Some("") => (true, Vec::new()),
        Some(pr) => {
            let parts = pr
                .split('.')
                .map(|seg| match seg.parse::<u64>() {
                    Ok(n) => PrePart::Num(n),
                    Err(_) => PrePart::Alpha(seg.to_ascii_lowercase()),
                })
                .collect();
            (false, parts)
        }
    };
    SemverSortKey {
        major,
        minor,
        patch,
        is_stable,
        pre,
    }
}

pub fn prune_old_releases(release_dir: &Path, keep_versions: usize) -> Result<Vec<String>> {
    let keep = keep_versions.max(1);
    let artifacts = scan_artifacts(release_dir);
    let mut by_kind: BTreeMap<String, BTreeMap<SemverSortKey, Vec<PathBuf>>> = BTreeMap::new();

    for a in artifacts {
        if a.is_archived || !matches!(a.kind.as_str(), "installer" | "portable") {
            continue;
        }
        let Some(ver) = a.version else {
            continue;
        };
        by_kind
            .entry(a.kind)
            .or_default()
            .entry(semver_sort_key(&ver))
            .or_default()
            .push(PathBuf::from(a.path));
    }

    let mut archived = Vec::new();
    for (_kind, versions) in by_kind {
        let mut keys: Vec<_> = versions.keys().cloned().collect();
        keys.sort();
        keys.reverse();
        for old_key in keys.into_iter().skip(keep) {
            if let Some(paths) = versions.get(&old_key) {
                for p in paths {
                    if let Some(n) = p.file_name().and_then(|s| s.to_str()) {
                        // Move old artifacts safely into `release/archive/` instead of permanently deleting them
                        if archive_artifact(release_dir, n).is_ok() {
                            archived.push(n.to_string());
                        }
                    }
                }
            }
        }
    }
    Ok(archived)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard_exclude_case_insensitive() {
        let ex = vec!["*.pdb".into(), "temp*".into(), "数据位置.json".into()];
        assert!(matches_exclude("app/FOO.PDB", "FOO.PDB", &ex));
        assert!(matches_exclude("app/temp_cache.txt", "temp_cache.txt", &ex));
        assert!(matches_exclude("引擎/数据位置.json", "数据位置.json", &ex));
        assert!(!matches_exclude("app/main.exe", "main.exe", &ex));
    }

    #[test]
    fn test_pack_zip_targz_self_exclusion_and_publish_guard() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_pack_test_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(2)
        ));
        let rel = base.join("release");
        fs::create_dir_all(base.join("app")).unwrap();
        fs::create_dir_all(&rel).unwrap();
        fs::write(base.join("app/hello.txt"), b"hello shipforge").unwrap();
        fs::write(base.join("app/SKIP.PDB"), b"debug").unwrap();

        let res = pack_portable(
            &base,
            &rel,
            &PackageRequest {
                project_id: "demo".into(),
                format: "zip".into(),
                output_name: "Demo_1.2.3_x64-portable.zip".into(),
                sources: vec!["app".into(), "release".into()], // intentionally include release to test self-exclusion!
                excludes: vec!["*.pdb".into()],
                readme_name: Some("README.txt".into()),
                readme_content: Some("Demo portable".into()),
                split_size_mb: None,
                allow_sensitive: false,
            },
        )
        .unwrap();
        assert_eq!(res.file_count, 2);
        assert_eq!(res.sha256.len(), 64);

        // Generate local-mode manifest and ensure publish guard BLOCKS uploading localhost URL!
        generate_manifest(
            &rel,
            &ManifestRequest {
                project_id: "demo".into(),
                version: "1.2.3".into(),
                channel: "stable".into(),
                url_mode: "local".into(),
                github_repo: String::new(),
                base_url: String::new(),
                local_port: 18765,
                installer_file: None,
                portable_file: Some("Demo_1.2.3_x64-portable.zip".into()),
                extra_files: vec![],
                min_version: String::new(),
                notes: "Local test".into(),
            },
        )
        .unwrap();
        let local_err = validate_manifest_for_publish(
            &rel,
            "v1.2.3",
            &["latest.json".into(), "Demo_1.2.3_x64-portable.zip".into()],
        );
        assert!(local_err.is_err());

        // Generate production GitHub manifest and verify publish guard PASSES on matching tag & FAILS on mismatched tag!
        generate_manifest(
            &rel,
            &ManifestRequest {
                project_id: "demo".into(),
                version: "1.2.3".into(),
                channel: "stable".into(),
                url_mode: "github".into(),
                github_repo: "alice/demo".into(),
                base_url: String::new(),
                local_port: 18765,
                installer_file: None,
                portable_file: Some("Demo_1.2.3_x64-portable.zip".into()),
                extra_files: vec![],
                min_version: String::new(),
                notes: "Release 1.2.3".into(),
            },
        )
        .unwrap();
        assert!(validate_manifest_for_publish(
            &rel,
            "v1.2.3",
            &["latest.json".into(), "Demo_1.2.3_x64-portable.zip".into()]
        )
        .is_ok());
        assert!(validate_manifest_for_publish(
            &rel,
            "v1.2.4",
            &["latest.json".into(), "Demo_1.2.3_x64-portable.zip".into()]
        )
        .is_err());

        let verified = verify_checksums(&rel).unwrap();
        assert_eq!(verified.len(), 1);
        assert_eq!(verified[0].status, "ok");

        let tg = pack_portable(
            &base,
            &rel,
            &PackageRequest {
                project_id: "demo".into(),
                format: "targz".into(),
                output_name: "Demo_1.2.3_linux.tar.gz".into(),
                sources: vec!["app".into()],
                excludes: vec!["*.pdb".into()],
                readme_name: Some("README.txt".into()),
                readme_content: Some("Demo tar.gz".into()),
                split_size_mb: None,
                allow_sensitive: false,
            },
        )
        .unwrap();
        assert_eq!(tg.file_count, 2);
        // Verify single-pass HashingWriter matches on-disk SHA-256 byte-for-byte!
        let disk_tg_sha = sha256_file(Path::new(&tg.output_path)).unwrap();
        assert_eq!(tg.sha256, disk_tg_sha);

        // Verify parallel multi-file SHA-256 preserves exact order & values
        let par = sha256_files_parallel(&[
            PathBuf::from(&res.output_path),
            PathBuf::from(&tg.output_path),
        ]);
        assert_eq!(par.len(), 2);
        assert_eq!(par[0].as_ref().unwrap(), &res.sha256);
        assert_eq!(par[1].as_ref().unwrap(), &disk_tg_sha);

        assert!(is_already_compressed_ext("assets/logo.PNG"));
        assert!(is_already_compressed_ext("bundle/data.zip"));
        assert!(!is_already_compressed_ext("bin/app.exe"));

        fs::write(rel.join("Demo_1.0.0_x64-portable.zip"), b"a").unwrap();
        fs::write(rel.join("Demo_1.1.0_x64-portable.zip"), b"b").unwrap();
        let removed = prune_old_releases(&rel, 2).unwrap();
        assert_eq!(removed, vec!["Demo_1.0.0_x64-portable.zip".to_string()]);

        // 0-byte archive split must fail cleanly
        let zero_file = rel.join("empty.zip");
        fs::write(&zero_file, b"").unwrap();
        assert!(split_archive_into_volumes(&zero_file, &rel, "empty.zip", 1024, "0").is_err());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_sensitive_file_guard_and_split_volume_roundtrip() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_split_test_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(53)
        ));
        let rel = base.join("release");
        fs::create_dir_all(base.join("app")).unwrap();
        fs::create_dir_all(&rel).unwrap();
        fs::write(base.join("app/.env"), b"SECRET=123").unwrap();
        fs::write(base.join("app/data.bin"), vec![42u8; 4096]).unwrap();

        // Blocked by default when .env is present
        let blocked = pack_portable(
            &base,
            &rel,
            &PackageRequest {
                project_id: "demo".into(),
                format: "zip".into(),
                output_name: "blocked.zip".into(),
                sources: vec!["app".into()],
                excludes: vec![],
                readme_name: None,
                readme_content: None,
                split_size_mb: None,
                allow_sensitive: false,
            },
        );
        assert!(blocked.is_err());

        // Split archive test: exact chunk size boundary (1024 bytes -> 3 parts on 3072-byte synthetic file)
        let sample = rel.join("sample.zip");
        let payload: Vec<u8> = (0..3072u32).map(|i| (i % 251) as u8).collect();
        fs::write(&sample, &payload).unwrap();
        let orig_sha = sha256_file(&sample).unwrap();
        let (parts, man_path) =
            split_archive_into_volumes(&sample, &rel, "sample.zip", 1024, &orig_sha).unwrap();
        assert_eq!(parts, vec!["sample.zip.001", "sample.zip.002", "sample.zip.003"]);
        assert!(man_path.exists());

        // Reassemble and verify byte-for-byte equality
        let mut combined = Vec::new();
        for p in &parts {
            combined.extend_from_slice(&fs::read(rel.join(p)).unwrap());
        }
        assert_eq!(combined, payload);

        // Stale split part cleanup: create fake .004, re-split with larger chunk -> only 2 parts, .003 & .004 gone!
        fs::write(rel.join("sample.zip.004"), b"stale").unwrap();
        let (parts2, _) =
            split_archive_into_volumes(&sample, &rel, "sample.zip", 2048, &orig_sha).unwrap();
        assert_eq!(parts2, vec!["sample.zip.001", "sample.zip.002"]);
        assert!(!rel.join("sample.zip.003").exists());
        assert!(!rel.join("sample.zip.004").exists());

        // Path traversal & CMD metachar injection must be blocked
        assert!(pack_portable(
            &base,
            &rel,
            &PackageRequest {
                project_id: "demo".into(),
                format: "zip".into(),
                output_name: "bad&calc.zip".into(),
                sources: vec!["app".into()],
                excludes: vec![],
                readme_name: None,
                readme_content: None,
                split_size_mb: None,
                allow_sensitive: true,
            }
        )
        .is_err());
        assert!(pack_portable(
            &base,
            &rel,
            &PackageRequest {
                project_id: "demo".into(),
                format: "zip".into(),
                output_name: "ok.zip".into(),
                sources: vec!["../outside".into()],
                excludes: vec![],
                readme_name: None,
                readme_content: None,
                split_size_mb: None,
                allow_sensitive: true,
            }
        )
        .is_err());

        // 0-byte archive split must fail cleanly
        let zero_file = rel.join("empty.zip");
        fs::write(&zero_file, b"").unwrap();
        assert!(split_archive_into_volumes(&zero_file, &rel, "empty.zip", 1024, "0").is_err());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_verify_checksums_reports_unlisted_active_artifacts() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_sums_unlisted_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(17)
        ));
        fs::create_dir_all(&base).unwrap();
        let cur = base.join("Demo_1.2.0_x64-portable.zip");
        let old = base.join("Demo_1.1.0_x64-portable.zip");
        fs::write(&cur, b"v1.2.0 payload").unwrap();
        fs::write(&old, b"v1.1.0 payload").unwrap();
        let cur_sha = sha256_file(&cur).unwrap();
        let old_sha = sha256_file(&old).unwrap();
        fs::write(
            base.join("SHA256SUMS.txt"),
            format!("{}  Demo_1.2.0_x64-portable.zip\n", cur_sha),
        )
        .unwrap();

        let items = verify_checksums(&base).unwrap();
        assert_eq!(items.len(), 2);
        let cur_item = items.iter().find(|i| i.file == "Demo_1.2.0_x64-portable.zip").unwrap();
        let old_item = items.iter().find(|i| i.file == "Demo_1.1.0_x64-portable.zip").unwrap();
        assert_eq!(cur_item.status, "ok");
        assert_eq!(old_item.status, "unlisted");
        assert_eq!(old_item.actual, old_sha);
        assert!(old_item.expected.is_empty());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_redteam_packager_blocks_zip_slip_traversal_checksums_and_archives_on_prune() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_rt_pack_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(19)
        ));
        let rel = base.join("release");
        fs::create_dir_all(&rel).unwrap();
        fs::write(base.join("app.exe"), b"bin").unwrap();
        fs::write(base.join("secret.txt"), b"TOP_SECRET").unwrap();

        // 1. Zip-Slip in readme_name must be rejected
        let bad_readme = PackageRequest {
            project_id: "rt".into(),
            format: "zip".into(),
            output_name: "ok.zip".into(),
            sources: vec!["app.exe".into()],
            excludes: vec![],
            readme_name: Some("../../evil.txt".into()),
            readme_content: Some("pwned".into()),
            split_size_mb: None,
            allow_sensitive: false,
        };
        assert!(pack_portable(&base, &rel, &bad_readme).is_err());

        // 2. Path traversal in output_name must be rejected (not silently stripped)
        let bad_out = PackageRequest {
            output_name: "../escape.zip".into(),
            readme_name: None,
            readme_content: None,
            ..bad_readme
        };
        assert!(pack_portable(&base, &rel, &bad_out).is_err());

        // 3. Crafted SHA256SUMS.txt pointing outside release_dir must be reported as `invalid`, never hashed
        fs::write(
            rel.join("SHA256SUMS.txt"),
            "deadbeef  ../secret.txt\ndeadbeef  C:\\Windows\\win.ini\n",
        )
        .unwrap();
        let sums = verify_checksums(&rel).unwrap();
        assert_eq!(sums.len(), 2);
        assert!(sums.iter().all(|s| s.status == "invalid" && s.actual.is_empty()));

        // 4. archive_artifact must reject traversal and directories
        fs::create_dir_all(rel.join("archive")).unwrap();
        assert!(archive_artifact(&rel, "../secret.txt").is_err());
        assert!(archive_artifact(&rel, "archive").is_err());

        // 5. prune_old_releases must archive old builds into release/archive/ instead of deleting them!
        fs::write(rel.join("App_1.0.0_x64-portable.zip"), b"v1").unwrap();
        fs::write(rel.join("App_1.1.0_x64-portable.zip"), b"v2").unwrap();
        fs::write(rel.join("App_1.2.0_x64-portable.zip"), b"v3").unwrap();
        let moved = prune_old_releases(&rel, 2).unwrap();
        assert_eq!(moved, vec!["App_1.0.0_x64-portable.zip".to_string()]);
        assert!(!rel.join("App_1.0.0_x64-portable.zip").exists());
        assert!(
            rel.join("archive").join("App_1.0.0_x64-portable.zip").is_file(),
            "prune_old_releases must safely archive old builds into release/archive/"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_round2_packager_blocks_junction_escape_split_bomb_and_manifest_traversal() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_r2_pack_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(43)
        ));
        let proj = base.join("proj");
        let sub = proj.join("sub");
        let rel = proj.join("release");
        let outside = base.join("outside");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(&rel).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(sub.join("ok.txt"), b"safe payload").unwrap();
        fs::write(outside.join("secret.txt"), b"OUTSIDE_SECRET").unwrap();

        // 1. NTFS junction inside packed dir must NOT pull outside files into ZIP!
        #[cfg(windows)]
        {
            let junc = sub.join("leak_dir");
            let st = std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &junc.to_string_lossy(),
                    &outside.to_string_lossy(),
                ])
                .status()
                .unwrap();
            if st.success() {
                let res = pack_portable(
                    &proj,
                    &rel,
                    &PackageRequest {
                        project_id: "r2".into(),
                        format: "zip".into(),
                        output_name: "safe_1.0.0.zip".into(),
                        sources: vec!["sub".into()],
                        excludes: vec![],
                        readme_name: None,
                        readme_content: None,
                        split_size_mb: None,
                        allow_sensitive: true,
                    },
                )
                .unwrap();
                assert_eq!(res.file_count, 1, "Only ok.txt should be packed, never junction contents");
                let _ = fs::remove_dir(&junc);
            }
        }

        // 2. Split volume bomb (>999 parts) must be rejected
        let big = rel.join("data.bin");
        fs::write(&big, vec![0u8; 2000]).unwrap();
        assert!(
            split_archive_into_volumes(&big, &rel, "data.bin", 1, "abc").is_err(),
            "Splitting 2000 bytes into 1-byte chunks (>999 parts) must fail cleanly"
        );

        // 3. validate_manifest_for_publish must reject traversal file inside latest.json
        fs::write(
            rel.join("latest.json"),
            r#"{"version":"1.0.0","installer":{"file":"../secret.txt","url":"https://github.com/a/b/releases/download/v1.0.0/x","sha256":"deadbeef"}}"#,
        )
        .unwrap();
        assert!(
            validate_manifest_for_publish(&rel, "V1.0.0", &["latest.json".into(), "secret.txt".into()])
                .is_err()
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_redteam_manifest_size_mismatch_missing_sha_and_semver_prune() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_rt_manifest_prune_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(91)
        ));
        let _ = fs::remove_dir_all(&base);
        let rel = base.join("release");
        fs::create_dir_all(&rel).unwrap();

        let exe_path = rel.join("App_1.2.0_x64-setup.exe");
        fs::write(&exe_path, b"MZ-real-installer-payload-bytes").unwrap();
        let real_sha = sha256_file(&exe_path).unwrap();
        let real_size = fs::metadata(&exe_path).unwrap().len();

        // 1. Valid manifest passes
        let ok_mf = serde_json::json!({
            "version": "1.2.0",
            "installer": {
                "file": "App_1.2.0_x64-setup.exe",
                "url": "https://github.com/a/b/releases/download/v1.2.0/App_1.2.0_x64-setup.exe",
                "size": real_size,
                "sha256": real_sha,
            }
        });
        fs::write(rel.join("latest.json"), serde_json::to_string(&ok_mf).unwrap()).unwrap();
        fs::write(
            rel.join("SHA256SUMS.txt"),
            format!("{}  App_1.2.0_x64-setup.exe\n", real_sha),
        )
        .unwrap();
        assert!(validate_manifest_for_publish(
            &rel,
            "v1.2.0",
            &[
                "App_1.2.0_x64-setup.exe".into(),
                "SHA256SUMS.txt".into(),
                "latest.json".into()
            ]
        )
        .is_ok());

        // 2. Tampered size in latest.json must be blocked even when sha256 is valid!
        let bad_size_mf = serde_json::json!({
            "version": "1.2.0",
            "installer": {
                "file": "App_1.2.0_x64-setup.exe",
                "url": "https://github.com/a/b/releases/download/v1.2.0/App_1.2.0_x64-setup.exe",
                "size": real_size + 100,
                "sha256": real_sha,
            }
        });
        fs::write(rel.join("latest.json"), serde_json::to_string(&bad_size_mf).unwrap()).unwrap();
        let err_sz = validate_manifest_for_publish(
            &rel,
            "v1.2.0",
            &["App_1.2.0_x64-setup.exe".into(), "latest.json".into()],
        )
        .unwrap_err();
        assert!(err_sz.to_string().contains("体积"), "unexpected: {}", err_sz);

        // 3. Missing sha256 or 0.0.0.0 URL or missing version must be blocked
        let no_sha_mf = serde_json::json!({
            "version": "1.2.0",
            "installer": {
                "file": "App_1.2.0_x64-setup.exe",
                "url": "https://github.com/a/b/releases/download/v1.2.0/App_1.2.0_x64-setup.exe",
                "size": real_size,
            }
        });
        fs::write(rel.join("latest.json"), serde_json::to_string(&no_sha_mf).unwrap()).unwrap();
        assert!(validate_manifest_for_publish(
            &rel,
            "v1.2.0",
            &["App_1.2.0_x64-setup.exe".into(), "latest.json".into()],
        )
        .is_err());

        // 4. Stale SHA256SUMS.txt must be blocked even without latest.json
        fs::write(
            rel.join("SHA256SUMS.txt"),
            format!("{}  App_1.2.0_x64-setup.exe\n", "0".repeat(64)),
        )
        .unwrap();
        assert!(validate_manifest_for_publish(
            &rel,
            "v1.2.0",
            &["App_1.2.0_x64-setup.exe".into(), "SHA256SUMS.txt".into()],
        )
        .is_err());

        // 5. Numeric SemVer prerelease pruning: keep 1.2.0 & 1.2.0-beta.10, archive 1.2.0-beta.2
        let prune_dir = base.join("prune");
        fs::create_dir_all(&prune_dir).unwrap();
        fs::write(prune_dir.join("App_1.2.0-beta.2_x64-portable.zip"), b"a").unwrap();
        fs::write(prune_dir.join("App_1.2.0-beta.10_x64-portable.zip"), b"b").unwrap();
        fs::write(prune_dir.join("App_1.2.0_x64-portable.zip"), b"c").unwrap();
        let archived = prune_old_releases(&prune_dir, 2).unwrap();
        assert_eq!(archived, vec!["App_1.2.0-beta.2_x64-portable.zip".to_string()]);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_extract_artifact_version_and_publish_gate_blocks_poisoned_and_relative_manifests() {
        // Non-greedy version extraction handles hyphenated arch/ext & split volumes accurately
        assert_eq!(extract_artifact_version("MyApp-2.0.0-win64.zip").as_deref(), Some("2.0.0"));
        assert_eq!(extract_artifact_version("App_1.0.0-setup.exe").as_deref(), Some("1.0.0"));
        assert_eq!(
            extract_artifact_version("QDisk_1.46.7-beta.1_x64-setup.exe").as_deref(),
            Some("1.46.7-beta.1")
        );
        assert_eq!(
            extract_artifact_version("MyApp-1.2.3-rc.2-x64-setup.exe").as_deref(),
            Some("1.2.3-rc.2")
        );
        assert_eq!(
            extract_artifact_version("MyApp-1.2.3-beta.1.zip.part001").as_deref(),
            Some("1.2.3-beta.1")
        );

        let base = std::env::temp_dir().join(format!(
            "shipforge_r2_mf_gate_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(97)
        ));
        fs::create_dir_all(&base).unwrap();
        let old_file = base.join("App_1.0.0_x64-setup.exe");
        fs::write(&old_file, b"installer-payload").unwrap();
        let real_sha = sha256_file(&old_file).unwrap();
        let real_sz = fs::metadata(&old_file).unwrap().len();

        // 1. Version-poisoned manifest (claims v1.0.1 but references 1.0.0 installer) -> BLOCKED!
        let poisoned = serde_json::json!({
            "version": "1.0.1",
            "installer": {
                "file": "App_1.0.0_x64-setup.exe",
                "url": "https://github.com/a/b/releases/download/v1.0.1/App_1.0.0_x64-setup.exe",
                "size": real_sz,
                "sha256": real_sha,
            }
        });
        fs::write(base.join("latest.json"), serde_json::to_string(&poisoned).unwrap()).unwrap();
        let err1 = validate_manifest_for_publish(
            &base,
            "v1.0.1",
            &["App_1.0.0_x64-setup.exe".into(), "latest.json".into()],
        )
        .unwrap_err()
        .to_string();
        assert!(err1.contains("错配"), "unexpected err: {err1}");

        // 2. Relative URL without http(s):// scheme -> BLOCKED!
        let rel_url = serde_json::json!({
            "version": "1.0.0",
            "installer": {
                "file": "App_1.0.0_x64-setup.exe",
                "url": "App_1.0.0_x64-setup.exe",
                "size": real_sz,
                "sha256": real_sha,
            }
        });
        fs::write(base.join("latest.json"), serde_json::to_string(&rel_url).unwrap()).unwrap();
        assert!(validate_manifest_for_publish(
            &base,
            "v1.0.0",
            &["App_1.0.0_x64-setup.exe".into(), "latest.json".into()],
        )
        .is_err());

        // 3. Empty manifest with zero entries -> BLOCKED!
        fs::write(base.join("latest.json"), r#"{"version":"1.0.0"}"#).unwrap();
        assert!(validate_manifest_for_publish(&base, "v1.0.0", &["latest.json".into()]).is_err());

        // 4. generate_manifest with custom url_mode and empty base_url -> BLOCKED!
        let bad_custom = ManifestRequest {
            project_id: "p".into(),
            version: "1.0.0".into(),
            channel: "stable".into(),
            url_mode: "custom".into(),
            github_repo: String::new(),
            base_url: String::new(),
            local_port: 18765,
            installer_file: Some("App_1.0.0_x64-setup.exe".into()),
            portable_file: None,
            extra_files: vec![],
            min_version: String::new(),
            notes: "ok".into(),
        };
        assert!(generate_manifest(&base, &bad_custom).is_err(), "empty custom base_url must fail fast");

        let _ = fs::remove_dir_all(&base);
    }
}
