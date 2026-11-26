use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, State};
use zip::ZipArchive;

use crate::config;
use crate::models::UpdaterConfig;
use crate::AppState;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 单次更新清单最大体积（2 MiB），防止恶意超大 JSON 耗尽内存。
pub const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
/// 单次更新安装包/便携包最大体积（512 MiB），防无限流 DoS。
pub const MAX_UPDATE_BYTES: u64 = 512 * 1024 * 1024;
/// 便携包解压总字节数硬上限（256 MiB），防 ZIP 炸弹。
const MAX_PORTABLE_EXTRACT_BYTES: u64 = 256 * 1024 * 1024;
/// 便携包最大条目数上限。
const MAX_PORTABLE_ENTRIES: usize = 2000;
/// 最大重定向跳数。
const MAX_REDIRECTS: usize = 5;

/// 官方内置 GitHub Releases 更新源（独立于 QDisk，指向 ShipForge 专属仓库）。
pub const OFFICIAL_FEED_URL: &str =
    "https://github.com/3347567186/ShipForge/releases/latest/download/latest.json";
pub const OFFICIAL_LATEST_REDIRECT_URL: &str =
    "https://github.com/3347567186/ShipForge/releases/latest";
pub const OFFICIAL_API_LATEST_URL: &str =
    "https://api.github.com/repos/3347567186/ShipForge/releases/latest";
pub const OFFICIAL_API_RELEASES_URL: &str =
    "https://api.github.com/repos/3347567186/ShipForge/releases?per_page=8";
pub const OFFICIAL_RELEASES_ATOM_URL: &str =
    "https://github.com/3347567186/ShipForge/releases.atom";
pub const OFFICIAL_RELEASES_PAGE: &str =
    "https://github.com/3347567186/ShipForge/releases";
pub const OFFICIAL_API_CHANGELOG_URL: &str =
    "https://api.github.com/repos/3347567186/ShipForge/releases?per_page=20";

/// 编译期内嵌完整变更记录，离线状态也可即时查看全部版本历史。
const EMBEDDED_CHANGELOG_MD: &str = include_str!("../../../CHANGELOG.md");

static PART_SEQ: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------- Verifier & SemVer

pub struct Sha256Verifier;

impl Sha256Verifier {
    pub fn verify_file(&self, path: &Path, expected_sha256: &str, expected_size: u64) -> Result<String> {
        let want = normalize_sha256(expected_sha256)?;
        let meta = fs::metadata(path)
            .with_context(|| format!("无法读取待校验文件：{}", path.display()))?;
        if expected_size > 0 && meta.len() != expected_size {
            bail!(
                "文件体积不匹配：期望 {} 字节，实际 {} 字节",
                expected_size,
                meta.len()
            );
        }
        let mut f = File::open(path)
            .with_context(|| format!("无法打开待校验文件：{}", path.display()))?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let got = hex_lower(&hasher.finalize());
        if got != want {
            bail!("SHA-256 校验失败：期望 {want}，实际 {got}");
        }
        Ok(got)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl SemVer {
    pub fn parse(raw: &str) -> Result<Self> {
        let s = raw.trim().trim_start_matches(['v', 'V']);
        let main = s.split('+').next().unwrap_or(s);
        let (core, pre) = match main.split_once('-') {
            Some((c, p)) => {
                if p.is_empty()
                    || p.split('.').any(|seg| {
                        seg.is_empty()
                            || !seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    })
                {
                    bail!("版本号预发布段无效：{raw}");
                }
                (c, Some(p.to_string()))
            }
            None => (main, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() != 3 {
            bail!("版本号格式必须为 MAJOR.MINOR.PATCH：{raw}");
        }
        let major = parts[0].parse::<u64>().with_context(|| format!("版本号无效：{raw}"))?;
        let minor = parts[1].parse::<u64>().with_context(|| format!("版本号无效：{raw}"))?;
        let patch = parts[2].parse::<u64>().with_context(|| format!("版本号无效：{raw}"))?;
        Ok(Self { major, minor, patch, pre })
    }

    pub fn is_prerelease(&self) -> bool {
        self.pre.is_some()
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(a), Some(b)) => compare_prerelease(a, b),
            })
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn compare_prerelease(a: &str, b: &str) -> std::cmp::Ordering {
    let sa: Vec<&str> = a.split('.').collect();
    let sb: Vec<&str> = b.split('.').collect();
    for (x, y) in sa.iter().zip(sb.iter()) {
        let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(nx), Ok(ny)) => nx.cmp(&ny),
            (Ok(_), Err(_)) => std::cmp::Ordering::Less,
            (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
            (Err(_), Err(_)) => x.cmp(y),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    sa.len().cmp(&sb.len())
}

// ---------------------------------------------------------------- Models & State

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAsset {
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseManifest {
    pub version: String,
    pub channel: String,
    pub pub_date: String,
    pub min_version: String,
    pub notes: String,
    pub installer: Option<ReleaseAsset>,
    pub portable: Option<ReleaseAsset>,
    pub manifest_url: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub is_ignored: bool,
    pub is_prerelease: bool,
    pub install_kind: String,
    pub package_kind: String,
    pub channel: String,
    pub pub_date: String,
    pub notes: String,
    pub download_url: String,
    pub file_name: String,
    pub size: u64,
    pub sha256: String,
    pub manifest_url: String,
    pub staged: Option<StagedInfo>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedInfo {
    pub version: String,
    pub package_kind: String,
    pub file_name: String,
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdaterProgressPayload {
    pub phase: String,
    pub downloaded: u64,
    pub total: u64,
    pub speed_bps: u64,
    pub percent: u8,
    pub version: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdaterStatusSnapshot {
    pub current_version: String,
    pub install_kind: String,
    pub busy: bool,
    pub config: UpdaterConfig,
    pub releases_page: String,
    pub updates_dir: String,
    pub cache_bytes: u64,
    pub staged: Option<StagedInfo>,
}

pub struct UpdaterState {
    busy: AtomicBool,
    cancel: Arc<AtomicBool>,
    staged: Mutex<Option<StagedInfo>>,
}

impl Default for UpdaterState {
    fn default() -> Self {
        Self {
            busy: AtomicBool::new(false),
            cancel: Arc::new(AtomicBool::new(false)),
            staged: Mutex::new(None),
        }
    }
}

struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------- Path & URL Security Helpers

pub fn updates_dir() -> PathBuf {
    config::app_data_dir().join("updates")
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn detect_install_kind_for(dir: &Path) -> &'static str {
    let low = dir.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    if dir.join("uninstall.exe").is_file()
        || dir.join("Uninstall.exe").is_file()
        || low.contains("\\appdata\\local\\programs\\")
        || low.contains("\\program files\\")
        || low.contains("\\program files (x86)\\")
    {
        "installer"
    } else {
        "portable"
    }
}

pub fn detect_install_kind() -> &'static str {
    detect_install_kind_for(&exe_dir())
}

pub fn is_local_host(host: &str) -> bool {
    let h = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    matches!(h.as_str(), "localhost" | "127.0.0.1" | "::1")
}

pub fn split_url(raw: &str) -> std::result::Result<(bool, String, u16, String), String> {
    let s = raw.trim();
    let (secure, rest) = if let Some(r) = s.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = s.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(format!("仅支持 http:// 或 https:// 协议：{s}"));
    };
    if rest.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("URL 包含非法空白或控制字符".into());
    }
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], rest[idx..].to_string()),
        None => (rest, "/".to_string()),
    };
    if authority.is_empty() {
        return Err("URL 缺少主机名".into());
    }
    if authority.contains('@') {
        return Err("URL 不允许携带认证凭据".into());
    }
    let default_port = if secure { 443 } else { 80 };
    let (host, port) = if authority.starts_with('[') {
        let end = authority
            .find(']')
            .ok_or_else(|| "IPv6 主机名缺少右括号".to_string())?;
        let h = &authority[1..end];
        let tail = &authority[end + 1..];
        let p = if let Some(ps) = tail.strip_prefix(':') {
            ps.parse::<u16>().map_err(|_| format!("端口号无效：{ps}"))?
        } else if tail.is_empty() {
            default_port
        } else {
            return Err(format!("主机名格式非法：{authority}"));
        };
        (h.to_string(), p)
    } else if let Some((h, ps)) = authority.rsplit_once(':') {
        let p = ps.parse::<u16>().map_err(|_| format!("端口号无效：{ps}"))?;
        (h.to_string(), p)
    } else {
        (authority.to_string(), default_port)
    };
    if host.is_empty() {
        return Err("URL 主机名为空".into());
    }
    Ok((secure, host, port, path))
}

pub fn normalize_sha256(raw: &str) -> Result<String> {
    let cleaned = raw
        .trim()
        .strip_prefix("sha256:")
        .unwrap_or(raw.trim())
        .trim()
        .to_ascii_lowercase();
    if cleaned.len() != 64 || !cleaned.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("SHA-256 摘要格式无效（必须为 64 位十六进制字符串）");
    }
    Ok(cleaned)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

pub fn sanitize_file_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() || name.len() > 180 {
        bail!("更新文件名无效");
    }
    if name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains(':')
        || name.chars().any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        bail!("更新文件名包含非法路径字符：{name}");
    }
    Ok(name.to_string())
}

pub fn is_trusted_release_host(host: &str) -> bool {
    let low = host.to_ascii_lowercase();
    is_local_host(&low)
        || matches!(
            low.as_str(),
            "github.com"
                | "api.github.com"
                | "objects.githubusercontent.com"
                | "release-assets.githubusercontent.com"
                | "codeload.github.com"
        )
        || low.ends_with(".githubusercontent.com")
}

/// 仅放行指向回环地址（127.0.0.1 / localhost / ::1）的本机联调环境变量。
fn dev_loopback_feed() -> Option<String> {
    for key in ["SHIPFORGE_UPDATE_FEED", "UPDATE_FEED_URL"] {
        if let Ok(raw) = std::env::var(key) {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok((_, host, _, _)) = split_url(trimmed) {
                if is_local_host(&host) {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub fn validate_update_url(url: &str, origin_is_remote_https: bool) -> Result<(bool, String, u16, String)> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        bail!("更新地址不能为空");
    }
    if trimmed.len() > 2048 {
        bail!("更新地址过长");
    }
    let (secure, host, port, path) = split_url(trimmed).map_err(anyhow::Error::msg)?;
    if host.contains('@') {
        bail!("更新地址不允许携带认证凭据");
    }
    let local = is_local_host(&host);
    if !secure && !local {
        bail!("出于安全要求，远端更新地址必须使用 https://");
    }
    if !local && !is_trusted_release_host(&host) {
        bail!("安全拦截：拒绝连接非官方受信任更新域名（{host}）");
    }
    let path_only = path.split(['?', '#']).next().unwrap_or(&path);
    let path_low = path_only.to_ascii_lowercase();
    if path_low.contains("..")
        || path_low.contains('\\')
        || path_low.contains("%2e")
        || path_low.contains("%2f")
        || path_low.contains("%5c")
        || path_low.contains("%00")
    {
        bail!("安全拦截：更新路径包含非法穿越或编码序列");
    }
    if host.eq_ignore_ascii_case("github.com") && !path_low.starts_with("/3347567186/shipforge/") {
        bail!("安全拦截：拒绝从非官方仓库路径下载更新（{path_only}）");
    }
    if host.eq_ignore_ascii_case("api.github.com")
        && !path_low.starts_with("/repos/3347567186/shipforge/")
    {
        bail!("安全拦截：拒绝访问非官方仓库 API（{path_only}）");
    }
    if origin_is_remote_https && (!secure || local) {
        bail!("拒绝从远端 HTTPS 重定向到非 HTTPS 或本机回环地址");
    }
    Ok((secure, host, port, path))
}

pub fn resolve_asset_url(manifest_url: &str, raw_asset_url: &str) -> Result<String> {
    let u = raw_asset_url.trim();
    if u.is_empty() {
        bail!("更新清单缺少下载地址");
    }
    if u.starts_with("https://") || u.starts_with("http://") {
        return Ok(u.to_string());
    }
    if u.starts_with("//") || u.contains("://") {
        bail!("不支持的下载地址协议：{u}");
    }
    let base_no_q = manifest_url.split('?').next().unwrap_or(manifest_url);
    let dir = match base_no_q.rfind('/') {
        Some(i) => &base_no_q[..=i],
        None => bail!("无法基于清单地址解析相对下载路径"),
    };
    let rel = u.trim_start_matches("./").trim_start_matches('/');
    Ok(format!("{dir}{rel}"))
}

// ---------------------------------------------------------------- WinHTTP Streaming Client

pub fn describe_winhttp_error(code: u32) -> &'static str {
    match code {
        12002 => "连接超时",
        12007 => "无法解析域名（DNS 失败）",
        12015 => "代理认证失败",
        12017 => "请求已取消",
        12029 => "无法连接服务器（可能需检查代理或网络）",
        12030 => "连接被服务器中断",
        12031 => "连接被重置",
        12038 | 12044 | 12045 | 12175 => "TLS/SSL 安全连接握手失败（请检查系统时间或代理证书）",
        12152 => "服务器返回了无效响应",
        12178 => "系统自动代理配置无法获取",
        _ => "网络请求失败",
    }
}

pub fn extract_tag_from_release_location(loc: &str) -> Option<String> {
    let trimmed = loc.trim();
    let no_q = trimmed.split(['?', '#']).next()?;
    let idx = no_q.find("/releases/tag/")?;
    let tag = no_q[idx + "/releases/tag/".len()..].trim_matches('/');
    if tag.is_empty()
        || tag.len() > 80
        || tag.contains('/')
        || tag.contains('\\')
        || tag.contains("..")
        || !tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return None;
    }
    Some(tag.to_string())
}

pub fn extract_release_tags_from_atom(xml: &str) -> Vec<String> {
    const ABS_PREFIX: &str = "https://github.com/3347567186/ShipForge/releases/tag/";
    const REL_PREFIX: &str = "/3347567186/ShipForge/releases/tag/";

    let mut ranked: Vec<(SemVer, String)> = Vec::new();
    let mut cursor = xml;
    while let Some(s) = cursor.find("<entry") {
        let after_open = &cursor[s + 6..];
        let Some(first_ch) = after_open.bytes().next() else {
            break;
        };
        if !matches!(first_ch, b'>' | b' ' | b'\t' | b'\r' | b'\n') {
            cursor = after_open;
            continue;
        }
        let Some(tag_end) = after_open.find('>') else {
            break;
        };
        let body_start = &after_open[tag_end + 1..];
        let Some(e) = body_start.find("</entry>") else {
            break;
        };
        let entry = &body_start[..e];
        cursor = &body_start[e + 8..];

        let mut scan = entry;
        let mut chosen_tag: Option<String> = None;
        while let Some(ls) = scan.find('<') {
            let tail = &scan[ls..];
            if let Some(after_tag) = tail.strip_prefix("<content").or_else(|| tail.strip_prefix("<summary")) {
                if matches!(after_tag.bytes().next(), Some(b'>' | b' ' | b'\t' | b'\r' | b'\n')) {
                    let close_marker = if tail.starts_with("<content") { "</content>" } else { "</summary>" };
                    if let Some(ce) = tail.find(close_marker) {
                        scan = &tail[ce + close_marker.len()..];
                        continue;
                    }
                    break;
                }
            }
            if let Some(after_link) = tail.strip_prefix("<link") {
                if matches!(after_link.bytes().next(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    if let Some(gt) = after_link.find('>') {
                        let attrs = &after_link[..gt];
                        if let Some(href) = extract_xml_href_attr(attrs) {
                            let h = href.trim();
                            let matched_repo = h.len() > ABS_PREFIX.len()
                                && h[..ABS_PREFIX.len()].eq_ignore_ascii_case(ABS_PREFIX)
                                || h.len() > REL_PREFIX.len()
                                    && h[..REL_PREFIX.len()].eq_ignore_ascii_case(REL_PREFIX);
                            if matched_repo {
                                if let Some(tag) = extract_tag_from_release_location(h) {
                                    chosen_tag = Some(tag);
                                    break;
                                }
                            }
                        }
                        scan = &after_link[gt + 1..];
                        continue;
                    }
                    break;
                }
            }
            scan = &tail[1..];
        }
        if let Some(tag) = chosen_tag {
            if let Ok(sv) = SemVer::parse(tag.trim_start_matches(['v', 'V'])) {
                if !ranked.iter().any(|(_, t)| t == &tag) {
                    ranked.push((sv, tag));
                }
            }
        }
    }
    ranked.sort_by(|a, b| b.0.cmp(&a.0));
    ranked.into_iter().map(|(_, t)| t).collect()
}

fn extract_xml_href_attr(attrs: &str) -> Option<&str> {
    let b = attrs.as_bytes();
    let mut i = 0;
    while i + 6 < b.len() {
        let boundary = i == 0 || matches!(b[i - 1], b' ' | b'\t' | b'\r' | b'\n');
        if boundary && b[i..].starts_with(b"href=") {
            let q = b[i + 5];
            if q == b'"' || q == b'\'' {
                let start = i + 6;
                if let Some(end_rel) = b[start..].iter().position(|&c| c == q) {
                    return Some(&attrs[start..start + end_rel]);
                }
            }
        }
        i += 1;
    }
    None
}

pub fn parse_content_range(header: &str) -> Option<(u64, u64, Option<u64>)> {
    let trimmed = header.trim();
    let rest = trimmed
        .strip_prefix("bytes ")
        .or_else(|| trimmed.strip_prefix("BYTES "))
        .or_else(|| {
            if trimmed.len() >= 6 && trimmed[..6].eq_ignore_ascii_case("bytes ") {
                Some(&trimmed[6..])
            } else {
                None
            }
        })?
        .trim();
    let (range_part, total_part) = rest.split_once('/')?;
    let (s_str, e_str) = range_part.trim().split_once('-')?;
    let start = s_str.trim().parse::<u64>().ok()?;
    let end = e_str.trim().parse::<u64>().ok()?;
    if end < start {
        return None;
    }
    let total = if total_part.trim() == "*" {
        None
    } else {
        let t = total_part.trim().parse::<u64>().ok()?;
        if t == 0 || end >= t {
            return None;
        }
        Some(t)
    };
    Some((start, end, total))
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct StreamResponseMeta {
    resumed_from: u64,
    total_len: u64,
    etag: Option<String>,
}

#[cfg(windows)]
enum StreamEvent<'a> {
    Headers(&'a StreamResponseMeta),
    Chunk(&'a [u8], u64),
}

#[cfg(windows)]
fn is_retryable_transport_err(msg: &str) -> bool {
    msg.contains("12002")
        || msg.contains("12015")
        || msg.contains("12029")
        || msg.contains("12030")
        || msg.contains("12031")
        || msg.contains("12152")
        || msg.contains("12175")
        || msg.contains("12178")
        || msg.contains("超时")
        || msg.contains("中断")
        || msg.contains("重置")
        || msg.contains("HTTP 416")
        || msg.contains("HTTP 500")
        || msg.contains("HTTP 502")
        || msg.contains("HTTP 503")
        || msg.contains("HTTP 504")
}

#[cfg(windows)]
fn is_dns_err(msg: &str) -> bool {
    msg.contains("12007") || msg.contains("DNS")
}

#[cfg(windows)]
fn winhttp_fetch_bytes(
    url: &str,
    max_bytes: usize,
    timeout_ms: u64,
    accept: &str,
) -> Result<(u32, Vec<u8>, String)> {
    let mut last_err = None;
    for attempt in 0..2 {
        let mut sink = Vec::new();
        let mut total_written = 0u64;
        match winhttp_stream_get(
            url,
            timeout_ms,
            accept,
            None,
            max_bytes as u64,
            &mut |chunk, _expected| {
                total_written += chunk.len() as u64;
                if total_written > max_bytes as u64 {
                    bail!("响应体积超过上限（{} B）", max_bytes);
                }
                sink.extend_from_slice(chunk);
                Ok(())
            },
        ) {
            Ok((status, final_url)) => return Ok((status, sink, final_url)),
            Err(e) => {
                let msg = format!("{e:#}");
                let retryable = is_retryable_transport_err(&msg) && !is_dns_err(&msg);
                last_err = Some(e);
                if attempt == 0 && retryable {
                    std::thread::sleep(Duration::from_millis(350));
                    continue;
                }
                break;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("获取更新清单失败")))
}

#[cfg(windows)]
fn winhttp_stream_get<F>(
    initial_url: &str,
    timeout_ms: u64,
    accept: &str,
    cancel: Option<&AtomicBool>,
    hard_cap_bytes: u64,
    on_chunk: &mut F,
) -> Result<(u32, String)>
where
    F: FnMut(&[u8], u64) -> Result<()>,
{
    let (_, init_host, _, _) = validate_update_url(initial_url, false)?;
    if is_local_host(&init_host) {
        return winhttp_stream_get_with_proxy(
            initial_url,
            timeout_ms,
            accept,
            cancel,
            hard_cap_bytes,
            false,
            0,
            None,
            &mut |ev| match ev {
                StreamEvent::Headers(_) => Ok(()),
                StreamEvent::Chunk(c, total) => on_chunk(c, total),
            },
        );
    }
    let mut received_any = false;
    let first = winhttp_stream_get_with_proxy(
        initial_url,
        timeout_ms,
        accept,
        cancel,
        hard_cap_bytes,
        true,
        0,
        None,
        &mut |ev| match ev {
            StreamEvent::Headers(_) => Ok(()),
            StreamEvent::Chunk(chunk, expected) => {
                received_any = true;
                on_chunk(chunk, expected)
            }
        },
    );
    match first {
        Ok(ok) => Ok(ok),
        Err(err) => {
            if received_any || cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
                return Err(err);
            }
            let msg = format!("{err:#}");
            if !is_retryable_transport_err(&msg) {
                return Err(err);
            }
            winhttp_stream_get_with_proxy(
                initial_url,
                timeout_ms,
                accept,
                cancel,
                hard_cap_bytes,
                false,
                0,
                None,
                &mut |ev| match ev {
                    StreamEvent::Headers(_) => Ok(()),
                    StreamEvent::Chunk(c, total) => on_chunk(c, total),
                },
            )
            .map_err(|direct_err| anyhow::anyhow!("{direct_err:#}（自动代理模式：{msg}）"))
        }
    }
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn winhttp_stream_get_with_proxy<F>(
    initial_url: &str,
    timeout_ms: u64,
    accept: &str,
    cancel: Option<&AtomicBool>,
    hard_cap_bytes: u64,
    use_auto_proxy: bool,
    range_start: u64,
    if_range_etag: Option<&str>,
    on_event: &mut F,
) -> Result<(u32, String)>
where
    F: FnMut(StreamEvent<'_>) -> Result<()>,
{
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Networking::WinHttp::{
        WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
        WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ACCESS_TYPE_NO_PROXY,
        WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_DISABLE_FEATURE,
        WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_QUERY_CONTENT_LENGTH,
        WINHTTP_QUERY_CONTENT_RANGE, WINHTTP_QUERY_ETAG, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_LOCATION, WINHTTP_QUERY_STATUS_CODE,
    };

    const WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2: u32 = 0x0000_0800;
    const WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3: u32 = 0x0000_2000;

    let (init_sec, init_host, _, _) = validate_update_url(initial_url, false)?;
    let origin_remote_https = init_sec && !is_local_host(&init_host);
    let mut current_url = initial_url.trim().to_string();
    let timeout = timeout_ms.clamp(2000, 180_000) as i32;
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let agent_w = wide(&format!("ShipForge/{}", env!("CARGO_PKG_VERSION")));

    for hop in 0..=MAX_REDIRECTS {
        if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
            bail!("已取消更新下载");
        }
        let (secure, host, port, path) =
            validate_update_url(&current_url, hop > 0 && origin_remote_https)?;
        let local = is_local_host(&host);
        let access = if local || !use_auto_proxy {
            WINHTTP_ACCESS_TYPE_NO_PROXY
        } else {
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY
        };
        let mut header_text = format!(
            "Accept: {accept}\r\nAccept-Encoding: identity\r\nCache-Control: no-cache\r\n"
        );
        if host.eq_ignore_ascii_case("api.github.com") {
            header_text.push_str("X-GitHub-Api-Version: 2022-11-28\r\n");
        }
        if range_start > 0 {
            header_text.push_str(&format!("Range: bytes={range_start}-\r\n"));
            if let Some(et) = if_range_etag {
                let clean = et.trim();
                if !clean.is_empty()
                    && !clean.starts_with("W/")
                    && !clean.chars().any(|c| c == '\r' || c == '\n' || c == '\0')
                {
                    header_text.push_str(&format!("If-Range: {clean}\r\n"));
                }
            }
        }
        let header_w = wide(&header_text);
        let host_w = wide(&host);
        let path_w = wide(&path);
        let method_w = wide("GET");

        let next_location: Option<String> = unsafe {
            let session = WinHttpOpen(agent_w.as_ptr(), access, ptr::null(), ptr::null(), 0);
            if session.is_null() {
                let code = GetLastError();
                bail!(
                    "WinHTTP 初始化失败：{}（错误码 {code}）",
                    describe_winhttp_error(code)
                );
            }
            let step = (|| -> Result<Result<Option<String>, (u32, String)>> {
                WinHttpSetTimeouts(session, timeout, timeout, timeout, timeout);
                let mut protos =
                    WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3;
                if WinHttpSetOption(
                    session,
                    WINHTTP_OPTION_SECURE_PROTOCOLS,
                    &mut protos as *mut _ as *mut c_void,
                    std::mem::size_of::<u32>() as u32,
                ) == 0
                {
                    let mut tls12 = WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2;
                    let _ = WinHttpSetOption(
                        session,
                        WINHTTP_OPTION_SECURE_PROTOCOLS,
                        &mut tls12 as *mut _ as *mut c_void,
                        std::mem::size_of::<u32>() as u32,
                    );
                }
                let connect = WinHttpConnect(session, host_w.as_ptr(), port, 0);
                if connect.is_null() {
                    let code = GetLastError();
                    bail!(
                        "无法连接更新服务器 {host}:{port}：{}（{code}）",
                        describe_winhttp_error(code)
                    );
                }
                let flags = if secure { WINHTTP_FLAG_SECURE } else { 0 };
                let request = WinHttpOpenRequest(
                    connect,
                    method_w.as_ptr(),
                    path_w.as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    flags,
                );
                if request.is_null() {
                    WinHttpCloseHandle(connect);
                    bail!("无法创建更新请求");
                }
                let inner = (|| -> Result<Result<Option<String>, (u32, String)>> {
                    let mut disable_redir: u32 = WINHTTP_DISABLE_REDIRECTS;
                    WinHttpSetOption(
                        request,
                        WINHTTP_OPTION_DISABLE_FEATURE,
                        &mut disable_redir as *mut _ as *mut c_void,
                        std::mem::size_of::<u32>() as u32,
                    );
                    if WinHttpSendRequest(
                        request,
                        header_w.as_ptr(),
                        u32::MAX,
                        ptr::null_mut(),
                        0,
                        0,
                        0,
                    ) == 0
                    {
                        let code = GetLastError();
                        bail!(
                            "连接更新服务器失败：{}（错误码 {code}）",
                            describe_winhttp_error(code)
                        );
                    }
                    if WinHttpReceiveResponse(request, ptr::null_mut()) == 0 {
                        let code = GetLastError();
                        bail!(
                            "更新服务器无响应：{}（错误码 {code}）",
                            describe_winhttp_error(code)
                        );
                    }
                    let mut status: u32 = 0;
                    let mut slen = std::mem::size_of::<u32>() as u32;
                    WinHttpQueryHeaders(
                        request,
                        WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                        ptr::null(),
                        &mut status as *mut _ as *mut c_void,
                        &mut slen,
                        ptr::null_mut(),
                    );
                    if matches!(status, 301 | 302 | 303 | 307 | 308) {
                        let mut loc_bytes: u32 = 0;
                        WinHttpQueryHeaders(
                            request,
                            WINHTTP_QUERY_LOCATION,
                            ptr::null(),
                            ptr::null_mut(),
                            &mut loc_bytes,
                            ptr::null_mut(),
                        );
                        if loc_bytes == 0 || loc_bytes > 8192 {
                            bail!("服务器返回重定向但缺少有效 Location");
                        }
                        let mut wbuf = vec![0u16; (loc_bytes as usize / 2) + 2];
                        let mut actual = loc_bytes;
                        if WinHttpQueryHeaders(
                            request,
                            WINHTTP_QUERY_LOCATION,
                            ptr::null(),
                            wbuf.as_mut_ptr() as *mut c_void,
                            &mut actual,
                            ptr::null_mut(),
                        ) == 0
                        {
                            bail!("读取重定向地址失败");
                        }
                        let end = wbuf.iter().position(|&c| c == 0).unwrap_or(wbuf.len());
                        let loc = String::from_utf16_lossy(&wbuf[..end]).trim().to_string();
                        if loc.is_empty() {
                            bail!("重定向目标为空");
                        }
                        let resolved = if loc.starts_with("http://") || loc.starts_with("https://") {
                            loc
                        } else if loc.starts_with("//") {
                            let scheme = if secure { "https" } else { "http" };
                            format!("{scheme}:{loc}")
                        } else if loc.starts_with('/') {
                            let scheme = if secure { "https" } else { "http" };
                            let host_fmt = if host.contains(':') {
                                format!("[{host}]")
                            } else {
                                host.clone()
                            };
                            let default_p = if secure { 443 } else { 80 };
                            if port == default_p {
                                format!("{scheme}://{host_fmt}{loc}")
                            } else {
                                format!("{scheme}://{host_fmt}:{port}{loc}")
                            }
                        } else {
                            resolve_asset_url(&current_url, &loc)?
                        };
                        return Ok(Ok(Some(resolved)));
                    }

                    if status == 403 || status == 429 {
                        bail!("更新服务器触发访问限流（HTTP {status}）");
                    }
                    if status == 416 {
                        bail!("断点续传偏移超出范围（HTTP 416）");
                    }
                    if status != 200 && !(status == 206 && range_start > 0) {
                        bail!("更新服务器返回 HTTP {status}");
                    }

                    let mut cl_buf = [0u16; 64];
                    let mut cl_bytes = (cl_buf.len() * 2) as u32;
                    let expected_len = if WinHttpQueryHeaders(
                        request,
                        WINHTTP_QUERY_CONTENT_LENGTH,
                        ptr::null(),
                        cl_buf.as_mut_ptr() as *mut c_void,
                        &mut cl_bytes,
                        ptr::null_mut(),
                    ) != 0
                    {
                        let end = cl_buf.iter().position(|&c| c == 0).unwrap_or(cl_buf.len());
                        String::from_utf16_lossy(&cl_buf[..end])
                            .trim()
                            .parse::<u64>()
                            .unwrap_or(0)
                    } else {
                        0
                    };

                    let (resumed_from, total_len, segment_expected_len) = if status == 206 {
                        let mut cr_buf = [0u16; 128];
                        let mut cr_bytes = (cr_buf.len() * 2) as u32;
                        if WinHttpQueryHeaders(
                            request,
                            WINHTTP_QUERY_CONTENT_RANGE,
                            ptr::null(),
                            cr_buf.as_mut_ptr() as *mut c_void,
                            &mut cr_bytes,
                            ptr::null_mut(),
                        ) == 0
                        {
                            bail!("服务器返回 HTTP 206 但缺少 Content-Range 头");
                        }
                        let end = cr_buf.iter().position(|&c| c == 0).unwrap_or(cr_buf.len());
                        let cr_str = String::from_utf16_lossy(&cr_buf[..end]);
                        let Some((cr_start, cr_end, cr_total)) = parse_content_range(&cr_str) else {
                            bail!("服务器返回的 Content-Range 格式非法：{cr_str}");
                        };
                        if cr_start != range_start {
                            bail!(
                                "断点续传起始偏移不匹配（请求 {range_start}，服务器返回 {cr_start}）"
                            );
                        }
                        let range_span = cr_end.saturating_sub(cr_start).saturating_add(1);
                        if expected_len > 0 && expected_len != range_span {
                            bail!(
                                "断点续传分片长度不一致（Content-Length {expected_len} != Content-Range {range_span}）"
                            );
                        }
                        let full_total = cr_total.unwrap_or(cr_end.saturating_add(1));
                        let seg_len = if expected_len > 0 { expected_len } else { range_span };
                        (cr_start, full_total, seg_len)
                    } else {
                        (0u64, expected_len, expected_len)
                    };

                    if total_len > hard_cap_bytes || segment_expected_len > hard_cap_bytes {
                        bail!(
                            "更新文件体积过大（{} B，超过上限 {} B）",
                            total_len.max(segment_expected_len),
                            hard_cap_bytes
                        );
                    }

                    let mut et_buf = [0u16; 128];
                    let mut et_bytes = (et_buf.len() * 2) as u32;
                    let etag = if WinHttpQueryHeaders(
                        request,
                        WINHTTP_QUERY_ETAG,
                        ptr::null(),
                        et_buf.as_mut_ptr() as *mut c_void,
                        &mut et_bytes,
                        ptr::null_mut(),
                    ) != 0
                    {
                        let end = et_buf.iter().position(|&c| c == 0).unwrap_or(et_buf.len());
                        let s = String::from_utf16_lossy(&et_buf[..end]).trim().to_string();
                        if s.is_empty() || s.starts_with("W/") {
                            None
                        } else {
                            Some(s)
                        }
                    } else {
                        None
                    };

                    let meta = StreamResponseMeta {
                        resumed_from,
                        total_len,
                        etag,
                    };
                    on_event(StreamEvent::Headers(&meta))?;

                    let mut buf = vec![0u8; 64 * 1024];
                    let mut seen: u64 = 0;
                    loop {
                        if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
                            bail!("已取消更新下载");
                        }
                        let mut available: u32 = 0;
                        if WinHttpQueryDataAvailable(request, &mut available) == 0 {
                            let code = GetLastError();
                            bail!(
                                "读取更新数据失败：{}（错误码 {code}）",
                                describe_winhttp_error(code)
                            );
                        }
                        if available == 0 {
                            break;
                        }
                        let want = available.min(buf.len() as u32);
                        let mut got: u32 = 0;
                        if WinHttpReadData(
                            request,
                            buf.as_mut_ptr() as *mut c_void,
                            want,
                            &mut got,
                        ) == 0
                        {
                            let code = GetLastError();
                            bail!(
                                "读取更新数据中断：{}（错误码 {code}）",
                                describe_winhttp_error(code)
                            );
                        }
                        if got == 0 {
                            break;
                        }
                        seen = seen.saturating_add(got as u64);
                        if resumed_from.saturating_add(seen) > hard_cap_bytes {
                            bail!("下载体积超过安全上限（{} B）", hard_cap_bytes);
                        }
                        on_event(StreamEvent::Chunk(&buf[..got as usize], total_len))?;
                    }
                    if segment_expected_len > 0 && seen != segment_expected_len {
                        bail!(
                            "更新下载在传输中途中断（本段期望 {} B，实际接收 {} B）",
                            segment_expected_len,
                            seen
                        );
                    }
                    Ok(Err((status, current_url.clone())))
                })();
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
                inner
            })();
            WinHttpCloseHandle(session);
            match step? {
                Ok(loc) => loc,
                Err(done) => return Ok(done),
            }
        };

        if let Some(next) = next_location {
            if hop == MAX_REDIRECTS {
                bail!("更新地址重定向次数过多（超过 {MAX_REDIRECTS} 次）");
            }
            current_url = next;
        } else {
            break;
        }
    }
    bail!("更新地址重定向次数过多")
}

// ---------------------------------------------------------------- Manifest & Release Parsing

pub fn parse_sha256sums_text(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut parts = t.splitn(2, |c: char| c.is_whitespace());
        let Some(hash) = parts.next() else { continue };
        let Some(rest) = parts.next() else { continue };
        let name = rest.trim().trim_start_matches('*').trim();
        if let Ok(norm) = normalize_sha256(hash) {
            if !name.is_empty() {
                map.insert(name.to_ascii_lowercase(), norm);
            }
        }
    }
    map
}

fn parse_asset_obj(manifest_url: &str, v: &Value) -> Result<ReleaseAsset> {
    let raw_url = v
        .get("url")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim();
    if raw_url.is_empty() {
        bail!("更新资产缺少 url 字段");
    }
    let url = resolve_asset_url(manifest_url, raw_url)?;
    let raw_file = v
        .get("file")
        .or_else(|| v.get("name"))
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            url.split('?')
                .next()
                .unwrap_or(&url)
                .rsplit('/')
                .next()
                .unwrap_or("")
        });
    let file = sanitize_file_name(raw_file)?;
    let sha_raw = v
        .get("sha256")
        .or_else(|| v.get("digest"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let sha256 = normalize_sha256(sha_raw)?;
    let size = v.get("size").and_then(|x| x.as_u64()).unwrap_or(0);
    Ok(ReleaseAsset {
        file,
        url,
        size,
        sha256,
    })
}

pub fn parse_static_manifest(manifest_url: &str, v: &Value) -> Result<ReleaseManifest> {
    let version = v
        .get("version")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    let _ = SemVer::parse(&version)?;

    let channel = v
        .get("channel")
        .and_then(|x| x.as_str())
        .unwrap_or("stable")
        .trim()
        .to_ascii_lowercase();
    let pub_date = v
        .get("pubDate")
        .or_else(|| v.get("pub_date"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let min_version = v
        .get("minVersion")
        .or_else(|| v.get("min_version"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let notes = v
        .get("notes")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();

    let mut installer = match v.get("installer") {
        Some(obj) if obj.is_object() => Some(parse_asset_obj(manifest_url, obj)?),
        _ => None,
    };
    let portable = match v.get("portable") {
        Some(obj) if obj.is_object() => Some(parse_asset_obj(manifest_url, obj)?),
        _ => None,
    };
    if installer.is_none() {
        if let Some(win) = v
            .get("platforms")
            .and_then(|p| p.get("windows-x86_64"))
            .filter(|x| x.is_object())
        {
            installer = Some(parse_asset_obj(manifest_url, win)?);
        }
    }
    if installer.is_none() && portable.is_none() {
        bail!("更新清单未包含可用的 installer 或 portable 产物");
    }

    Ok(ReleaseManifest {
        version,
        channel: if channel.is_empty() { "stable".into() } else { channel },
        pub_date,
        min_version,
        notes,
        installer,
        portable,
        manifest_url: manifest_url.to_string(),
    })
}

#[cfg(windows)]
fn probe_latest_tag_via_redirect(timeout_ms: u64) -> Result<String> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Networking::WinHttp::{
        WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
        WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ACCESS_TYPE_NO_PROXY,
        WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_DISABLE_FEATURE,
        WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_LOCATION,
        WINHTTP_QUERY_STATUS_CODE,
    };
    const TLS12: u32 = 0x0000_0800;
    const TLS13: u32 = 0x0000_2000;

    let (secure, host, port, path) = validate_update_url(OFFICIAL_LATEST_REDIRECT_URL, false)?;
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let agent_w = wide(&format!("ShipForge/{}", env!("CARGO_PKG_VERSION")));
    let host_w = wide(&host);
    let path_w = wide(&path);
    let method_w = wide("GET");
    let timeout = timeout_ms.clamp(2000, 30_000) as i32;

    let mut last_err = None;
    for use_auto in [true, false] {
        let access = if use_auto {
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY
        } else {
            WINHTTP_ACCESS_TYPE_NO_PROXY
        };
        let res = unsafe {
            let session = WinHttpOpen(agent_w.as_ptr(), access, ptr::null(), ptr::null(), 0);
            if session.is_null() {
                Err(anyhow::anyhow!("WinHTTP 初始化失败"))
            } else {
                let out = (|| -> Result<String> {
                    WinHttpSetTimeouts(session, timeout, timeout, timeout, timeout);
                    let mut protos = TLS12 | TLS13;
                    if WinHttpSetOption(
                        session,
                        WINHTTP_OPTION_SECURE_PROTOCOLS,
                        &mut protos as *mut _ as *mut c_void,
                        std::mem::size_of::<u32>() as u32,
                    ) == 0
                    {
                        let mut p12 = TLS12;
                        let _ = WinHttpSetOption(
                            session,
                            WINHTTP_OPTION_SECURE_PROTOCOLS,
                            &mut p12 as *mut _ as *mut c_void,
                            std::mem::size_of::<u32>() as u32,
                        );
                    }
                    let connect = WinHttpConnect(session, host_w.as_ptr(), port, 0);
                    if connect.is_null() {
                        let code = GetLastError();
                        bail!("无法连接 GitHub：{}（{code}）", describe_winhttp_error(code));
                    }
                    let flags = if secure { WINHTTP_FLAG_SECURE } else { 0 };
                    let request = WinHttpOpenRequest(
                        connect,
                        method_w.as_ptr(),
                        path_w.as_ptr(),
                        ptr::null(),
                        ptr::null(),
                        ptr::null(),
                        flags,
                    );
                    if request.is_null() {
                        WinHttpCloseHandle(connect);
                        bail!("无法创建标签探测请求");
                    }
                    let inner = (|| -> Result<String> {
                        let mut disable_redir: u32 = WINHTTP_DISABLE_REDIRECTS;
                        WinHttpSetOption(
                            request,
                            WINHTTP_OPTION_DISABLE_FEATURE,
                            &mut disable_redir as *mut _ as *mut c_void,
                            std::mem::size_of::<u32>() as u32,
                        );
                        if WinHttpSendRequest(request, ptr::null(), 0, ptr::null_mut(), 0, 0, 0) == 0 {
                            let code = GetLastError();
                            bail!("探测最新版本失败：{}（{code}）", describe_winhttp_error(code));
                        }
                        if WinHttpReceiveResponse(request, ptr::null_mut()) == 0 {
                            let code = GetLastError();
                            bail!("探测最新版本超时：{}（{code}）", describe_winhttp_error(code));
                        }
                        let mut status: u32 = 0;
                        let mut slen = std::mem::size_of::<u32>() as u32;
                        WinHttpQueryHeaders(
                            request,
                            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                            ptr::null(),
                            &mut status as *mut _ as *mut c_void,
                            &mut slen,
                            ptr::null_mut(),
                        );
                        if !matches!(status, 301 | 302 | 303 | 307 | 308) {
                            bail!("未获得版本重定向（HTTP {status}）");
                        }
                        let mut loc_bytes: u32 = 0;
                        WinHttpQueryHeaders(
                            request,
                            WINHTTP_QUERY_LOCATION,
                            ptr::null(),
                            ptr::null_mut(),
                            &mut loc_bytes,
                            ptr::null_mut(),
                        );
                        if loc_bytes == 0 || loc_bytes > 4096 {
                            bail!("重定向缺少有效 Location");
                        }
                        let mut wbuf = vec![0u16; (loc_bytes as usize / 2) + 2];
                        let mut actual = loc_bytes;
                        if WinHttpQueryHeaders(
                            request,
                            WINHTTP_QUERY_LOCATION,
                            ptr::null(),
                            wbuf.as_mut_ptr() as *mut c_void,
                            &mut actual,
                            ptr::null_mut(),
                        ) == 0
                        {
                            bail!("读取版本重定向地址失败");
                        }
                        let end = wbuf.iter().position(|&c| c == 0).unwrap_or(wbuf.len());
                        let loc = String::from_utf16_lossy(&wbuf[..end]);
                        extract_tag_from_release_location(&loc)
                            .context("无法从重定向地址解析最新版本号")
                    })();
                    WinHttpCloseHandle(request);
                    WinHttpCloseHandle(connect);
                    inner
                })();
                WinHttpCloseHandle(session);
                out
            }
        };
        match res {
            Ok(tag) => return Ok(tag),
            Err(e) => {
                let msg = format!("{e:#}");
                last_err = Some(e);
                if !is_retryable_transport_err(&msg) {
                    break;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("无法探测最新版本标签")))
}

#[cfg(windows)]
fn fetch_and_parse_manifest(feed_url: &str) -> Result<ReleaseManifest> {
    let (_status, bytes, final_url) = winhttp_fetch_bytes(
        feed_url,
        MAX_MANIFEST_BYTES,
        9_000,
        "application/json, application/vnd.github+json",
    )?;
    let raw_v: Value = serde_json::from_slice(&bytes).context("更新清单不是合法的 JSON 格式")?;

    let v = if let Some(arr) = raw_v.as_array() {
        let non_drafts: Vec<&Value> = arr
            .iter()
            .filter(|item| !item.get("draft").and_then(|d| d.as_bool()).unwrap_or(false))
            .collect();
        non_drafts
            .iter()
            .filter_map(|item| {
                let tag = item.get("tag_name").and_then(|t| t.as_str())?;
                let sv = SemVer::parse(tag.trim().trim_start_matches(['v', 'V'])).ok()?;
                Some((sv, *item))
            })
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, item)| item.clone())
            .or_else(|| non_drafts.first().map(|item| (*item).clone()))
            .context("当前仓库尚未发布任何公开版本")?
    } else {
        raw_v
    };

    if v.get("tag_name").is_some() && v.get("assets").and_then(|a| a.as_array()).is_some() {
        let assets = v["assets"].as_array().unwrap();
        if let Some(manifest_asset) = assets.iter().find(|a| {
            a.get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("latest.json"))
        }) {
            if let Some(dl) = manifest_asset.get("browser_download_url").and_then(|u| u.as_str()) {
                if let Ok(m) = winhttp_fetch_bytes(dl, MAX_MANIFEST_BYTES, 9_000, "application/json")
                    .and_then(|(_, mbytes, murl)| {
                        let mv: Value = serde_json::from_slice(&mbytes)
                            .context("GitHub Release 内的 latest.json 解析失败")?;
                        parse_static_manifest(&murl, &mv)
                    })
                {
                    return Ok(m);
                }
            }
        }
        let mut sums = HashMap::new();
        if let Some(sum_asset) = assets.iter().find(|a| {
            a.get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("SHA256SUMS.txt"))
        }) {
            if let Some(dl) = sum_asset.get("browser_download_url").and_then(|u| u.as_str()) {
                if let Ok((_, sbytes, _)) = winhttp_fetch_bytes(dl, 256 * 1024, 9_000, "text/plain") {
                    sums = parse_sha256sums_text(&String::from_utf8_lossy(&sbytes));
                }
            }
        }
        return parse_github_release(&final_url, &v, &sums);
    }

    parse_static_manifest(&final_url, &v)
}

pub fn parse_github_release(
    manifest_url: &str,
    v: &Value,
    sums: &HashMap<String, String>,
) -> Result<ReleaseManifest> {
    let version = v
        .get("tag_name")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    let _ = SemVer::parse(&version)?;
    let prerelease = v.get("prerelease").and_then(|x| x.as_bool()).unwrap_or(false);
    let pub_date = v
        .get("published_at")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let notes = v
        .get("body")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let assets = v
        .get("assets")
        .and_then(|a| a.as_array())
        .context("GitHub Release 缺少 assets 字段")?;

    let mut installer: Option<ReleaseAsset> = None;
    let mut portable: Option<ReleaseAsset> = None;

    for a in assets {
        let name = a.get("name").and_then(|x| x.as_str()).unwrap_or("").trim();
        let url = a
            .get("browser_download_url")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim();
        let size = a.get("size").and_then(|x| x.as_u64()).unwrap_or(0);
        if name.is_empty() || url.is_empty() {
            continue;
        }
        let low = name.to_ascii_lowercase();
        let is_inst = low.ends_with(".exe") && low.contains("setup");
        let is_port = low.ends_with(".zip") && (low.contains("便携") || low.contains("portable"));
        if !is_inst && !is_port {
            continue;
        }
        let sha_raw = a
            .get("digest")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .or_else(|| sums.get(&low).cloned())
            .unwrap_or_default();
        let Ok(sha256) = normalize_sha256(&sha_raw) else {
            continue;
        };
        let Ok(file) = sanitize_file_name(name) else {
            continue;
        };
        let item = ReleaseAsset {
            file,
            url: url.to_string(),
            size,
            sha256,
        };
        if is_inst && installer.is_none() {
            installer = Some(item);
        } else if is_port && portable.is_none() {
            portable = Some(item);
        }
    }

    if installer.is_none() && portable.is_none() {
        bail!("GitHub Release 中未找到带 SHA-256 摘要的安装包或便携包（请随包上传 latest.json 或 SHA256SUMS.txt）");
    }

    Ok(ReleaseManifest {
        version,
        channel: if prerelease { "beta".into() } else { "stable".into() },
        pub_date,
        min_version: String::new(),
        notes,
        installer,
        portable,
        manifest_url: manifest_url.to_string(),
    })
}

// ---------------------------------------------------------------- Config & Disk Helpers

fn read_updater_cfg(app_state: &AppState) -> UpdaterConfig {
    app_state
        .config
        .lock()
        .map(|g| g.updater.clone())
        .unwrap_or_default()
}

fn update_updater_cfg(app_state: &AppState, patch: &Value) -> Result<UpdaterConfig, String> {
    let mut guard = app_state
        .config
        .lock()
        .map_err(|_| "配置锁异常".to_string())?;
    let next = &mut guard.updater;
    if let Some(obj) = patch.as_object() {
        if let Some(ch) = obj.get("channel").and_then(|x| x.as_str()) {
            next.channel = if ch == "beta" { "beta".into() } else { "stable".into() };
        }
        if let Some(ac) = obj
            .get("autoCheck")
            .or_else(|| obj.get("auto_check"))
            .and_then(|x| x.as_bool())
        {
            next.auto_check = ac;
        }
        if let Some(ig) = obj
            .get("ignoredVersion")
            .or_else(|| obj.get("ignored_version"))
            .and_then(|x| x.as_str())
        {
            next.ignored_version = ig.trim().to_string();
        }
        if let Some(lc) = obj
            .get("lastCheckAt")
            .or_else(|| obj.get("last_check_at"))
            .and_then(|x| x.as_str())
        {
            next.last_check_at = lc.trim().to_string();
        }
    }
    let cloned = next.clone();
    config::save_config(&guard).map_err(|e| format!("保存更新配置失败：{e}"))?;
    Ok(cloned)
}

fn dir_size_bytes(dir: &Path) -> u64 {
    let mut sum = 0u64;
    if let Ok(rd) = fs::read_dir(dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            if let Ok(m) = fs::symlink_metadata(&p) {
                if m.file_type().is_symlink() {
                    continue;
                }
                if m.is_dir() {
                    sum = sum.saturating_add(dir_size_bytes(&p));
                } else {
                    sum = sum.saturating_add(m.len());
                }
            }
        }
    }
    sum
}

fn sweep_old_updates(keep_dir: Option<&Path>) {
    let root = updates_dir();
    let Ok(rd) = fs::read_dir(&root) else { return };
    for ent in rd.flatten() {
        let p = ent.path();
        if keep_dir.is_some_and(|k| k == p) {
            continue;
        }
        if let Ok(m) = fs::symlink_metadata(&p) {
            if m.is_dir() && !m.file_type().is_symlink() {
                let _ = fs::remove_dir_all(&p);
            } else {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

// ---------------------------------------------------------------- Core Evaluation & Download

pub fn evaluate_manifest(
    current_version: &str,
    install_kind: &str,
    channel: &str,
    ignored_version: &str,
    manifest: &ReleaseManifest,
    staged: Option<StagedInfo>,
) -> Result<UpdateCheckResult> {
    let cur = SemVer::parse(current_version)?;
    let latest = SemVer::parse(&manifest.version)?;

    if !manifest.min_version.is_empty() {
        if let Ok(min_v) = SemVer::parse(&manifest.min_version) {
            if cur < min_v {
                bail!(
                    "当前版本 {current_version} 低于在线升级最低要求 {}，请手动下载完整安装包升级",
                    manifest.min_version
                );
            }
        }
    }

    let is_pre = latest.is_prerelease() || manifest.channel == "beta";
    let channel_allows = channel == "beta" || !is_pre;
    let has_update = latest > cur && channel_allows;
    let is_ignored = !ignored_version.is_empty() && ignored_version == manifest.version;

    let (package_kind, asset) = if install_kind == "installer" {
        if let Some(a) = &manifest.installer {
            ("installer", a)
        } else if let Some(a) = &manifest.portable {
            ("portable", a)
        } else {
            bail!("清单缺少可下载产物");
        }
    } else if let Some(a) = &manifest.portable {
        ("portable", a)
    } else if let Some(a) = &manifest.installer {
        ("installer", a)
    } else {
        bail!("清单缺少可下载产物");
    };

    let matched_staged = staged.filter(|s| {
        s.version == manifest.version
            && s.sha256.eq_ignore_ascii_case(&asset.sha256)
            && Path::new(&s.path).is_file()
    });

    Ok(UpdateCheckResult {
        current_version: current_version.to_string(),
        latest_version: manifest.version.clone(),
        has_update,
        is_ignored,
        is_prerelease: is_pre,
        install_kind: install_kind.to_string(),
        package_kind: package_kind.to_string(),
        channel: manifest.channel.clone(),
        pub_date: manifest.pub_date.clone(),
        notes: manifest.notes.clone(),
        download_url: asset.url.clone(),
        file_name: asset.file.clone(),
        size: asset.size,
        sha256: asset.sha256.clone(),
        manifest_url: manifest.manifest_url.clone(),
        staged: matched_staged,
    })
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
pub fn download_and_verify_to(
    updates_root: &Path,
    url: &str,
    expected_sha256: &str,
    expected_size: u64,
    version: &str,
    file_name: &str,
    package_kind: &str,
    cancel: &AtomicBool,
    app: Option<&AppHandle>,
) -> Result<StagedInfo> {
    if !matches!(package_kind, "installer" | "portable") {
        bail!("未知的更新包类型：{package_kind}");
    }
    let cur = SemVer::parse(env!("CARGO_PKG_VERSION"))?;
    let target_ver = SemVer::parse(version)?;
    if target_ver <= cur {
        bail!(
            "拒绝降级或重复安装：目标版本 {version} 不高于当前版本 {}",
            env!("CARGO_PKG_VERSION")
        );
    }
    let want_sha = normalize_sha256(expected_sha256)?;
    let safe_name = sanitize_file_name(file_name)?;
    validate_update_url(url, false)?;

    fs::create_dir_all(updates_root).context("无法创建更新缓存目录")?;
    let ver_dir = updates_root.join(format!("v{}", version.trim().trim_start_matches(['v', 'V'])));
    fs::create_dir_all(&ver_dir).context("无法创建版本更新目录")?;
    let final_path = ver_dir.join(&safe_name);

    if final_path.is_file() {
        if let Ok(verified) = Sha256Verifier.verify_file(&final_path, &want_sha, expected_size) {
            let size = fs::metadata(&final_path).map(|m| m.len()).unwrap_or(expected_size);
            verify_binary_magic(&final_path, package_kind)?;
            return Ok(StagedInfo {
                version: version.to_string(),
                package_kind: package_kind.to_string(),
                file_name: safe_name,
                path: final_path.to_string_lossy().to_string(),
                size,
                sha256: verified,
            });
        }
        let _ = fs::remove_file(&final_path);
    }

    let seq = PART_SEQ.fetch_add(1, Ordering::Relaxed);
    let part_path = ver_dir.join(format!("{safe_name}.{}.{seq}.part", std::process::id()));

    let emit_prog = |phase: &str, downloaded: u64, total: u64, speed_bps: u64| {
        let pct = if total > 0 {
            ((downloaded.saturating_mul(100)) / total).min(100) as u8
        } else {
            0
        };
        if let Some(a) = app {
            let _ = a.emit(
                "updater-progress",
                UpdaterProgressPayload {
                    phase: phase.to_string(),
                    downloaded,
                    total,
                    speed_bps,
                    percent: pct,
                    version: version.to_string(),
                },
            );
        }
    };

    emit_prog("downloading", 0, expected_size, 0);

    let dl_res = (|| -> Result<(u64, String)> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&part_path)
            .with_context(|| format!("无法创建临时更新文件：{}", part_path.display()))?;
        let (_, init_host, _, _) = validate_update_url(url, false)?;
        let is_local = is_local_host(&init_host);
        let mut use_auto_proxy = !is_local;
        let mut downloaded: u64 = 0;
        let mut total_hint = expected_size;
        let started = Instant::now();
        let mut last_emit = Instant::now();

        const MAX_DOWNLOAD_ATTEMPTS: usize = 5;
        const RETRY_BACKOFF_MS: [u64; 4] = [500, 1200, 2400, 3500];
        let mut last_err: Option<anyhow::Error> = None;
        let mut pinned_etag: Option<String> = None;
        let mut stalled_streak: usize = 0;

        for attempt in 0..MAX_DOWNLOAD_ATTEMPTS {
            if cancel.load(Ordering::SeqCst) {
                bail!("已取消更新下载");
            }
            if expected_size > 0 && downloaded >= expected_size {
                break;
            }
            let resume_offset = downloaded;
            let bytes_before = downloaded;
            let etag_for_req = if resume_offset > 0 {
                pinned_etag.clone()
            } else {
                None
            };

            let step = winhttp_stream_get_with_proxy(
                url,
                30_000,
                "application/octet-stream, */*",
                Some(cancel),
                MAX_UPDATE_BYTES,
                use_auto_proxy,
                resume_offset,
                etag_for_req.as_deref(),
                &mut |ev| match ev {
                    StreamEvent::Headers(meta) => {
                        if expected_size > 0 && meta.total_len > 0 && meta.total_len != expected_size
                        {
                            bail!(
                                "服务器文件体积与更新清单不一致（清单期望 {} B，服务器声明 {} B）",
                                expected_size,
                                meta.total_len
                            );
                        }
                        if total_hint == 0 && meta.total_len > 0 {
                            total_hint = meta.total_len;
                        }
                        if meta.resumed_from == 0 {
                            if let Some(ref et) = meta.etag {
                                pinned_etag = Some(et.clone());
                            }
                            if downloaded > 0 {
                                file.set_len(0)?;
                                file.seek(SeekFrom::Start(0))?;
                                downloaded = 0;
                            }
                        } else {
                            file.seek(SeekFrom::Start(meta.resumed_from))?;
                            downloaded = meta.resumed_from;
                        }
                        Ok(())
                    }
                    StreamEvent::Chunk(chunk, content_len) => {
                        let next_len = downloaded.saturating_add(chunk.len() as u64);
                        if expected_size > 0 && next_len > expected_size {
                            bail!(
                                "下载体积超出清单声明（期望 {} B，已接收 {} B）",
                                expected_size,
                                next_len
                            );
                        }
                        file.write_all(chunk)?;
                        downloaded = next_len;
                        if total_hint == 0 && content_len > 0 {
                            total_hint = content_len;
                        }
                        if last_emit.elapsed() >= Duration::from_millis(100) {
                            let secs = started.elapsed().as_secs_f64().max(0.05);
                            let speed = (downloaded as f64 / secs) as u64;
                            emit_prog("downloading", downloaded, total_hint, speed);
                            last_emit = Instant::now();
                        }
                        Ok(())
                    }
                },
            );

            let normalized_step = match step {
                Ok(_) if expected_size > 0 && downloaded < expected_size => Err(anyhow::anyhow!(
                    "更新下载在传输中途中断（清单期望 {} B，已接收 {} B）",
                    expected_size,
                    downloaded
                )),
                other => other,
            };

            match normalized_step {
                Ok(_) => {
                    last_err = None;
                    break;
                }
                Err(e) => {
                    let _ = file.flush();
                    if cancel.load(Ordering::SeqCst) {
                        return Err(e);
                    }
                    let msg = format!("{e:#}");
                    if msg.contains("HTTP 416") {
                        let _ = file.set_len(0);
                        let _ = file.seek(SeekFrom::Start(0));
                        downloaded = 0;
                        pinned_etag = None;
                    }
                    if downloaded > bytes_before {
                        stalled_streak = 0;
                    } else {
                        stalled_streak += 1;
                    }
                    let retryable = is_retryable_transport_err(&msg) && !is_dns_err(&msg);
                    last_err = Some(e);
                    if !retryable || attempt + 1 >= MAX_DOWNLOAD_ATTEMPTS {
                        break;
                    }
                    if !is_local && use_auto_proxy && (downloaded == 0 || stalled_streak >= 2) {
                        use_auto_proxy = false;
                        stalled_streak = 0;
                    }
                    let wait_ms = RETRY_BACKOFF_MS[attempt.min(RETRY_BACKOFF_MS.len() - 1)];
                    let mut slept = 0u64;
                    while slept < wait_ms {
                        if cancel.load(Ordering::SeqCst) {
                            bail!("已取消更新下载");
                        }
                        let step_ms = (wait_ms - slept).min(100);
                        std::thread::sleep(Duration::from_millis(step_ms));
                        slept += step_ms;
                    }
                }
            }
        }
        if let Some(err) = last_err {
            return Err(err);
        }

        file.sync_all()?;
        drop(file);

        let disk_size = fs::metadata(&part_path)?.len();
        emit_prog("verifying", disk_size, total_hint.max(disk_size), 0);
        let got_sha = Sha256Verifier.verify_file(&part_path, &want_sha, expected_size)?;
        verify_binary_magic(&part_path, package_kind)?;
        Ok((disk_size, got_sha))
    })();

    let (downloaded, got_sha) = match dl_res {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_file(&part_path);
            emit_prog("error", 0, expected_size, 0);
            return Err(e);
        }
    };

    if let Err(e) = fs::rename(&part_path, &final_path) {
        let _ = fs::remove_file(&part_path);
        return Err(e).context("落盘更新包失败");
    }

    emit_prog("ready", downloaded, downloaded, 0);
    Ok(StagedInfo {
        version: version.to_string(),
        package_kind: package_kind.to_string(),
        file_name: safe_name,
        path: final_path.to_string_lossy().to_string(),
        size: downloaded,
        sha256: got_sha,
    })
}

pub fn verify_binary_magic(path: &Path, package_kind: &str) -> Result<()> {
    let mut f = File::open(path).with_context(|| format!("无法打开更新包：{}", path.display()))?;
    let mut head = [0u8; 4];
    let n = f.read(&mut head)?;
    if package_kind == "installer" {
        if n < 2 || &head[..2] != b"MZ" {
            bail!("安装包文件头无效（非合法 Windows 可执行安装程序）");
        }
    } else if package_kind == "portable" {
        if n < 4 || &head[..4] != b"PK\x03\x04" {
            bail!("便携包文件头无效（非合法 ZIP 压缩包）");
        }
    } else {
        bail!("未知的更新包类型：{package_kind}");
    }
    Ok(())
}

/// 安全解压便携更新包：拦截 Zip Slip 路径穿越、符号链接、条目数超限及解压炸弹。
pub fn extract_portable_zip_safe(zip_path: &Path, dst_dir: &Path) -> Result<()> {
    fs::create_dir_all(dst_dir).context("无法创建便携更新暂存目录")?;
    let canon_dst = fs::canonicalize(dst_dir).context("无法规范化暂存目录")?;
    let file = File::open(zip_path).context("无法打开便携 ZIP 包")?;
    let mut archive = ZipArchive::new(file).context("便携 ZIP 格式无效")?;
    if archive.len() > MAX_PORTABLE_ENTRIES {
        bail!("便携包条目数量超过安全上限（{}）", MAX_PORTABLE_ENTRIES);
    }

    let mut total_written: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("读取 ZIP 条目失败")?;
        if entry.is_symlink() {
            bail!("安全拦截：便携包禁止包含符号链接条目");
        }
        let Some(rel) = entry.enclosed_name() else {
            bail!("安全拦截：便携包包含非法路径穿越条目");
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out_path = canon_dst.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
            let canon_parent = fs::canonicalize(parent)?;
            if !canon_parent.starts_with(&canon_dst) {
                bail!("安全拦截：解压目标目录逃逸出受控暂存区");
            }
        }
        let remaining = MAX_PORTABLE_EXTRACT_BYTES.saturating_sub(total_written);
        let mut limited = (&mut entry).take(remaining.saturating_add(1));
        let mut out_file = File::create(&out_path)?;
        let copied = std::io::copy(&mut limited, &mut out_file)?;
        total_written = total_written.saturating_add(copied);
        if total_written > MAX_PORTABLE_EXTRACT_BYTES {
            bail!("安全拦截：便携包解压体积超过上限（256 MiB）");
        }
    }
    Ok(())
}

/// 定位便携包解压后的实际程序根目录（兼容平铺与单层外壳目录），并验证 shipforge.exe 完整有效。
pub fn find_portable_payload_root(stage_dir: &Path) -> Result<PathBuf> {
    let min_exe_bytes: u64 = if cfg!(test) { 8 } else { 512 * 1024 };
    let valid_root = |d: &Path| -> bool {
        let exe = d.join("shipforge.exe");
        let Ok(smeta) = fs::symlink_metadata(&exe) else { return false };
        if smeta.file_type().is_symlink() || !smeta.is_file() {
            return false;
        }
        if smeta.len() < min_exe_bytes {
            return false;
        }
        verify_binary_magic(&exe, "installer").is_ok()
    };
    if valid_root(stage_dir) {
        return Ok(stage_dir.to_path_buf());
    }
    let mut dirs = Vec::new();
    if let Ok(rd) = fs::read_dir(stage_dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                dirs.push(p);
            }
        }
    }
    if dirs.len() == 1 && valid_root(&dirs[0]) {
        return Ok(dirs.remove(0));
    }
    bail!("便携更新包结构不完整：缺少合法的 shipforge.exe")
}

pub fn validate_staged_before_install(updates_root: &Path, staged: &StagedInfo) -> Result<PathBuf> {
    if !matches!(staged.package_kind.as_str(), "installer" | "portable") {
        bail!("未知的更新包类型：{}", staged.package_kind);
    }
    let cur = SemVer::parse(env!("CARGO_PKG_VERSION"))?;
    let target = SemVer::parse(&staged.version)?;
    if target <= cur {
        bail!(
            "安全拦截：拒绝安装低于或等于当前版本的更新包（目标 v{}，当前 v{}）",
            staged.version,
            env!("CARGO_PKG_VERSION")
        );
    }
    let p = PathBuf::from(&staged.path);
    let canon_root = fs::canonicalize(updates_root)
        .with_context(|| format!("更新目录不可用：{}", updates_root.display()))?;
    let meta = fs::symlink_metadata(&p)
        .with_context(|| format!("已下载的更新包不存在：{}", p.display()))?;
    if meta.file_type().is_symlink() {
        bail!("拒绝执行符号链接更新包");
    }
    let canon_file = fs::canonicalize(&p)
        .with_context(|| format!("无法定位更新包：{}", p.display()))?;
    if !canon_file.starts_with(&canon_root) {
        bail!("安全拦截：更新包路径不在受控更新缓存目录内");
    }
    Sha256Verifier.verify_file(&canon_file, &staged.sha256, staged.size)?;
    verify_binary_magic(&canon_file, &staged.package_kind)?;
    Ok(canon_file)
}

#[cfg(windows)]
fn spawn_installer_bootstrap(installer_path: &Path, silent: bool) -> Result<()> {
    let cur_pid = std::process::id().to_string();
    let cur_exe = std::env::current_exe()
        .context("无法获取当前程序路径")?
        .to_string_lossy()
        .to_string();
    let script = if silent {
        r#"
$ErrorActionPreference = 'SilentlyContinue'
$pidVal = [int]$env:_SF_UPD_PID
for ($i = 0; $i -lt 150; $i++) {
  if (-not (Get-Process -Id $pidVal -ErrorAction SilentlyContinue)) { break }
  Start-Sleep -Milliseconds 100
}
$p = Start-Process -FilePath $env:_SF_UPD_FILE -ArgumentList '/S' -PassThru -Wait
if ($p -and $p.ExitCode -eq 0 -and (Test-Path -LiteralPath $env:_SF_UPD_EXE)) {
  Start-Process -FilePath $env:_SF_UPD_EXE
}
"#
    } else {
        r#"
$ErrorActionPreference = 'SilentlyContinue'
$pidVal = [int]$env:_SF_UPD_PID
for ($i = 0; $i -lt 150; $i++) {
  if (-not (Get-Process -Id $pidVal -ErrorAction SilentlyContinue)) { break }
  Start-Sleep -Milliseconds 100
}
Start-Process -FilePath $env:_SF_UPD_FILE
"#
    };
    Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", script])
        .env("_SF_UPD_PID", cur_pid)
        .env("_SF_UPD_FILE", installer_path.to_string_lossy().to_string())
        .env("_SF_UPD_EXE", cur_exe)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("启动安装更新引导器失败")?;
    Ok(())
}

#[cfg(windows)]
fn spawn_portable_swap_bootstrap(
    payload_root: &Path,
    app_dir: &Path,
    backup_dir: &Path,
    stage_root: &Path,
) -> Result<()> {
    let cur_pid = std::process::id().to_string();
    let script = r#"
$ErrorActionPreference = 'Stop'
$pidVal = [int]$env:_SF_UPD_PID
$appDir = $env:_SF_UPD_APP
$srcDir = $env:_SF_UPD_SRC
$bakDir = $env:_SF_UPD_BAK
$stgDir = $env:_SF_UPD_STG

for ($i = 0; $i -lt 150; $i++) {
  if (-not (Get-Process -Id $pidVal -ErrorAction SilentlyContinue)) { break }
  Start-Sleep -Milliseconds 100
}
Start-Sleep -Milliseconds 300

try {
  if (Test-Path -LiteralPath $bakDir) { Remove-Item -LiteralPath $bakDir -Recurse -Force -ErrorAction SilentlyContinue }
  [void][System.IO.Directory]::CreateDirectory($bakDir)

  $oldExe = Join-Path $appDir 'shipforge.exe'
  if (Test-Path -LiteralPath $oldExe) {
    Copy-Item -LiteralPath $oldExe -Destination (Join-Path $bakDir 'shipforge.exe') -Force
  }
  $oldWv = Join-Path $appDir 'WebView2Loader.dll'
  if (Test-Path -LiteralPath $oldWv) {
    Copy-Item -LiteralPath $oldWv -Destination (Join-Path $bakDir 'WebView2Loader.dll') -Force -ErrorAction SilentlyContinue
  }

  $newExe = Join-Path $srcDir 'shipforge.exe'
  $copied = $false
  for ($try = 1; $try -le 10; $try++) {
    try {
      Copy-Item -LiteralPath $newExe -Destination $oldExe -Force
      $copied = $true
      break
    } catch {
      if ($try -eq 10) { throw }
      Start-Sleep -Milliseconds (250 * $try)
    }
  }
  if (-not $copied) { throw 'Failed to replace shipforge.exe' }

  foreach ($extra in @('WebView2Loader.dll', 'README.md', 'LICENSE')) {
    $srcExtra = Join-Path $srcDir $extra
    if (Test-Path -LiteralPath $srcExtra) {
      Copy-Item -LiteralPath $srcExtra -Destination (Join-Path $appDir $extra) -Force -ErrorAction SilentlyContinue
    }
  }
  Remove-Item -LiteralPath $bakDir -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $stgDir -Recurse -Force -ErrorAction SilentlyContinue
} catch {
  try {
    $bakExe = Join-Path $bakDir 'shipforge.exe'
    if (Test-Path -LiteralPath $bakExe) { Copy-Item -LiteralPath $bakExe -Destination (Join-Path $appDir 'shipforge.exe') -Force }
    $bakWv = Join-Path $bakDir 'WebView2Loader.dll'
    if (Test-Path -LiteralPath $bakWv) { Copy-Item -LiteralPath $bakWv -Destination (Join-Path $appDir 'WebView2Loader.dll') -Force -ErrorAction SilentlyContinue }
  } catch {}
} finally {
  $launch = Join-Path $appDir 'shipforge.exe'
  if (Test-Path -LiteralPath $launch) {
    Start-Process -FilePath $launch
  }
}
"#;
    Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", script])
        .env("_SF_UPD_PID", cur_pid)
        .env("_SF_UPD_APP", app_dir.to_string_lossy().to_string())
        .env("_SF_UPD_SRC", payload_root.to_string_lossy().to_string())
        .env("_SF_UPD_BAK", backup_dir.to_string_lossy().to_string())
        .env("_SF_UPD_STG", stage_root.to_string_lossy().to_string())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("启动便携版原子替换引导器失败")?;
    Ok(())
}

// ---------------------------------------------------------------- Multi-Channel Official Fetch

#[cfg(windows)]
enum AtomBetaFetch {
    Primary(ReleaseManifest),
    FallbackOnly(ReleaseManifest),
}

#[cfg(windows)]
fn fetch_beta_manifest_via_atom(
    atom_url: &str,
    download_base_prefix: &str,
) -> Result<AtomBetaFetch> {
    let (_status, bytes, _final_url) = winhttp_fetch_bytes(
        atom_url,
        MAX_MANIFEST_BYTES,
        9_000,
        "application/atom+xml, application/xml, text/xml",
    )?;
    let xml = String::from_utf8_lossy(&bytes);
    let tags = extract_release_tags_from_atom(&xml);
    if tags.is_empty() {
        bail!("官方发布订阅未包含有效版本标签");
    }

    let prefix = download_base_prefix.trim_end_matches('/');
    let mut last_err: Option<anyhow::Error> = None;
    for (idx, tag) in tags.into_iter().take(3).enumerate() {
        let direct_url = format!("{prefix}/{tag}/latest.json");
        match fetch_and_parse_manifest(&direct_url) {
            Ok(manifest) => {
                let tag_sv = SemVer::parse(tag.trim_start_matches(['v', 'V'])).ok();
                let mf_sv = SemVer::parse(&manifest.version).ok();
                if tag_sv.is_some() && mf_sv.is_some() && tag_sv != mf_sv {
                    last_err = Some(anyhow::anyhow!(
                        "标签 {tag} 下的 latest.json 版本号不匹配（实际为 {}）",
                        manifest.version
                    ));
                    continue;
                }
                return Ok(if idx == 0 {
                    AtomBetaFetch::Primary(manifest)
                } else {
                    AtomBetaFetch::FallbackOnly(manifest)
                });
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if is_dns_err(&msg) {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Atom 候选发布均缺少有效 latest.json")))
}

#[cfg(windows)]
fn fetch_official_manifest<F>(channel: &str, mut on_stage: F) -> Result<ReleaseManifest>
where
    F: FnMut(&str, u8),
{
    if let Some(local_feed) = dev_loopback_feed() {
        on_stage("正在连接本机调试更新源…", 40);
        return fetch_and_parse_manifest(&local_feed);
    }
    let mut errors: Vec<String> = Vec::new();
    let mut saw_dns_fail = false;

    if channel == "beta" {
        let mut atom_fallback: Option<ReleaseManifest> = None;
        on_stage("正在查询官方预发布订阅源 (1/3)…", 25);
        match fetch_beta_manifest_via_atom(
            OFFICIAL_RELEASES_ATOM_URL,
            "https://github.com/3347567186/ShipForge/releases/download",
        ) {
            Ok(AtomBetaFetch::Primary(m)) => return Ok(m),
            Ok(AtomBetaFetch::FallbackOnly(m)) => {
                atom_fallback = Some(m);
            }
            Err(e) => {
                let msg = format!("{e:#}");
                saw_dns_fail |= is_dns_err(&msg);
                errors.push(msg);
            }
        }
        if !saw_dns_fail {
            on_stage("正在切换 GitHub Releases API 重试 (2/3)…", 55);
            match fetch_and_parse_manifest(OFFICIAL_API_RELEASES_URL) {
                Ok(m) => {
                    if let Some(fb) = atom_fallback {
                        let m_sv = SemVer::parse(&m.version).ok();
                        let fb_sv = SemVer::parse(&fb.version).ok();
                        if fb_sv > m_sv {
                            return Ok(fb);
                        }
                    }
                    return Ok(m);
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    saw_dns_fail |= is_dns_err(&msg);
                    errors.push(msg);
                }
            }
        }
        if let Some(fb) = atom_fallback {
            return Ok(fb);
        }
        if !saw_dns_fail {
            on_stage("正在切换官方正式版清单兜底 (3/3)…", 80);
            match fetch_and_parse_manifest(OFFICIAL_FEED_URL) {
                Ok(m) => return Ok(m),
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
    } else {
        on_stage("正在连接官方更新清单 (1/3)…", 25);
        match fetch_and_parse_manifest(OFFICIAL_FEED_URL) {
            Ok(m) => return Ok(m),
            Err(e) => {
                let msg = format!("{e:#}");
                saw_dns_fail |= is_dns_err(&msg);
                errors.push(msg);
            }
        }
        if !saw_dns_fail {
            on_stage("主源响应异常，正在通过标签直链重试 (2/3)…", 55);
            match probe_latest_tag_via_redirect(8_000).and_then(|tag| {
                let direct_url = format!(
                    "https://github.com/3347567186/ShipForge/releases/download/{tag}/latest.json"
                );
                fetch_and_parse_manifest(&direct_url)
            }) {
                Ok(m) => return Ok(m),
                Err(e) => {
                    let msg = format!("{e:#}");
                    saw_dns_fail |= is_dns_err(&msg);
                    errors.push(msg);
                }
            }
        }
        if !saw_dns_fail {
            on_stage("正在切换 GitHub Releases API 备用通道 (3/3)…", 80);
            match fetch_and_parse_manifest(OFFICIAL_API_LATEST_URL) {
                Ok(m) => return Ok(m),
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
    }

    if saw_dns_fail {
        bail!("无法解析 GitHub 域名（DNS 失败），请检查网络连接或系统代理设置");
    }
    let summary = errors.into_iter().next().unwrap_or_else(|| "网络不可达".into());
    bail!("获取更新失败：{summary}（可检查网络/系统代理后重试，或前往 GitHub Releases 页面手动下载）")
}

// ---------------------------------------------------------------- Changelog (Offline + Optional Remote)

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangelogItem {
    pub version: String,
    pub title: String,
    pub date: String,
    pub body: String,
    pub is_current: bool,
    pub is_prerelease: bool,
    pub source: String,
    pub url: String,
}

pub fn parse_embedded_changelog(raw: &str, current_version: &str) -> Vec<ChangelogItem> {
    let cur = current_version.trim().trim_start_matches(['v', 'V']);
    let mut out = Vec::new();
    let mut cur_ver = String::new();
    let mut cur_date = String::new();
    let mut cur_title = String::new();
    let mut cur_lines: Vec<String> = Vec::new();
    let mut in_fence = false;

    let flush = |out: &mut Vec<ChangelogItem>,
                 ver: &mut String,
                 date: &mut String,
                 title: &mut String,
                 lines: &mut Vec<String>| {
        if ver.is_empty() {
            return;
        }
        let body = lines.join("\n").trim().to_string();
        let is_pre = SemVer::parse(ver).map(|s| s.is_prerelease()).unwrap_or(false);
        out.push(ChangelogItem {
            version: ver.clone(),
            title: if title.is_empty() {
                format!("v{}", ver)
            } else {
                title.clone()
            },
            date: date.clone(),
            body,
            is_current: ver == cur,
            is_prerelease: is_pre,
            source: "embedded".into(),
            url: format!("{OFFICIAL_RELEASES_PAGE}/tag/v{}", ver),
        });
        ver.clear();
        date.clear();
        title.clear();
        lines.clear();
    };

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence && trimmed.starts_with("## ") {
            let rest = trimmed[3..].trim();
            let rest_clean = rest.trim_start_matches(['[', 'v', 'V']);
            let ver_token = rest_clean
                .split(|c: char| c.is_whitespace() || c == ']' || c == '(')
                .next()
                .unwrap_or("")
                .trim();
            if SemVer::parse(ver_token).is_ok() {
                flush(&mut out, &mut cur_ver, &mut cur_date, &mut cur_title, &mut cur_lines);
                cur_ver = ver_token.to_string();
                if let Some(d_start) = rest.find('(') {
                    if let Some(d_end) = rest[d_start + 1..].find(')') {
                        cur_date = rest[d_start + 1..d_start + 1 + d_end].trim().to_string();
                    }
                }
                if let Some(idx) = rest.find("——").or_else(|| rest.find(" - ")) {
                    let sep_len = if rest[idx..].starts_with("——") { "——".len() } else { 3 };
                    cur_title = rest[idx + sep_len..].trim().to_string();
                }
                continue;
            }
        }
        if !cur_ver.is_empty() {
            cur_lines.push(line.to_string());
        }
    }
    flush(&mut out, &mut cur_ver, &mut cur_date, &mut cur_title, &mut cur_lines);
    out
}

// ---------------------------------------------------------------- Tauri Commands

#[tauri::command]
pub fn updater_status(
    state: State<'_, UpdaterState>,
    app_state: State<'_, AppState>,
) -> UpdaterStatusSnapshot {
    let dir = updates_dir();
    let staged = state
        .staged
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .filter(|s| Path::new(&s.path).is_file());
    UpdaterStatusSnapshot {
        current_version: env!("CARGO_PKG_VERSION").to_string(),
        install_kind: detect_install_kind().to_string(),
        busy: state.busy.load(Ordering::SeqCst),
        config: read_updater_cfg(&app_state),
        releases_page: OFFICIAL_RELEASES_PAGE.to_string(),
        updates_dir: dir.to_string_lossy().to_string(),
        cache_bytes: dir_size_bytes(&dir),
        staged,
    }
}

#[tauri::command]
pub fn updater_config_get(app_state: State<'_, AppState>) -> UpdaterConfig {
    read_updater_cfg(&app_state)
}

#[tauri::command]
pub fn updater_config_set(
    app_state: State<'_, AppState>,
    patch: Value,
) -> Result<UpdaterConfig, String> {
    update_updater_cfg(&app_state, &patch)
}

#[tauri::command]
pub async fn updater_check(
    app: AppHandle,
    state: State<'_, UpdaterState>,
    app_state: State<'_, AppState>,
    channel: Option<String>,
) -> Result<UpdateCheckResult, String> {
    let cfg = read_updater_cfg(&app_state);
    let ch = channel
        .map(|s| if s == "beta" { "beta".to_string() } else { "stable".to_string() })
        .unwrap_or(cfg.channel);
    let ignored = cfg.ignored_version;
    let staged = state.staged.lock().ok().and_then(|g| g.clone());
    let app2 = app.clone();

    let res = tauri::async_runtime::spawn_blocking(move || -> Result<UpdateCheckResult, String> {
        let emit_check = |stage: &str, percent: u8| {
            let _ = app2.emit(
                "updater-check-progress",
                json!({ "stage": stage, "percent": percent }),
            );
        };
        emit_check("正在准备检查更新…", 10);
        let manifest = fetch_official_manifest(&ch, |stage, pct| emit_check(stage, pct))
            .map_err(|e| format!("{e:#}"))?;
        emit_check("正在核验版本信息…", 92);
        let mut res = evaluate_manifest(
            env!("CARGO_PKG_VERSION"),
            detect_install_kind(),
            &ch,
            &ignored,
            &manifest,
            staged,
        )
        .map_err(|e| format!("{e:#}"))?;
        if res.has_update && res.staged.is_none() {
            let candidate = updates_dir()
                .join(format!("v{}", res.latest_version))
                .join(&res.file_name);
            if candidate.is_file() {
                if let Ok(verified) = Sha256Verifier.verify_file(&candidate, &res.sha256, res.size) {
                    if verify_binary_magic(&candidate, &res.package_kind).is_ok() {
                        let actual_size = fs::metadata(&candidate).map(|m| m.len()).unwrap_or(res.size);
                        res.staged = Some(StagedInfo {
                            version: res.latest_version.clone(),
                            package_kind: res.package_kind.clone(),
                            file_name: res.file_name.clone(),
                            path: candidate.to_string_lossy().to_string(),
                            size: actual_size,
                            sha256: verified,
                        });
                    }
                }
            }
        }
        Ok(res)
    })
    .await
    .map_err(|e| format!("检查更新任务失败：{e}"))??;

    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let _ = update_updater_cfg(&app_state, &json!({ "lastCheckAt": now }));

    if let Some(st_info) = res.staged.clone() {
        if let Ok(mut g) = state.staged.lock() {
            *g = Some(st_info);
        }
    }
    Ok(res)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn updater_download(
    app: AppHandle,
    state: State<'_, UpdaterState>,
    url: String,
    sha256: String,
    size: u64,
    version: String,
    file_name: String,
    package_kind: String,
) -> Result<StagedInfo, String> {
    let (_, dl_host, _, _) = validate_update_url(&url, false).map_err(|e| format!("{e:#}"))?;
    if is_local_host(&dl_host) && dev_loopback_feed().is_none() {
        return Err("安全拦截：未启用本机调试更新源环境变量时禁止从回环地址下载更新".into());
    }
    if state
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("当前已有更新下载正在进行中".into());
    }
    let _guard = BusyGuard(&state.busy);
    state.cancel.store(false, Ordering::SeqCst);
    let cancel = Arc::clone(&state.cancel);
    let app2 = app.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let root = updates_dir();
        let info = download_and_verify_to(
            &root,
            &url,
            &sha256,
            size,
            &version,
            &file_name,
            &package_kind,
            &cancel,
            Some(&app2),
        )
        .map_err(|e| format!("{e:#}"))?;
        let keep = Path::new(&info.path).parent().map(Path::to_path_buf);
        sweep_old_updates(keep.as_deref());
        Ok::<StagedInfo, String>(info)
    })
    .await
    .map_err(|e| format!("下载更新任务失败：{e}"))?;

    let info = res?;
    if let Ok(mut g) = state.staged.lock() {
        *g = Some(info.clone());
    }
    Ok(info)
}

#[tauri::command]
pub fn updater_cancel(state: State<'_, UpdaterState>) -> bool {
    state.cancel.store(true, Ordering::SeqCst);
    true
}

#[tauri::command]
pub fn updater_clear_cache(state: State<'_, UpdaterState>) -> Result<bool, String> {
    if state.busy.load(Ordering::SeqCst) {
        return Err("更新下载进行中，请先取消后再清理缓存".into());
    }
    if let Ok(mut g) = state.staged.lock() {
        *g = None;
    }
    sweep_old_updates(None);
    Ok(true)
}

#[tauri::command]
pub async fn updater_install(
    app: AppHandle,
    state: State<'_, UpdaterState>,
    app_state: State<'_, AppState>,
    silent: Option<bool>,
) -> Result<bool, String> {
    if app_state.runner.has_any_running() {
        return Err("当前有构建或发布任务正在运行，请等待完成或停止任务后再安装更新".into());
    }
    if state
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("更新任务正在进行中，请稍候".into());
    }
    let _guard = BusyGuard(&state.busy);
    let staged = state
        .staged
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .ok_or_else(|| "尚未下载已校验的更新包，请先下载更新".to_string())?;

    let staged_clone = staged.clone();
    let silent_flag = silent.unwrap_or(false);
    let prepared = tauri::async_runtime::spawn_blocking(move || -> Result<PathBuf, String> {
        let root = updates_dir();
        let file = validate_staged_before_install(&root, &staged_clone)
            .map_err(|e| format!("{e:#}"))?;
        if staged_clone.package_kind == "portable" {
            let probe = exe_dir().join(format!(".sf_upd_probe_{}.tmp", std::process::id()));
            if let Err(e) = fs::write(&probe, b"ok") {
                return Err(format!(
                    "程序目录无写入权限（{e}），请先以管理员身份重启后再安装便携更新"
                ));
            }
            let _ = fs::remove_file(&probe);

            let stage_dir = root.join(format!("stage_{}", staged_clone.version));
            if let Ok(sm) = fs::symlink_metadata(&stage_dir) {
                if sm.file_type().is_symlink() {
                    return Err("安全拦截：更新暂存目录为异常符号链接".into());
                }
                let _ = fs::remove_dir_all(&stage_dir);
            }
            if let Err(e) = extract_portable_zip_safe(&file, &stage_dir) {
                let _ = fs::remove_dir_all(&stage_dir);
                return Err(format!("解压便携更新包失败：{e:#}"));
            }
            let payload = match find_portable_payload_root(&stage_dir) {
                Ok(p) => p,
                Err(e) => {
                    let _ = fs::remove_dir_all(&stage_dir);
                    return Err(format!("{e:#}"));
                }
            };
            Ok(payload)
        } else {
            Ok(file)
        }
    })
    .await
    .map_err(|e| format!("准备安装失败：{e}"))??;

    // 退出前再次复核活动任务，防止解压期间启动的构建/发版事务被中断
    if app_state.runner.has_any_running() {
        return Err("当前有构建或发布任务正在运行，请等待完成或停止任务后再安装更新".into());
    }

    // 停止本机测试更新源，释放端口与文件锁
    if let Ok(mut srv) = app_state.server.lock() {
        srv.stop();
    }

    if staged.package_kind == "portable" {
        let root = updates_dir();
        let stage_dir = root.join(format!("stage_{}", staged.version));
        let backup_dir = root.join(format!("backup_{}", staged.version));
        spawn_portable_swap_bootstrap(&prepared, &exe_dir(), &backup_dir, &stage_dir)
            .map_err(|e| format!("{e:#}"))?;
    } else {
        spawn_installer_bootstrap(&prepared, silent_flag).map_err(|e| format!("{e:#}"))?;
    }

    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(260));
        handle.exit(0);
    });
    Ok(true)
}

#[tauri::command]
pub async fn updater_changelog(include_remote: Option<bool>) -> Result<Value, String> {
    let want_remote = include_remote.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let cur = env!("CARGO_PKG_VERSION");
        let mut items = parse_embedded_changelog(EMBEDDED_CHANGELOG_MD, cur);
        let mut remote_synced = false;
        let mut remote_error: Option<String> = None;

        if want_remote {
            match winhttp_fetch_bytes(
                OFFICIAL_API_CHANGELOG_URL,
                MAX_MANIFEST_BYTES,
                9_000,
                "application/vnd.github+json, application/json",
            ) {
                Ok((_, bytes, _)) => {
                    if let Ok(Value::Array(arr)) = serde_json::from_slice::<Value>(&bytes) {
                        for rel in arr {
                            if rel.get("draft").and_then(|d| d.as_bool()).unwrap_or(false) {
                                continue;
                            }
                            let tag = rel
                                .get("tag_name")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                                .trim();
                            let ver = tag.trim_start_matches(['v', 'V']).to_string();
                            if SemVer::parse(&ver).is_err() {
                                continue;
                            }
                            if items.iter().any(|x| x.version == ver) {
                                continue;
                            }
                            let title = rel
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or(tag)
                                .trim()
                                .to_string();
                            let date = rel
                                .get("published_at")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .chars()
                                .take(10)
                                .collect::<String>();
                            let body = rel
                                .get("body")
                                .and_then(|b| b.as_str())
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            let url = rel
                                .get("html_url")
                                .and_then(|u| u.as_str())
                                .filter(|u| u.starts_with("https://github.com/3347567186/ShipForge/"))
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("{OFFICIAL_RELEASES_PAGE}/tag/{tag}"));
                            let is_pre = rel
                                .get("prerelease")
                                .and_then(|p| p.as_bool())
                                .unwrap_or(false);
                            items.push(ChangelogItem {
                                version: ver.clone(),
                                title,
                                date,
                                body,
                                is_current: ver == cur,
                                is_prerelease: is_pre,
                                source: "remote".into(),
                                url,
                            });
                        }
                        items.sort_by(|a, b| {
                            let sa = SemVer::parse(&a.version).ok();
                            let sb = SemVer::parse(&b.version).ok();
                            sb.cmp(&sa)
                        });
                        remote_synced = true;
                    }
                }
                Err(e) => {
                    remote_error = Some(format!("{e:#}"));
                }
            }
        }

        Ok(json!({
            "currentVersion": cur,
            "items": items,
            "remoteSynced": remote_synced,
            "remoteError": remote_error,
        }))
    })
    .await
    .map_err(|e| format!("读取更新日志失败：{e}"))?
}

// ---------------------------------------------------------------- Unit Tests

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    #[test]
    fn test_semver_ordering_and_prerelease() {
        let a = SemVer::parse("1.3.17").unwrap();
        let b = SemVer::parse("v1.4.0-beta.2").unwrap();
        let c = SemVer::parse("1.4.0-beta.10").unwrap();
        let d = SemVer::parse("1.4.0").unwrap();
        assert!(a < b);
        assert!(b < c);
        assert!(c < d);
        assert!(SemVer::parse("1.2").is_err());
        assert!(SemVer::parse("1.2.3-").is_err());
    }

    #[test]
    fn test_validate_update_url_enforces_shipforge_repo_scope() {
        assert!(validate_update_url(OFFICIAL_FEED_URL, false).is_ok());
        assert!(validate_update_url(OFFICIAL_API_LATEST_URL, false).is_ok());
        assert!(validate_update_url(
            "https://objects.githubusercontent.com/github-production-release-asset/foo",
            true
        )
        .is_ok());
        // Must block QDisk repo or third-party repos
        assert!(validate_update_url(
            "https://github.com/3347567186/QDisk/releases/latest/download/latest.json",
            false
        )
        .is_err());
        assert!(validate_update_url(
            "https://github.com/evil/ShipForge/releases/latest/download/latest.json",
            false
        )
        .is_err());
        assert!(validate_update_url(
            "https://github.com/3347567186/ShipForge/../QDisk/releases/latest",
            false
        )
        .is_err());
        assert!(validate_update_url(
            "https://github.com/3347567186/ShipForge/%2e./QDisk/releases/latest",
            false
        )
        .is_err());
        assert!(validate_update_url(
            "https://github.com/3347567186/ShipForge/releases%2f..%2fQDisk",
            false
        )
        .is_err());
        // Must block HTTP remote & HTTPS->loopback redirect downgrade
        assert!(validate_update_url(
            "http://github.com/3347567186/ShipForge/releases/latest/download/latest.json",
            false
        )
        .is_err());
        assert!(validate_update_url("http://127.0.0.1:18765/latest.json", false).is_ok());
        assert!(validate_update_url("http://127.0.0.1:18765/latest.json", true).is_err());
    }

    #[test]
    fn test_atom_tag_extraction_blocks_injected_links_and_foreign_repos() {
        let xml = r#"<feed>
            <entry>
                <link rel="alternate" href="https://github.com/3347567186/ShipForge/releases/tag/v1.4.0"/>
                <content type="html"><link href="https://github.com/3347567186/ShipForge/releases/tag/v9.9.9"/></content>
            </entry>
            <entry>
                <link rel="alternate" href="https://github.com/3347567186/QDisk/releases/tag/v9.0.0"/>
            </entry>
            <entry>
                <link rel="alternate" href="/3347567186/ShipForge/releases/tag/v1.5.0-beta.1"/>
            </entry>
        </feed>"#;
        let tags = extract_release_tags_from_atom(xml);
        assert_eq!(tags, vec!["v1.5.0-beta.1".to_string(), "v1.4.0".to_string()]);
    }

    #[test]
    fn test_extract_portable_zip_blocks_zip_slip_and_validates_payload() {
        let tmp = std::env::temp_dir().join(format!("sf_upd_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // 1. Zip Slip zip must be blocked
        let bad_zip = tmp.join("slip.zip");
        {
            let f = File::create(&bad_zip).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            zw.start_file("../evil.txt", SimpleFileOptions::default()).unwrap();
            zw.write_all(b"evil").unwrap();
            zw.finish().unwrap();
        }
        assert!(extract_portable_zip_safe(&bad_zip, &tmp.join("out_bad")).is_err());

        // 2. Valid portable zip succeeds and resolves payload root
        let good_zip = tmp.join("good.zip");
        {
            let f = File::create(&good_zip).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            zw.start_file("shipforge.exe", SimpleFileOptions::default()).unwrap();
            zw.write_all(b"MZ\x90\x00dummy-pe").unwrap();
            zw.finish().unwrap();
        }
        let out_good = tmp.join("out_good");
        extract_portable_zip_safe(&good_zip, &out_good).unwrap();
        let root = find_portable_payload_root(&out_good).unwrap();
        assert!(root.join("shipforge.exe").is_file());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_validate_staged_blocks_downgrade_and_outside_path() {
        let tmp = std::env::temp_dir().join(format!("sf_upd_stage_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let upd_root = tmp.join("updates");
        fs::create_dir_all(&upd_root).unwrap();
        let pkg = upd_root.join("pkg.exe");
        fs::write(&pkg, b"MZ\x90\x00hello").unwrap();
        let sha = hex_lower(&Sha256::digest(b"MZ\x90\x00hello"));

        let downgrade = StagedInfo {
            version: "0.1.0".into(),
            package_kind: "installer".into(),
            file_name: "pkg.exe".into(),
            path: pkg.to_string_lossy().to_string(),
            size: 9,
            sha256: sha.clone(),
        };
        assert!(validate_staged_before_install(&upd_root, &downgrade).is_err());

        let outside_file = tmp.join("outside.exe");
        fs::write(&outside_file, b"MZ\x90\x00hello").unwrap();
        let escape = StagedInfo {
            version: "99.0.0".into(),
            package_kind: "installer".into(),
            file_name: "outside.exe".into(),
            path: outside_file.to_string_lossy().to_string(),
            size: 9,
            sha256: sha,
        };
        assert!(validate_staged_before_install(&upd_root, &escape).is_err());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_embedded_changelog_parses_entries() {
        let items = parse_embedded_changelog(EMBEDDED_CHANGELOG_MD, env!("CARGO_PKG_VERSION"));
        assert!(!items.is_empty(), "Embedded CHANGELOG.md must contain at least one release entry");
        assert!(SemVer::parse(&items[0].version).is_ok());
    }
}
