use crate::credentials::{default_base_url, normalize_provider, resolve_token, scrub_secret};
use crate::git_ops::{git_delete_tag, validate_git_tag_name};
use crate::guardrails::NET_RETRY_BACKOFF_MS;
use crate::models::{
    DeleteReleaseAssetRequest, DeleteReleaseRequest, DeleteReleaseResult, GhReleaseItem,
    MultiPublishRequest, ReleaseAssetItem, RemoteRepoItem, UpdateReleaseRequest,
};
use crate::packager::{encode_uri_component, validate_manifest_for_publish};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ParsedUrl {
    pub https: bool,
    pub host: String,
    pub port: u16,
    pub path_and_query: String,
}

/// Strictly validate `owner/repo` (or GitLab `group/subgroup/project`) against path traversal or injection.
pub fn validate_repo_slug(repo: &str) -> Result<String> {
    let clean = repo.trim().trim_matches('/');
    if !clean.contains('/') || clean.contains("..") || clean.contains("//") {
        return Err(anyhow!("仓库名称格式无效（应为 owner/repo）：{}", clean));
    }
    if !clean
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
    {
        return Err(anyhow!("仓库名称包含非法字符：{}", clean));
    }
    Ok(clean.to_string())
}

/// Strict URL parser supporting https:// and loopback http:// (127.0.0.1 / localhost) for local testing or LAN self-hosted GitLab/Gitea.
pub fn parse_http_url(raw: &str) -> Result<ParsedUrl> {
    let s = raw.trim();
    let (https, rest) = if let Some(r) = s.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = s.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(anyhow!("无效的仓库服务地址（需以 https:// 或 http:// 开头）：{}", s));
    };
    if rest.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(anyhow!("仓库服务地址包含非法空白或控制字符"));
    }
    let (authority, path_and_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], rest[idx..].to_string()),
        None => (rest, "/".to_string()),
    };
    if authority.is_empty() || authority.contains('@') {
        return Err(anyhow!("仓库服务地址格式无效（不允许内嵌凭据）"));
    }
    let (host, port) = if let Some((h, p)) = authority.rsplit_once(':') {
        let parsed_port: u16 = p
            .parse()
            .map_err(|_| anyhow!("仓库服务端口号无效：{}", p))?;
        (h.to_string(), parsed_port)
    } else {
        (authority.to_string(), if https { 443 } else { 80 })
    };
    if host.is_empty() {
        return Err(anyhow!("仓库服务主机名不能为空"));
    }
    Ok(ParsedUrl {
        https,
        host,
        port,
        path_and_query,
    })
}

pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
    pub fn json(&self) -> Result<Value> {
        serde_json::from_slice(&self.body).map_err(|e| anyhow!("解析接口 JSON 失败：{}", e))
    }
}

#[cfg(windows)]
fn to_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
struct WinHttpHandle(*mut core::ffi::c_void);

#[cfg(windows)]
impl Drop for WinHttpHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                windows_sys::Win32::Networking::WinHttp::WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// Native Windows WinHTTP streaming request supporting:
/// - System proxy / TLS 1.2+ / Windows Root Certificate Store
/// - Fixed-size 64 KiB streaming body upload with byte-level progress callback
/// - Zero OpenSSL or external CLI dependency
#[cfg(windows)]
pub fn winhttp_request<F>(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body_reader: Option<(&mut dyn Read, u64)>,
    mut on_progress: F,
) -> Result<HttpResponse>
where
    F: FnMut(u64, u64),
{
    use windows_sys::Win32::Networking::WinHttp::*;

    let parsed = parse_http_url(url)?;
    let ua = to_wide("ShipForge/2.0 (Windows; WinHTTP)");
    let session = WinHttpHandle(unsafe {
        WinHttpOpen(
            ua.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    });
    if session.0.is_null() {
        return Err(anyhow!("初始化 WinHTTP 会话失败"));
    }

    // Generous safe timeouts: resolve 15s, connect 30s, send 180s, receive 180s (for 25MB+ Release uploads)
    // Also explicitly enable TLS 1.2 + 1.3 (fallback to TLS 1.2 on older Windows)
    unsafe {
        WinHttpSetTimeouts(session.0, 15_000, 30_000, 180_000, 180_000);
        const TLS12: u32 = 0x0000_0800;
        const TLS13: u32 = 0x0000_2000;
        let mut protos: u32 = TLS12 | TLS13;
        if WinHttpSetOption(
            session.0,
            WINHTTP_OPTION_SECURE_PROTOCOLS,
            &mut protos as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        ) == 0
        {
            let mut tls12: u32 = TLS12;
            let _ = WinHttpSetOption(
                session.0,
                WINHTTP_OPTION_SECURE_PROTOCOLS,
                &mut tls12 as *mut u32 as *mut core::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            );
        }
    }

    let host_w = to_wide(&parsed.host);
    let conn = WinHttpHandle(unsafe { WinHttpConnect(session.0, host_w.as_ptr(), parsed.port, 0) });
    if conn.0.is_null() {
        return Err(anyhow!("连接远端主机 {} 失败", parsed.host));
    }

    let method_w = to_wide(method);
    let path_w = to_wide(&parsed.path_and_query);
    let flags = if parsed.https { WINHTTP_FLAG_SECURE } else { 0 };
    let req = WinHttpHandle(unsafe {
        WinHttpOpenRequest(
            conn.0,
            method_w.as_ptr(),
            path_w.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            flags,
        )
    });
    if req.0.is_null() {
        return Err(anyhow!("创建 HTTP 请求失败：{}", parsed.path_and_query));
    }

    let mut hdr_block = String::new();
    for (k, v) in headers {
        if k.chars().any(|c| c == '\r' || c == '\n' || c == '\0')
            || v.chars().any(|c| c == '\r' || c == '\n' || c == '\0')
        {
            return Err(anyhow!("HTTP 请求头包含非法换行或控制字符，已拦截"));
        }
        hdr_block.push_str(k);
        hdr_block.push_str(": ");
        hdr_block.push_str(v);
        hdr_block.push_str("\r\n");
    }
    let hdr_w = to_wide(&hdr_block);
    let hdr_ptr = if hdr_block.is_empty() {
        std::ptr::null()
    } else {
        hdr_w.as_ptr()
    };
    let hdr_len = if hdr_block.is_empty() {
        0
    } else {
        (hdr_w.len() - 1) as u32
    };

    let total_len = body_reader.as_ref().map(|(_, len)| *len).unwrap_or(0);
    let total_len_u32 = u32::try_from(total_len)
        .map_err(|_| anyhow!("单个上传流超过 4 GB WinHTTP 限制，请启用分卷打包"))?;

    use windows_sys::Win32::Foundation::GetLastError;

    let sent = unsafe {
        WinHttpSendRequest(
            req.0,
            hdr_ptr,
            hdr_len,
            std::ptr::null(),
            0,
            total_len_u32,
            0,
        )
    };
    if sent == 0 {
        let code = unsafe { GetLastError() };
        return Err(anyhow!(
            "发送 HTTP 请求失败（请检查网络连接或代理设置，WinHTTP 错误码 {}）",
            code
        ));
    }

    if let Some((reader, total)) = body_reader {
        let mut buf = vec![0u8; 256 * 1024];
        let mut written_total: u64 = 0;
        while written_total < total {
            let max_read = ((total - written_total) as usize).min(buf.len());
            let n = reader.read(&mut buf[..max_read])?;
            if n == 0 {
                break;
            }
            let mut offset = 0usize;
            while offset < n {
                let mut written: u32 = 0;
                let ok = unsafe {
                    WinHttpWriteData(
                        req.0,
                        buf[offset..n].as_ptr() as *const core::ffi::c_void,
                        (n - offset) as u32,
                        &mut written,
                    )
                };
                if ok == 0 {
                    let code = unsafe { GetLastError() };
                    return Err(anyhow!(
                        "上传数据流中断（已发送 {}/{} B，WinHTTP 错误码 {}）",
                        written_total,
                        total,
                        code
                    ));
                }
                if written == 0 {
                    return Err(anyhow!(
                        "上传数据流异常停滞（已发送 {}/{} B，连续写入 0 字节）",
                        written_total,
                        total
                    ));
                }
                offset += written as usize;
                written_total += written as u64;
                on_progress(written_total, total);
            }
        }
        if written_total != total {
            return Err(anyhow!(
                "待上传文件在传输期间发生变动或读取提前结束（期望 {} B，实际发送 {} B），已中止上传",
                total,
                written_total
            ));
        }
    }

    let recv = unsafe { WinHttpReceiveResponse(req.0, std::ptr::null_mut()) };
    if recv == 0 {
        let code = unsafe { GetLastError() };
        return Err(anyhow!(
            "等待远端服务器响应超时或连接中断（WinHTTP 错误码 {}）",
            code
        ));
    }

    let mut status_code: u32 = 0;
    let mut status_size: u32 = std::mem::size_of::<u32>() as u32;
    unsafe {
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            &mut status_code as *mut u32 as *mut core::ffi::c_void,
            &mut status_size,
            std::ptr::null_mut(),
        );
    }

    let mut body = Vec::new();
    const MAX_RESP_BYTES: usize = 8 * 1024 * 1024;
    loop {
        let mut avail: u32 = 0;
        if unsafe { WinHttpQueryDataAvailable(req.0, &mut avail) } == 0 {
            let code = unsafe { GetLastError() };
            return Err(anyhow!("读取远端响应失败（WinHTTP 错误码 {}）", code));
        }
        if avail == 0 {
            break;
        }
        let chunk_len = (avail as usize).min(64 * 1024);
        let mut chunk = vec![0u8; chunk_len];
        let mut read_len: u32 = 0;
        if unsafe {
            WinHttpReadData(
                req.0,
                chunk.as_mut_ptr() as *mut core::ffi::c_void,
                chunk_len as u32,
                &mut read_len,
            )
        } == 0
        {
            let code = unsafe { GetLastError() };
            return Err(anyhow!("读取远端响应流中断（WinHTTP 错误码 {}）", code));
        }
        if read_len == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read_len as usize]);
        if body.len() > MAX_RESP_BYTES {
            return Err(anyhow!(
                "远端 API 响应体积超过安全上限（{} MB），已中止读取",
                MAX_RESP_BYTES / (1024 * 1024)
            ));
        }
    }

    Ok(HttpResponse {
        status: status_code as u16,
        body,
    })
}

#[cfg(not(windows))]
pub fn winhttp_request<F>(
    _method: &str,
    _url: &str,
    _headers: &[(&str, String)],
    _body_reader: Option<(&mut dyn Read, u64)>,
    _on_progress: F,
) -> Result<HttpResponse>
where
    F: FnMut(u64, u64),
{
    Err(anyhow!("WinHTTP 仅支持 Windows 平台"))
}

fn http_json_with_retry(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    json_body: Option<&Value>,
    secret_to_scrub: &str,
) -> Result<HttpResponse> {
    let bytes_opt = json_body.map(|v| serde_json::to_vec(v).unwrap_or_default());
    let mut last_err = None;
    for attempt in 0..=NET_RETRY_BACKOFF_MS.len() {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(NET_RETRY_BACKOFF_MS[attempt - 1]));
        }
        let mut slice_ref = bytes_opt.as_deref();
        let reader_arg = slice_ref.as_mut().map(|s| {
            let len = s.len() as u64;
            (s as &mut dyn Read, len)
        });
        match winhttp_request(method, url, headers, reader_arg, |_, _| {}) {
            Ok(resp) => {
                // Retry only on transient 5xx / 429
                if (resp.status >= 500 || resp.status == 429) && attempt < NET_RETRY_BACKOFF_MS.len()
                {
                    last_err = Some(anyhow!("远端暂不可用 (HTTP {})", resp.status));
                    continue;
                }
                return Ok(resp);
            }
            Err(e) => {
                last_err = Some(anyhow!("{}", scrub_secret(&e.to_string(), secret_to_scrub)));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("网络请求失败")))
}

fn provider_api_root(provider: &str, base_url: &str) -> Result<String> {
    let prov = normalize_provider(provider)?;
    let custom = base_url.trim().trim_end_matches('/');
    let root = if custom.is_empty() {
        default_base_url(&prov).to_string()
    } else {
        let _ = parse_http_url(custom)?;
        custom.to_string()
    };
    Ok(root)
}

fn auth_headers(provider: &str, token: &str) -> Vec<(&'static str, String)> {
    match provider {
        "github" => vec![
            ("Authorization", format!("Bearer {}", token)),
            ("Accept", "application/vnd.github+json".into()),
            ("X-GitHub-Api-Version", "2022-11-28".into()),
        ],
        "gitee" => vec![
            ("Authorization", format!("token {}", token)),
            ("Accept", "application/json".into()),
        ],
        "gitlab" => vec![
            ("PRIVATE-TOKEN", token.to_string()),
            ("Accept", "application/json".into()),
        ],
        "gitea" => vec![
            ("Authorization", format!("token {}", token)),
            ("Accept", "application/json".into()),
        ],
        _ => vec![("Authorization", format!("Bearer {}", token))],
    }
}

/// Verify a token against the target platform and return the authenticated username.
pub fn verify_token(provider: &str, base_url: &str, token: &str) -> Result<String> {
    let prov = normalize_provider(provider)?;
    let root = provider_api_root(&prov, base_url)?;
    let url = match prov.as_str() {
        "github" => format!("{}/user", root),
        "gitee" => format!("{}/api/v5/user?access_token={}", root, encode_uri_component(token)),
        "gitlab" => format!("{}/api/v4/user", root),
        "gitea" => format!("{}/api/v1/user", root),
        _ => unreachable!(),
    };
    let headers = auth_headers(&prov, token);
    let resp = http_json_with_retry("GET", &url, &headers, None, token)?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(anyhow!(
            "令牌验证失败 (HTTP {})：请确认令牌未过期且具备仓库写权限",
            resp.status
        ));
    }
    let val = resp.json()?;
    let username = val
        .get("login")
        .or_else(|| val.get("username"))
        .or_else(|| val.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("authenticated")
        .to_string();
    Ok(username)
}

/// List repositories the authenticated user can push/publish to (for one-click dropdown binding).
pub fn list_user_repos(
    provider: &str,
    base_url: &str,
    credential_id: &str,
) -> Result<Vec<RemoteRepoItem>> {
    let prov = normalize_provider(provider)?;
    let (token, _) = resolve_token(&prov, base_url, credential_id)?;
    let root = provider_api_root(&prov, base_url)?;
    let url = match prov.as_str() {
        "github" => format!("{}/user/repos?sort=updated&per_page=50&affiliation=owner,collaborator,organization_member", root),
        "gitee" => format!(
            "{}/api/v5/user/repos?sort=updated&per_page=50&access_token={}",
            root,
            encode_uri_component(&token)
        ),
        "gitlab" => format!("{}/api/v4/projects?membership=true&min_access_level=30&order_by=updated_at&per_page=50", root),
        "gitea" => format!("{}/api/v1/user/repos?limit=50", root),
        _ => unreachable!(),
    };
    let headers = auth_headers(&prov, &token);
    let resp = http_json_with_retry("GET", &url, &headers, None, &token)?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(anyhow!("拉取仓库列表失败 (HTTP {})", resp.status));
    }
    let arr = resp
        .json()?
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mut out = Vec::new();
    for item in arr {
        let full_name = item
            .get("full_name")
            .or_else(|| item.get("path_with_namespace"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if full_name.is_empty() {
            continue;
        }
        let html_url = item
            .get("html_url")
            .or_else(|| item.get("web_url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let clone_url = item
            .get("clone_url")
            .or_else(|| item.get("http_url_to_repo"))
            .and_then(|v| v.as_str())
            .unwrap_or(&html_url)
            .to_string();
        let private = item
            .get("private")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| {
                item.get("visibility")
                    .and_then(|v| v.as_str())
                    .map(|s| s == "private")
                    .unwrap_or(false)
            });
        let default_branch = item
            .get("default_branch")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .to_string();
        let description = item
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(RemoteRepoItem {
            full_name,
            html_url,
            clone_url,
            private,
            default_branch,
            description,
        });
    }
    Ok(out)
}

fn json_id_to_string(v: Option<&Value>) -> String {
    match v {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.trim().to_string(),
        _ => String::new(),
    }
}

fn validate_safe_id(raw: &str, label: &str) -> Result<String> {
    let clean = raw.trim();
    if clean.is_empty()
        || !clean
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(anyhow!("无效的 {}：{}", label, clean));
    }
    Ok(clean.to_string())
}

fn parse_release_item(prov: &str, item: &Value) -> Option<GhReleaseItem> {
    let tag_name = item
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if tag_name.is_empty() {
        return None;
    }
    let id = {
        let raw_id = json_id_to_string(item.get("id"));
        if raw_id.is_empty() && prov == "gitlab" {
            tag_name.clone()
        } else {
            raw_id
        }
    };
    let name = item
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&tag_name)
        .to_string();
    let body = item
        .get("body")
        .or_else(|| item.get("description"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let published_at = item
        .get("published_at")
        .or_else(|| item.get("released_at"))
        .or_else(|| item.get("created_at"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let url = item
        .get("html_url")
        .or_else(|| item.get("_links").and_then(|l| l.get("self")))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut assets = Vec::new();
    if prov == "gitlab" {
        if let Some(links) = item
            .get("assets")
            .and_then(|a| a.get("links"))
            .and_then(|l| l.as_array())
        {
            for l in links {
                let aname = l.get("name").and_then(|x| x.as_str()).unwrap_or("").trim();
                if aname.is_empty() {
                    continue;
                }
                let aid = json_id_to_string(l.get("id"));
                let dl = l
                    .get("direct_asset_url")
                    .or_else(|| l.get("url"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                assets.push(ReleaseAssetItem {
                    id: aid,
                    name: aname.to_string(),
                    size: 0,
                    download_url: dl,
                });
            }
        }
    } else if let Some(arr) = item.get("assets").and_then(|a| a.as_array()) {
        for a in arr {
            let aname = a.get("name").and_then(|x| x.as_str()).unwrap_or("").trim();
            if aname.is_empty() {
                continue;
            }
            let aid = json_id_to_string(a.get("id"));
            // Gitee includes synthetic source zipball/tarball items without numeric id; skip them as non-uploaded assets
            if prov == "gitee" && aid.is_empty() {
                continue;
            }
            let size = a.get("size").and_then(|x| x.as_u64()).unwrap_or(0);
            let dl = a
                .get("browser_download_url")
                .or_else(|| a.get("url"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            assets.push(ReleaseAssetItem {
                id: aid,
                name: aname.to_string(),
                size,
                download_url: dl,
            });
        }
    }

    Some(GhReleaseItem {
        id,
        tag_name,
        name,
        body,
        is_prerelease: item
            .get("prerelease")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        is_draft: item.get("draft").and_then(|v| v.as_bool()).unwrap_or(false),
        published_at,
        url,
        assets,
    })
}

fn resolve_release_id_if_missing(
    prov: &str,
    root: &str,
    token: &str,
    clean_repo: &str,
    tag_name: &str,
    release_id: &str,
) -> Result<String> {
    if prov == "gitlab" {
        let t = tag_name.trim();
        if t.is_empty() {
            return Err(anyhow!("GitLab Release 缺少 tag_name"));
        }
        return Ok(t.to_string());
    }
    if !release_id.trim().is_empty() {
        return validate_safe_id(release_id, "Release ID");
    }
    let tag = tag_name.trim();
    if tag.is_empty() {
        return Err(anyhow!("缺少 Release ID 与 Tag"));
    }
    let url = match prov {
        "github" => format!("{}/repos/{}/releases/tags/{}", root, clean_repo, encode_uri_component(tag)),
        "gitee" => format!(
            "{}/api/v5/repos/{}/releases/tags/{}?access_token={}",
            root,
            clean_repo,
            encode_uri_component(tag),
            encode_uri_component(token)
        ),
        "gitea" => format!(
            "{}/api/v1/repos/{}/releases/tags/{}",
            root,
            clean_repo,
            encode_uri_component(tag)
        ),
        _ => return Err(anyhow!("暂不支持的平台：{}", prov)),
    };
    let headers = auth_headers(prov, token);
    let resp = http_json_with_retry("GET", &url, &headers, None, token)?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(anyhow!("无法查询 Release {} (HTTP {})", tag, resp.status));
    }
    let json = resp.json()?;
    let resolved = json_id_to_string(json.get("id"));
    validate_safe_id(&resolved, "Release ID")
}

/// List recent releases for a bound repository across all 4 supported platforms.
pub fn list_releases_multi(
    provider: &str,
    base_url: &str,
    credential_id: &str,
    repo: &str,
) -> Result<Vec<GhReleaseItem>> {
    let prov = normalize_provider(provider)?;
    let (token, _) = resolve_token(&prov, base_url, credential_id)?;
    list_releases_multi_with_token(&prov, base_url, repo, &token)
}

fn list_releases_multi_with_token(
    prov: &str,
    base_url: &str,
    repo: &str,
    token: &str,
) -> Result<Vec<GhReleaseItem>> {
    let clean_repo = validate_repo_slug(repo)?;
    let root = provider_api_root(prov, base_url)?;
    let url = match prov {
        "github" => format!("{}/repos/{}/releases?per_page=25", root, clean_repo),
        "gitee" => format!(
            "{}/api/v5/repos/{}/releases?per_page=25&access_token={}",
            root,
            clean_repo,
            encode_uri_component(token)
        ),
        "gitlab" => format!(
            "{}/api/v4/projects/{}/releases?per_page=25",
            root,
            encode_uri_component(&clean_repo)
        ),
        "gitea" => format!("{}/api/v1/repos/{}/releases?limit=25", root, clean_repo),
        _ => return Err(anyhow!("暂不支持的仓库平台：{}", prov)),
    };
    let headers = auth_headers(prov, token);
    let resp = http_json_with_retry("GET", &url, &headers, None, token)?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(anyhow!("读取线上 Releases 失败 (HTTP {})", resp.status));
    }
    let arr = resp.json()?.as_array().cloned().unwrap_or_default();
    let list = arr
        .iter()
        .filter_map(|item| parse_release_item(prov, item))
        .collect();
    Ok(list)
}

/// Update an existing online Release's title, Markdown notes, prerelease flag, or draft (unpublish/republish) status.
pub fn update_release_multi(req: &UpdateReleaseRequest) -> Result<GhReleaseItem> {
    let prov = normalize_provider(&req.provider)?;
    let (token, _) = resolve_token(&prov, &req.base_url, &req.credential_id)?;
    update_release_multi_with_token(&prov, req, &token)
}

fn update_release_multi_with_token(
    prov: &str,
    req: &UpdateReleaseRequest,
    token: &str,
) -> Result<GhReleaseItem> {
    let clean_repo = validate_repo_slug(&req.repo)?;
    let root = provider_api_root(prov, &req.base_url)?;
    let mut headers = auth_headers(prov, token);
    headers.push(("Content-Type", "application/json".into()));

    let resp = match prov {
        "github" => {
            let rid = resolve_release_id_if_missing(prov, &root, token, &clean_repo, &req.tag_name, &req.release_id)?;
            let url = format!("{}/repos/{}/releases/{}", root, clean_repo, rid);
            let body = serde_json::json!({
                "name": req.title.trim(),
                "body": req.notes.trim(),
                "draft": req.draft,
                "prerelease": req.prerelease,
            });
            http_json_with_retry("PATCH", &url, &headers, Some(&body), token)?
        }
        "gitee" => {
            if req.draft {
                return Err(anyhow!("Gitee 暂不支持将 Release 设为草稿模式；如需下架请使用「撤回发布」"));
            }
            let rid = resolve_release_id_if_missing(prov, &root, token, &clean_repo, &req.tag_name, &req.release_id)?;
            let url = format!("{}/api/v5/repos/{}/releases/{}", root, clean_repo, rid);
            let body = serde_json::json!({
                "access_token": token,
                "tag_name": req.tag_name.trim(),
                "name": req.title.trim(),
                "body": if req.notes.trim().is_empty() { format!("Release {}", req.tag_name.trim()) } else { req.notes.trim().to_string() },
                "prerelease": req.prerelease,
            });
            http_json_with_retry("PATCH", &url, &headers, Some(&body), token)?
        }
        "gitlab" => {
            if req.draft {
                return Err(anyhow!("GitLab Releases 暂不支持草稿状态切换；如需下架请使用「撤回发布」"));
            }
            let url = format!(
                "{}/api/v4/projects/{}/releases/{}",
                root,
                encode_uri_component(&clean_repo),
                encode_uri_component(req.tag_name.trim())
            );
            let body = serde_json::json!({
                "name": req.title.trim(),
                "description": req.notes.trim(),
            });
            http_json_with_retry("PUT", &url, &headers, Some(&body), token)?
        }
        "gitea" => {
            let rid = resolve_release_id_if_missing(prov, &root, token, &clean_repo, &req.tag_name, &req.release_id)?;
            let url = format!("{}/api/v1/repos/{}/releases/{}", root, clean_repo, rid);
            let body = serde_json::json!({
                "name": req.title.trim(),
                "body": req.notes.trim(),
                "draft": req.draft,
                "prerelease": req.prerelease,
            });
            http_json_with_retry("PATCH", &url, &headers, Some(&body), token)?
        }
        _ => return Err(anyhow!("暂不支持的仓库平台：{}", prov)),
    };

    if resp.status < 200 || resp.status >= 300 {
        return Err(anyhow!(
            "更新 Release 失败 (HTTP {})：{}",
            resp.status,
            scrub_secret(&resp.text(), token)
        ));
    }
    let val = resp.json()?;
    parse_release_item(prov, &val).ok_or_else(|| anyhow!("已更新但返回数据缺少 tag_name"))
}

/// Recall / delete an online Release, with optional deletion of its remote and/or local Git tag.
pub fn delete_release_multi(
    req: &DeleteReleaseRequest,
    local_repo_root: Option<&Path>,
) -> Result<DeleteReleaseResult> {
    let prov = normalize_provider(&req.provider)?;
    let (token, _) = resolve_token(&prov, &req.base_url, &req.credential_id)?;
    delete_release_multi_with_token(&prov, req, &token, local_repo_root)
}

fn delete_release_multi_with_token(
    prov: &str,
    req: &DeleteReleaseRequest,
    token: &str,
    local_repo_root: Option<&Path>,
) -> Result<DeleteReleaseResult> {
    let clean_repo = validate_repo_slug(&req.repo)?;
    let tag = req.tag_name.trim();
    if !tag.is_empty() {
        validate_git_tag_name(tag)?;
    }
    let root = provider_api_root(prov, &req.base_url)?;
    let headers = auth_headers(prov, token);

    let del_url = match prov {
        "github" => {
            let rid = resolve_release_id_if_missing(prov, &root, token, &clean_repo, tag, &req.release_id)?;
            format!("{}/repos/{}/releases/{}", root, clean_repo, rid)
        }
        "gitee" => {
            let rid = resolve_release_id_if_missing(prov, &root, token, &clean_repo, tag, &req.release_id)?;
            format!(
                "{}/api/v5/repos/{}/releases/{}?access_token={}",
                root,
                clean_repo,
                rid,
                encode_uri_component(token)
            )
        }
        "gitlab" => {
            if tag.is_empty() {
                return Err(anyhow!("GitLab 撤回 Release 需要有效的 tag_name"));
            }
            format!(
                "{}/api/v4/projects/{}/releases/{}",
                root,
                encode_uri_component(&clean_repo),
                encode_uri_component(tag)
            )
        }
        "gitea" => {
            let rid = resolve_release_id_if_missing(prov, &root, token, &clean_repo, tag, &req.release_id)?;
            format!("{}/api/v1/repos/{}/releases/{}", root, clean_repo, rid)
        }
        _ => return Err(anyhow!("暂不支持的仓库平台：{}", prov)),
    };

    let del_resp = http_json_with_retry("DELETE", &del_url, &headers, None, token)?;
    if del_resp.status == 404 {
        return Err(anyhow!("线上未找到该 Release（可能已被撤回或删除）"));
    }
    if del_resp.status < 200 || del_resp.status >= 300 {
        return Err(anyhow!(
            "撤回删除 Release 失败 (HTTP {})：{}",
            del_resp.status,
            scrub_secret(&del_resp.text(), token)
        ));
    }

    let mut warnings = Vec::new();
    let mut remote_tag_deleted = false;
    if req.delete_remote_tag && !tag.is_empty() {
        let tag_url = match prov {
            "github" => Some(format!(
                "{}/repos/{}/git/refs/tags/{}",
                root,
                clean_repo,
                encode_uri_component(tag)
            )),
            "gitee" => Some(format!(
                "{}/api/v5/repos/{}/tags/{}?access_token={}",
                root,
                clean_repo,
                encode_uri_component(tag),
                encode_uri_component(token)
            )),
            "gitlab" => Some(format!(
                "{}/api/v4/projects/{}/repository/tags/{}",
                root,
                encode_uri_component(&clean_repo),
                encode_uri_component(tag)
            )),
            "gitea" => Some(format!(
                "{}/api/v1/repos/{}/tags/{}",
                root,
                clean_repo,
                encode_uri_component(tag)
            )),
            _ => None,
        };
        if let Some(u) = tag_url {
            match http_json_with_retry("DELETE", &u, &headers, None, token) {
                Ok(r) if r.status >= 200 && r.status < 300 => {
                    remote_tag_deleted = true;
                }
                Ok(r) if r.status == 404 || r.status == 422 => {
                    warnings.push(format!("远端 Tag {} 不存在或已被删除", tag));
                }
                Ok(r) => {
                    // Fallback to git push delete for providers like Gitee where tag delete REST may be restricted
                    // IMPORTANT: delete remote tag ONLY so unchecked `delete_local_tag` is strictly respected!
                    if prov == "gitee" {
                        if let Some(local_root) = local_repo_root {
                            if crate::git_ops::git_delete_remote_tag_only(local_root, tag).is_ok() {
                                remote_tag_deleted = true;
                            } else {
                                warnings.push(format!("远端 Tag {} 删除失败 (HTTP {})", tag, r.status));
                            }
                        } else {
                            warnings.push(format!("远端 Tag {} 删除失败 (HTTP {})", tag, r.status));
                        }
                    } else {
                        warnings.push(format!("远端 Tag {} 删除失败 (HTTP {})", tag, r.status));
                    }
                }
                Err(e) => warnings.push(format!("远端 Tag {} 删除异常：{}", tag, scrub_secret(&e.to_string(), token))),
            }
        }
    }

    let mut local_tag_deleted = false;
    if req.delete_local_tag && !tag.is_empty() {
        if let Some(local_root) = local_repo_root {
            if local_root.join(".git").exists() {
                match git_delete_tag(local_root, tag, false) {
                    Ok(_) => local_tag_deleted = true,
                    Err(e) => warnings.push(format!("本地 Tag {} 未删除：{}", tag, e)),
                }
            } else {
                warnings.push("本地目录未初始化 Git，已跳过本地 Tag 删除".into());
            }
        }
    }

    let mut extras = Vec::new();
    if remote_tag_deleted {
        extras.push("已删远端 Tag");
    }
    if local_tag_deleted {
        extras.push("已删本地 Tag");
    }
    let summary = if extras.is_empty() {
        format!("已撤回并删除线上 Release {}", tag)
    } else {
        format!("已撤回线上 Release {}（{}）", tag, extras.join("、"))
    };

    Ok(DeleteReleaseResult {
        release_deleted: true,
        remote_tag_deleted,
        local_tag_deleted,
        summary,
        warnings,
    })
}

/// Delete a single uploaded asset from an existing online Release.
pub fn delete_release_asset_multi(req: &DeleteReleaseAssetRequest) -> Result<String> {
    let prov = normalize_provider(&req.provider)?;
    let (token, _) = resolve_token(&prov, &req.base_url, &req.credential_id)?;
    delete_release_asset_multi_with_token(&prov, req, &token)
}

fn delete_release_asset_multi_with_token(
    prov: &str,
    req: &DeleteReleaseAssetRequest,
    token: &str,
) -> Result<String> {
    let clean_repo = validate_repo_slug(&req.repo)?;
    let aid = validate_safe_id(&req.asset_id, "资产 ID")?;
    let root = provider_api_root(prov, &req.base_url)?;
    let headers = auth_headers(prov, token);

    let url = match prov {
        "github" => format!("{}/repos/{}/releases/assets/{}", root, clean_repo, aid),
        "gitee" => {
            let rid = resolve_release_id_if_missing(prov, &root, token, &clean_repo, &req.tag_name, &req.release_id)?;
            format!(
                "{}/api/v5/repos/{}/releases/{}/attach_files/{}?access_token={}",
                root,
                clean_repo,
                rid,
                aid,
                encode_uri_component(token)
            )
        }
        "gitlab" => format!(
            "{}/api/v4/projects/{}/releases/{}/assets/links/{}",
            root,
            encode_uri_component(&clean_repo),
            encode_uri_component(req.tag_name.trim()),
            aid
        ),
        "gitea" => {
            let rid = resolve_release_id_if_missing(prov, &root, token, &clean_repo, &req.tag_name, &req.release_id)?;
            format!("{}/api/v1/repos/{}/releases/{}/assets/{}", root, clean_repo, rid, aid)
        }
        _ => return Err(anyhow!("暂不支持的仓库平台：{}", prov)),
    };

    let resp = http_json_with_retry("DELETE", &url, &headers, None, token)?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(anyhow!(
            "删除线上资产失败 (HTTP {})：{}",
            resp.status,
            scrub_secret(&resp.text(), token)
        ));
    }
    let label = if req.asset_name.trim().is_empty() {
        format!("#{}", aid)
    } else {
        req.asset_name.trim().to_string()
    };
    Ok(format!("已从线上 Release 移除资产 {}", label))
}

/// Multipart streaming reader combining prefix + file stream + suffix without loading large binaries into RAM.
struct MultipartFileReader {
    prefix: std::io::Cursor<Vec<u8>>,
    file: File,
    suffix: std::io::Cursor<Vec<u8>>,
    stage: u8,
}

impl MultipartFileReader {
    fn new(field_name: &str, file_name: &str, file_path: &Path, boundary: &str) -> Result<(Self, u64)> {
        let file = File::open(file_path)?;
        let file_len = file.metadata()?.len();
        let safe_name = file_name.replace(['"', '\r', '\n'], "_");
        let prefix_bytes = format!(
            "--{}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n",
            boundary, field_name, safe_name
        )
        .into_bytes();
        let suffix_bytes = format!("\r\n--{}--\r\n", boundary).into_bytes();
        let total_len = prefix_bytes.len() as u64 + file_len + suffix_bytes.len() as u64;
        Ok((
            Self {
                prefix: std::io::Cursor::new(prefix_bytes),
                file,
                suffix: std::io::Cursor::new(suffix_bytes),
                stage: 0,
            },
            total_len,
        ))
    }
}

impl Read for MultipartFileReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            match self.stage {
                0 => {
                    let n = self.prefix.read(buf)?;
                    if n > 0 {
                        return Ok(n);
                    }
                    self.stage = 1;
                }
                1 => {
                    let n = self.file.read(buf)?;
                    if n > 0 {
                        return Ok(n);
                    }
                    self.stage = 2;
                }
                _ => return self.suffix.read(buf),
            }
        }
    }
}

/// Order upload artifacts atomically: binary packages first, checksum table next,
/// and `latest.json` strictly LAST so live clients never see a manifest pointing to
/// an asset that has not finished uploading and verifying.
pub fn upload_order_rank(file_name: &str) -> u8 {
    if file_name.eq_ignore_ascii_case("latest.json") {
        2
    } else if file_name.eq_ignore_ascii_case("SHA256SUMS.txt") {
        1
    } else {
        0
    }
}

/// Publish a Release + stream-upload all selected artifacts to GitHub, Gitee, GitLab, or Gitea.
pub fn publish_release_multi<F>(
    release_dir: &Path,
    req: &MultiPublishRequest,
    mut on_progress: F,
) -> Result<String>
where
    F: FnMut(u8, &str),
{
    let prov = normalize_provider(&req.provider)?;
    let clean_repo = validate_repo_slug(&req.repo)?;
    validate_git_tag_name(&req.tag)?;
    let tag = req.tag.trim();

    if req.files.is_empty() {
        return Err(anyhow!("请至少选择一个要上传的产物文件"));
    }

    let mut file_entries: Vec<(String, std::path::PathBuf, u64)> = Vec::new();
    let mut clean_file_names: Vec<String> = Vec::new();
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for f in &req.files {
        let p = crate::guardrails::resolve_confined_path(release_dir, f)
            .map_err(|e| anyhow!("待上传文件路径越界或非法（仅允许上传 release 目录内产物）：{} ({})", f, e))?;
        if !p.exists() || !p.is_file() {
            return Err(anyhow!("待上传文件不存在：{}", f));
        }
        let fname = p
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("无效的文件名：{}", f))?
            .to_string();
        if !seen_names.insert(fname.to_ascii_lowercase()) {
            continue;
        }
        let size = p.metadata()?.len();
        if prov == "gitee" && size > 100 * 1024 * 1024 {
            return Err(anyhow!(
                "文件 {} 体积 {:.1} MB 超过 Gitee 单文件 100 MB 限制，请先在「分析与分包」启用 95 MB 分卷打包后再上传。",
                fname,
                size as f64 / (1024.0 * 1024.0)
            ));
        }
        clean_file_names.push(fname.clone());
        file_entries.push((fname, p, size));
    }

    // Upload binaries first, SHA256SUMS.txt second, latest.json strictly last
    file_entries.sort_by_key(|(fname, _, _)| upload_order_rank(fname));

    // Safety gate: block localhost/127.0.0.1 leakage, size drift & stale SHA-256 in latest.json / SHA256SUMS.txt
    validate_manifest_for_publish(release_dir, tag, &clean_file_names)?;

    let (token, _) = resolve_token(&prov, &req.base_url, &req.credential_id)?;
    let root = provider_api_root(&prov, &req.base_url)?;
    on_progress(8, &format!("正在连接 {} 仓库 {}…", prov, clean_repo));

    match prov.as_str() {
        "github" => publish_github(&root, &token, &clean_repo, tag, req, &file_entries, &mut on_progress),
        "gitee" => publish_gitee(&root, &token, &clean_repo, tag, req, &file_entries, &mut on_progress),
        "gitlab" => publish_gitlab(&root, &token, &clean_repo, tag, req, &file_entries, &mut on_progress),
        "gitea" => publish_gitea(&root, &token, &clean_repo, tag, req, &file_entries, &mut on_progress),
        _ => Err(anyhow!("暂不支持的仓库平台：{}", prov)),
    }
}

fn delete_github_asset_by_name(
    api_root: &str,
    headers: &[(&str, String)],
    token: &str,
    repo: &str,
    rel_id: u64,
    fname: &str,
) {
    let list_url = format!("{}/repos/{}/releases/{}/assets?per_page=100", api_root, repo, rel_id);
    if let Ok(resp) = http_json_with_retry("GET", &list_url, headers, None, token) {
        if resp.status == 200 {
            if let Ok(Value::Array(arr)) = resp.json() {
                for a in arr {
                    let aname = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if aname == fname {
                        if let Some(aid) = a.get("id").and_then(|x| x.as_u64()) {
                            let del_url = format!("{}/repos/{}/releases/assets/{}", api_root, repo, aid);
                            let _ = http_json_with_retry("DELETE", &del_url, headers, None, token);
                        }
                    }
                }
            }
        }
    }
}

fn verify_uploaded_asset_response(fname: &str, expected_size: u64, resp: &HttpResponse) -> Result<()> {
    let Ok(val) = resp.json() else {
        return Ok(());
    };
    if let Some(state) = val.get("state").and_then(|s| s.as_str()) {
        if !state.is_empty() && !state.eq_ignore_ascii_case("uploaded") {
            return Err(anyhow!(
                "远端资产 {} 状态异常（state={}，未完成上传）",
                fname,
                state
            ));
        }
    }
    if let Some(remote_size) = val.get("size").and_then(|s| s.as_u64()) {
        if remote_size != expected_size {
            return Err(anyhow!(
                "远端资产 {} 体积校验不一致（期望 {} B，远端返回 {} B）",
                fname,
                expected_size,
                remote_size
            ));
        }
    }
    Ok(())
}

fn publish_github<F>(
    api_root: &str,
    token: &str,
    repo: &str,
    tag: &str,
    req: &MultiPublishRequest,
    files: &[(String, std::path::PathBuf, u64)],
    on_progress: &mut F,
) -> Result<String>
where
    F: FnMut(u8, &str),
{
    let mut headers = auth_headers("github", token);
    headers.push(("Content-Type", "application/json".into()));

    // 1. Check if release already exists for tag
    let get_url = format!("{}/repos/{}/releases/tags/{}", api_root, repo, encode_uri_component(tag));
    let existing = http_json_with_retry("GET", &get_url, &headers, None, token)?;
    let mut existing_asset_ids: std::collections::HashMap<String, Vec<u64>> = std::collections::HashMap::new();
    let release_json = if existing.status == 200 {
        if !req.overwrite_assets {
            return Err(anyhow!(
                "GitHub Release {} 已存在；如需覆盖资产请勾选「覆盖同名资产」",
                tag
            ));
        }
        let rel = existing.json()?;
        let rel_id = rel["id"].as_u64().ok_or_else(|| anyhow!("无法读取 Release ID"))?;
        let patch_url = format!("{}/repos/{}/releases/{}", api_root, repo, rel_id);
        let body = serde_json::json!({
            "name": req.title.trim(),
            "body": req.notes.trim(),
            "draft": req.draft,
            "prerelease": req.prerelease,
        });
        let patched = http_json_with_retry("PATCH", &patch_url, &headers, Some(&body), token)?;
        if patched.status >= 300 {
            return Err(anyhow!("更新 GitHub Release 失败 (HTTP {})", patched.status));
        }
        // Record colliding asset IDs so each old asset is deleted JUST BEFORE uploading its replacement
        // (keeping old latest.json alive until all new binaries finish uploading & verifying!)
        if let Some(assets) = rel.get("assets").and_then(|a| a.as_array()) {
            for a in assets {
                let aname = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if let Some(aid) = a.get("id").and_then(|x| x.as_u64()) {
                    if !aname.is_empty() {
                        existing_asset_ids.entry(aname.to_string()).or_default().push(aid);
                    }
                }
            }
        }
        patched.json()?
    } else {
        let create_url = format!("{}/repos/{}/releases", api_root, repo);
        let body = serde_json::json!({
            "tag_name": tag,
            "name": req.title.trim(),
            "body": req.notes.trim(),
            "draft": req.draft,
            "prerelease": req.prerelease,
        });
        let created = http_json_with_retry("POST", &create_url, &headers, Some(&body), token)?;
        if created.status < 200 || created.status >= 300 {
            return Err(anyhow!(
                "创建 GitHub Release 失败 (HTTP {})：{}",
                created.status,
                scrub_secret(&created.text(), token)
            ));
        }
        created.json()?
    };

    let rel_id_opt = release_json.get("id").and_then(|v| v.as_u64());
    let upload_tmpl = release_json
        .get("upload_url")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let upload_base = upload_tmpl.split('{').next().unwrap_or("").to_string();
    if upload_base.is_empty() {
        return Err(anyhow!("GitHub 未返回 upload_url"));
    }

    let html_url = release_json
        .get("html_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("https://github.com/{}/releases/tag/{}", repo, tag));

    let total_files = files.len().max(1);
    for (idx, (fname, path, size)) in files.iter().enumerate() {
        // Delete colliding old asset for THIS file only right before uploading its replacement
        if let Some(aids) = existing_asset_ids.remove(fname) {
            for aid in aids {
                let del_url = format!("{}/repos/{}/releases/assets/{}", api_root, repo, aid);
                let _ = http_json_with_retry("DELETE", &del_url, &headers, None, token);
            }
        }
        let up_url = format!("{}?name={}", upload_base, encode_uri_component(fname));
        let mut up_headers = auth_headers("github", token);
        up_headers.push(("Content-Type", "application/octet-stream".into()));

        let base_pct = 15 + (idx * 80) / total_files;
        let span_pct = (80 / total_files).max(1);
        on_progress(base_pct as u8, &format!("上传 [{}/{}] {}…", idx + 1, total_files, fname));

        let mut last_err = None;
        let mut ok = false;
        for attempt in 0..=NET_RETRY_BACKOFF_MS.len() {
            if attempt > 0 {
                on_progress(
                    base_pct as u8,
                    &format!(
                        "网络波动，清理残片并第 {}/{} 次重试上传 {}…",
                        attempt,
                        NET_RETRY_BACKOFF_MS.len(),
                        fname
                    ),
                );
                // Clean up any partial / "starter" orphan asset left by a previous interrupted upload
                if let Some(rid) = rel_id_opt {
                    delete_github_asset_by_name(api_root, &headers, token, repo, rid, fname);
                }
                std::thread::sleep(Duration::from_millis(NET_RETRY_BACKOFF_MS[attempt - 1]));
            }
            let mut f = File::open(path)?;
            let current_size = f.metadata()?.len();
            if current_size != *size {
                return Err(anyhow!(
                    "文件 {} 在发布期间体积发生变化（预检 {} B，当前 {} B），已中止上传以防清单不一致",
                    fname,
                    size,
                    current_size
                ));
            }
            match winhttp_request("POST", &up_url, &up_headers, Some((&mut f, *size)), |done, tot| {
                let sub = if tot > 0 { ((done * span_pct as u64) / tot) as usize } else { 0 };
                on_progress(
                    (base_pct + sub).min(97) as u8,
                    &format!("上传 {} ({:.1}/{:.1} MB)", fname, done as f64 / 1048576.0, tot as f64 / 1048576.0),
                );
            }) {
                Ok(r) if r.status >= 200 && r.status < 300 => {
                    if let Err(ve) = verify_uploaded_asset_response(fname, *size, &r) {
                        if let Ok(val) = r.json() {
                            if let Some(aid) = val.get("id").and_then(|x| x.as_u64()) {
                                let del_url = format!("{}/repos/{}/releases/assets/{}", api_root, repo, aid);
                                let _ = http_json_with_retry("DELETE", &del_url, &headers, None, token);
                            }
                        }
                        last_err = Some(ve);
                        continue;
                    }
                    ok = true;
                    break;
                }
                Ok(r) => {
                    last_err = Some(anyhow!("上传 {} 失败 (HTTP {})：{}", fname, r.status, scrub_secret(&r.text(), token)));
                    if r.status == 422 && rel_id_opt.is_some() && attempt < NET_RETRY_BACKOFF_MS.len() {
                        // Colliding starter asset from earlier attempt — loop will clean it up and retry
                        continue;
                    }
                    if r.status < 500 && r.status != 429 {
                        break;
                    }
                }
                Err(e) => {
                    last_err = Some(anyhow!("{}", scrub_secret(&e.to_string(), token)));
                }
            }
        }
        if !ok {
            return Err(last_err.unwrap_or_else(|| anyhow!("上传 {} 失败", fname)));
        }
    }

    on_progress(100, "GitHub Release 发布完成");
    Ok(html_url)
}

fn publish_gitee<F>(
    api_root: &str,
    token: &str,
    repo: &str,
    tag: &str,
    req: &MultiPublishRequest,
    files: &[(String, std::path::PathBuf, u64)],
    on_progress: &mut F,
) -> Result<String>
where
    F: FnMut(u8, &str),
{
    let mut headers = auth_headers("gitee", token);
    headers.push(("Content-Type", "application/json".into()));

    let get_url = format!(
        "{}/api/v5/repos/{}/releases/tags/{}?access_token={}",
        api_root,
        repo,
        encode_uri_component(tag),
        encode_uri_component(token)
    );
    let existing = http_json_with_retry("GET", &get_url, &headers, None, token)?;
    let mut existing_asset_ids: std::collections::HashMap<String, Vec<u64>> = std::collections::HashMap::new();
    let release_json = if existing.status == 200 {
        if !req.overwrite_assets {
            return Err(anyhow!("Gitee Release {} 已存在；如需覆盖请勾选「覆盖同名资产」", tag));
        }
        let rel = existing.json()?;
        // Defer colliding attachment deletion until just before each file uploads (protects latest.json atomicity)
        if let Some(assets) = rel.get("assets").and_then(|a| a.as_array()) {
            for a in assets {
                let aname = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if let Some(aid) = a.get("id").and_then(|x| x.as_u64()) {
                    if !aname.is_empty() {
                        existing_asset_ids.entry(aname.to_string()).or_default().push(aid);
                    }
                }
            }
        }
        rel
    } else {
        // Dynamically resolve default branch instead of hardcoding "master"
        let info_url = format!(
            "{}/api/v5/repos/{}?access_token={}",
            api_root,
            repo,
            encode_uri_component(token)
        );
        let default_branch = http_json_with_retry("GET", &info_url, &headers, None, token)
            .ok()
            .filter(|r| r.status == 200)
            .and_then(|r| r.json().ok())
            .and_then(|v| v.get("default_branch").and_then(|b| b.as_str()).map(|s| s.to_string()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "main".to_string());

        let create_url = format!("{}/api/v5/repos/{}/releases", api_root, repo);
        let body = serde_json::json!({
            "access_token": token,
            "tag_name": tag,
            "name": req.title.trim(),
            "body": if req.notes.trim().is_empty() { format!("Release {}", tag) } else { req.notes.trim().to_string() },
            "prerelease": req.prerelease,
            "target_commitish": default_branch,
        });
        let created = http_json_with_retry("POST", &create_url, &headers, Some(&body), token)?;
        if created.status < 200 || created.status >= 300 {
            return Err(anyhow!(
                "创建 Gitee Release 失败 (HTTP {})：{}",
                created.status,
                scrub_secret(&created.text(), token)
            ));
        }
        created.json()?
    };

    let rel_id = release_json["id"]
        .as_u64()
        .ok_or_else(|| anyhow!("无法获取 Gitee Release ID"))?;
    let total_files = files.len().max(1);
    for (idx, (fname, path, expected_size)) in files.iter().enumerate() {
        if let Some(aids) = existing_asset_ids.remove(fname) {
            for aid in aids {
                let del_url = format!(
                    "{}/api/v5/repos/{}/releases/{}/attach_files/{}?access_token={}",
                    api_root,
                    repo,
                    rel_id,
                    aid,
                    encode_uri_component(token)
                );
                let _ = http_json_with_retry("DELETE", &del_url, &headers, None, token);
            }
        }
        let up_url = format!(
            "{}/api/v5/repos/{}/releases/{}/attach_files?access_token={}",
            api_root,
            repo,
            rel_id,
            encode_uri_component(token)
        );
        let boundary = format!("ShipForgeBoundary{}", chrono::Local::now().timestamp_nanos_opt().unwrap_or(1));
        let mut up_headers = auth_headers("gitee", token);
        up_headers.push(("Content-Type", format!("multipart/form-data; boundary={}", boundary)));
        let base_pct = 15 + (idx * 80) / total_files;
        let span_pct = (80 / total_files).max(1);
        let mut last_err = None;
        let mut ok = false;
        for attempt in 0..=NET_RETRY_BACKOFF_MS.len() {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(NET_RETRY_BACKOFF_MS[attempt - 1]));
            }
            let current_size = std::fs::metadata(path)?.len();
            if current_size != *expected_size {
                return Err(anyhow!(
                    "文件 {} 在发布期间体积发生变化（预检 {} B，当前 {} B），已中止上传",
                    fname,
                    expected_size,
                    current_size
                ));
            }
            let (mut reader, total_len) = MultipartFileReader::new("file", fname, path, &boundary)?;
            match winhttp_request("POST", &up_url, &up_headers, Some((&mut reader, total_len)), |done, tot| {
                let sub = if tot > 0 { ((done * span_pct as u64) / tot) as usize } else { 0 };
                on_progress((base_pct + sub).min(97) as u8, &format!("上传 {}…", fname));
            }) {
                Ok(resp) if resp.status >= 200 && resp.status < 300 => {
                    if let Err(ve) = verify_uploaded_asset_response(fname, *expected_size, &resp) {
                        last_err = Some(ve);
                        continue;
                    }
                    ok = true;
                    break;
                }
                Ok(resp) => {
                    last_err = Some(anyhow!(
                        "上传 Gitee 附件 {} 失败 (HTTP {})：{}",
                        fname,
                        resp.status,
                        scrub_secret(&resp.text(), token)
                    ));
                    if resp.status < 500 && resp.status != 429 {
                        break;
                    }
                }
                Err(e) => {
                    last_err = Some(anyhow!("{}", scrub_secret(&e.to_string(), token)));
                }
            }
        }
        if !ok {
            return Err(last_err.unwrap_or_else(|| anyhow!("上传 Gitee 附件 {} 失败", fname)));
        }
    }
    on_progress(100, "Gitee Release 发布完成");
    Ok(format!("{}/{}/releases/tag/{}", api_root, repo, tag))
}

fn publish_gitlab<F>(
    api_root: &str,
    token: &str,
    repo: &str,
    tag: &str,
    req: &MultiPublishRequest,
    files: &[(String, std::path::PathBuf, u64)],
    on_progress: &mut F,
) -> Result<String>
where
    F: FnMut(u8, &str),
{
    let enc_repo = encode_uri_component(repo);
    let pkg_ver = tag.trim_start_matches('v');
    let mut links = Vec::new();
    let total_files = files.len().max(1);

    // 1. Upload each file to GitLab Generic Package Registry
    for (idx, (fname, path, size)) in files.iter().enumerate() {
        let up_url = format!(
            "{}/api/v4/projects/{}/packages/generic/release/{}/{}",
            api_root,
            enc_repo,
            encode_uri_component(pkg_ver),
            encode_uri_component(fname)
        );
        let up_headers = auth_headers("gitlab", token);
        let base_pct = 12 + (idx * 75) / total_files;
        let span_pct = (75 / total_files).max(1);
        let mut last_err = None;
        let mut ok = false;
        for attempt in 0..=NET_RETRY_BACKOFF_MS.len() {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(NET_RETRY_BACKOFF_MS[attempt - 1]));
            }
            let mut f = File::open(path)?;
            match winhttp_request("PUT", &up_url, &up_headers, Some((&mut f, *size)), |done, tot| {
                let sub = if tot > 0 { ((done * span_pct as u64) / tot) as usize } else { 0 };
                on_progress((base_pct + sub).min(90) as u8, &format!("上传 {}…", fname));
            }) {
                Ok(resp) if resp.status >= 200 && resp.status < 300 => {
                    ok = true;
                    break;
                }
                Ok(resp) => {
                    last_err = Some(anyhow!(
                        "上传 GitLab 产物 {} 失败 (HTTP {})：{}",
                        fname,
                        resp.status,
                        scrub_secret(&resp.text(), token)
                    ));
                    if resp.status < 500 && resp.status != 429 {
                        break;
                    }
                }
                Err(e) => {
                    last_err = Some(anyhow!("{}", scrub_secret(&e.to_string(), token)));
                }
            }
        }
        if !ok {
            return Err(last_err.unwrap_or_else(|| anyhow!("上传 GitLab 产物 {} 失败", fname)));
        }
        links.push(serde_json::json!({
            "name": fname,
            "url": up_url,
            "link_type": "package",
        }));
    }

    // 2. Create or update GitLab Release pointing to those package assets
    let mut headers = auth_headers("gitlab", token);
    headers.push(("Content-Type", "application/json".into()));
    let create_url = format!("{}/api/v4/projects/{}/releases", api_root, enc_repo);
    let body = serde_json::json!({
        "name": req.title.trim(),
        "tag_name": tag,
        "description": req.notes.trim(),
        "assets": { "links": links },
    });
    let resp = http_json_with_retry("POST", &create_url, &headers, Some(&body), token)?;
    if resp.status == 409 && req.overwrite_assets {
        let put_url = format!(
            "{}/api/v4/projects/{}/releases/{}",
            api_root,
            enc_repo,
            encode_uri_component(tag)
        );
        let upd = serde_json::json!({
            "name": req.title.trim(),
            "description": req.notes.trim(),
        });
        let put_resp = http_json_with_retry("PUT", &put_url, &headers, Some(&upd), token)?;
        if put_resp.status < 200 || put_resp.status >= 300 {
            return Err(anyhow!(
                "更新 GitLab Release 失败 (HTTP {})：{}",
                put_resp.status,
                scrub_secret(&put_resp.text(), token)
            ));
        }
        // Sync missing asset links onto existing GitLab Release
        let links_url = format!(
            "{}/api/v4/projects/{}/releases/{}/assets/links",
            api_root,
            enc_repo,
            encode_uri_component(tag)
        );
        let existing_names: std::collections::HashSet<String> = http_json_with_retry("GET", &links_url, &headers, None, token)
            .ok()
            .filter(|r| r.status == 200)
            .and_then(|r| r.json().ok())
            .and_then(|v| v.as_array().cloned())
            .map(|arr| {
                arr.into_iter()
                    .filter_map(|x| x.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        for link in &links {
            let lname = link.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if !lname.is_empty() && !existing_names.contains(lname) {
                let _ = http_json_with_retry("POST", &links_url, &headers, Some(link), token);
            }
        }
    } else if resp.status < 200 || resp.status >= 300 {
        return Err(anyhow!(
            "创建 GitLab Release 失败 (HTTP {})：{}",
            resp.status,
            scrub_secret(&resp.text(), token)
        ));
    }

    on_progress(100, "GitLab Release 发布完成");
    Ok(format!("{}/{}/-/releases/{}", api_root, repo, tag))
}

fn publish_gitea<F>(
    api_root: &str,
    token: &str,
    repo: &str,
    tag: &str,
    req: &MultiPublishRequest,
    files: &[(String, std::path::PathBuf, u64)],
    on_progress: &mut F,
) -> Result<String>
where
    F: FnMut(u8, &str),
{
    let mut headers = auth_headers("gitea", token);
    headers.push(("Content-Type", "application/json".into()));
    let get_url = format!(
        "{}/api/v1/repos/{}/releases/tags/{}",
        api_root,
        repo,
        encode_uri_component(tag)
    );
    let existing = http_json_with_retry("GET", &get_url, &headers, None, token)?;
    let mut existing_asset_ids: std::collections::HashMap<String, Vec<u64>> = std::collections::HashMap::new();
    let release_json = if existing.status == 200 {
        if !req.overwrite_assets {
            return Err(anyhow!("Gitea Release {} 已存在；如需覆盖请勾选「覆盖同名资产」", tag));
        }
        let rel = existing.json()?;
        if let Some(assets) = rel.get("assets").and_then(|a| a.as_array()) {
            for a in assets {
                let aname = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if let Some(aid) = a.get("id").and_then(|x| x.as_u64()) {
                    if !aname.is_empty() {
                        existing_asset_ids.entry(aname.to_string()).or_default().push(aid);
                    }
                }
            }
        }
        rel
    } else {
        let create_url = format!("{}/api/v1/repos/{}/releases", api_root, repo);
        let body = serde_json::json!({
            "tag_name": tag,
            "name": req.title.trim(),
            "body": req.notes.trim(),
            "draft": req.draft,
            "prerelease": req.prerelease,
        });
        let created = http_json_with_retry("POST", &create_url, &headers, Some(&body), token)?;
        if created.status < 200 || created.status >= 300 {
            return Err(anyhow!(
                "创建 Gitea Release 失败 (HTTP {})：{}",
                created.status,
                scrub_secret(&created.text(), token)
            ));
        }
        created.json()?
    };

    let rel_id = release_json["id"]
        .as_u64()
        .ok_or_else(|| anyhow!("无法获取 Gitea Release ID"))?;
    let total_files = files.len().max(1);
    for (idx, (fname, path, expected_size)) in files.iter().enumerate() {
        if let Some(aids) = existing_asset_ids.remove(fname) {
            for aid in aids {
                let del_url = format!(
                    "{}/api/v1/repos/{}/releases/{}/assets/{}",
                    api_root, repo, rel_id, aid
                );
                let _ = http_json_with_retry("DELETE", &del_url, &headers, None, token);
            }
        }
        let up_url = format!(
            "{}/api/v1/repos/{}/releases/{}/assets?name={}",
            api_root,
            repo,
            rel_id,
            encode_uri_component(fname)
        );
        let boundary = format!("ShipForgeGitea{}", chrono::Local::now().timestamp_nanos_opt().unwrap_or(2));
        let mut up_headers = auth_headers("gitea", token);
        up_headers.push(("Content-Type", format!("multipart/form-data; boundary={}", boundary)));
        let base_pct = 15 + (idx * 80) / total_files;
        let span_pct = (80 / total_files).max(1);
        let mut last_err = None;
        let mut ok = false;
        for attempt in 0..=NET_RETRY_BACKOFF_MS.len() {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(NET_RETRY_BACKOFF_MS[attempt - 1]));
            }
            let current_size = std::fs::metadata(path)?.len();
            if current_size != *expected_size {
                return Err(anyhow!(
                    "文件 {} 在发布期间体积发生变化（预检 {} B，当前 {} B），已中止上传",
                    fname,
                    expected_size,
                    current_size
                ));
            }
            let (mut reader, total_len) = MultipartFileReader::new("attachment", fname, path, &boundary)?;
            match winhttp_request("POST", &up_url, &up_headers, Some((&mut reader, total_len)), |done, tot| {
                let sub = if tot > 0 { ((done * span_pct as u64) / tot) as usize } else { 0 };
                on_progress((base_pct + sub).min(97) as u8, &format!("上传 {}…", fname));
            }) {
                Ok(resp) if resp.status >= 200 && resp.status < 300 => {
                    if let Err(ve) = verify_uploaded_asset_response(fname, *expected_size, &resp) {
                        last_err = Some(ve);
                        continue;
                    }
                    ok = true;
                    break;
                }
                Ok(resp) => {
                    last_err = Some(anyhow!(
                        "上传 Gitea 资产 {} 失败 (HTTP {})：{}",
                        fname,
                        resp.status,
                        scrub_secret(&resp.text(), token)
                    ));
                    if resp.status < 500 && resp.status != 429 {
                        break;
                    }
                }
                Err(e) => {
                    last_err = Some(anyhow!("{}", scrub_secret(&e.to_string(), token)));
                }
            }
        }
        if !ok {
            return Err(last_err.unwrap_or_else(|| anyhow!("上传 Gitea 资产 {} 失败", fname)));
        }
    }

    on_progress(100, "Gitea Release 发布完成");
    Ok(format!("{}/{}/releases/tag/{}", api_root, repo, tag))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn test_parse_http_url_rejects_injection_and_embedded_creds() {
        assert!(parse_http_url("https://api.github.com/user").is_ok());
        assert!(parse_http_url("http://127.0.0.1:8080/api/v4").is_ok());
        assert!(parse_http_url("ftp://example.com").is_err());
        assert!(parse_http_url("https://user:pass@github.com").is_err());
        assert!(parse_http_url("https://example.com/a\r\nInjected: 1").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn test_winhttp_verify_token_against_loopback_mock() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let body = r#"{"login":"dev-tester"}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let user = verify_token("github", &format!("http://127.0.0.1:{}", port), "ghp_mocktoken12345")
            .unwrap();
        assert_eq!(user, "dev-tester");
        let _ = handle.join();
    }

    #[test]
    fn test_validate_repo_slug_and_crlf_header_blocking() {
        assert_eq!(validate_repo_slug("owner/repo-1.2").unwrap(), "owner/repo-1.2");
        assert_eq!(validate_repo_slug("org/sub/project").unwrap(), "org/sub/project");
        assert!(validate_repo_slug("owner/../admin").is_err());
        assert!(validate_repo_slug("owner/repo?x=1").is_err());
        assert!(validate_repo_slug("onlyone").is_err());

        #[cfg(windows)]
        {
            let err = winhttp_request(
                "GET",
                "http://127.0.0.1:9",
                &[("Authorization", "Bearer abc\r\nX-Injected: evil".into())],
                None,
                |_, _| {},
            );
            assert!(err.is_err());
            assert!(err.err().unwrap().to_string().contains("非法换行"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_release_management_list_update_delete_asset_and_recall_mock() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{}", port);

        let handle = std::thread::spawn(move || {
            let mut handled = 0usize;
            while handled < 5 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_text.lines().next().unwrap_or("");
                handled += 1;

                let (status, body) = if first_line.starts_with("GET /repos/acme/app/releases") {
                    (
                        "200 OK",
                        r#"[{"id":42,"tag_name":"v1.2.3","name":"App v1.2.3","body":"Initial notes","draft":false,"prerelease":false,"published_at":"2026-11-25T10:00:00Z","html_url":"https://github.com/acme/app/releases/tag/v1.2.3","assets":[{"id":99,"name":"App_1.2.3_x64-setup.exe","size":102400,"browser_download_url":"https://github.com/acme/app/releases/download/v1.2.3/App_1.2.3_x64-setup.exe"}]}]"#,
                    )
                } else if first_line.starts_with("PATCH /repos/acme/app/releases/42") {
                    (
                        "200 OK",
                        r#"{"id":42,"tag_name":"v1.2.3","name":"App v1.2.3 (Hotfix)","body":"Updated notes","draft":true,"prerelease":false,"published_at":"2026-11-25T10:00:00Z","html_url":"https://github.com/acme/app/releases/tag/v1.2.3","assets":[]}"#,
                    )
                } else if first_line.starts_with("DELETE /repos/acme/app/releases/assets/99") {
                    ("204 No Content", "")
                } else if first_line.starts_with("DELETE /repos/acme/app/releases/42") {
                    ("204 No Content", "")
                } else if first_line.starts_with("DELETE /repos/acme/app/git/refs/tags/v1.2.3") {
                    ("204 No Content", "")
                } else {
                    ("404 Not Found", r#"{"message":"not found"}"#)
                };

                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let tok = "ghp_mocktoken_manage";
        // 1. List releases with id, body, assets
        let list = list_releases_multi_with_token("github", &base_url, "acme/app", tok).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "42");
        assert_eq!(list[0].body, "Initial notes");
        assert_eq!(list[0].assets.len(), 1);
        assert_eq!(list[0].assets[0].id, "99");
        assert_eq!(list[0].assets[0].name, "App_1.2.3_x64-setup.exe");

        // 2. Update release (edit notes + soft unpublish to draft)
        let updated = update_release_multi_with_token(
            "github",
            &UpdateReleaseRequest {
                project_id: "demo".into(),
                provider: "github".into(),
                base_url: base_url.clone(),
                credential_id: String::new(),
                repo: "acme/app".into(),
                release_id: "42".into(),
                tag_name: "v1.2.3".into(),
                title: "App v1.2.3 (Hotfix)".into(),
                notes: "Updated notes".into(),
                prerelease: false,
                draft: true,
            },
            tok,
        )
        .unwrap();
        assert!(updated.is_draft);
        assert_eq!(updated.body, "Updated notes");

        // 3. Delete single uploaded asset
        let del_asset = delete_release_asset_multi_with_token(
            "github",
            &DeleteReleaseAssetRequest {
                provider: "github".into(),
                base_url: base_url.clone(),
                credential_id: String::new(),
                repo: "acme/app".into(),
                release_id: "42".into(),
                tag_name: "v1.2.3".into(),
                asset_id: "99".into(),
                asset_name: "App_1.2.3_x64-setup.exe".into(),
            },
            tok,
        )
        .unwrap();
        assert!(del_asset.contains("App_1.2.3_x64-setup.exe"));

        // 4. Recall release + remote tag
        let del_rel = delete_release_multi_with_token(
            "github",
            &DeleteReleaseRequest {
                project_id: "demo".into(),
                provider: "github".into(),
                base_url: base_url.clone(),
                credential_id: String::new(),
                repo: "acme/app".into(),
                release_id: "42".into(),
                tag_name: "v1.2.3".into(),
                delete_remote_tag: true,
                delete_local_tag: false,
            },
            tok,
            None,
        )
        .unwrap();
        assert!(del_rel.release_deleted);
        assert!(del_rel.remote_tag_deleted);
        assert!(del_rel.warnings.is_empty());

        let _ = handle.join();
    }

    #[test]
    fn test_publish_release_multi_blocks_absolute_and_traversal_files() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_pub_rt_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(29)
        ));
        let rel = base.join("release");
        std::fs::create_dir_all(&rel).unwrap();
        let outside = base.join("secret.env");
        std::fs::write(&outside, b"TOKEN=secret").unwrap();

        let mk_req = |files: Vec<String>| MultiPublishRequest {
            project_id: "rt".into(),
            provider: "github".into(),
            base_url: String::new(),
            credential_id: String::new(),
            repo: "acme/app".into(),
            tag: "v1.0.0".into(),
            title: "v1.0.0".into(),
            notes: "test".into(),
            files,
            prerelease: false,
            draft: true,
            overwrite_assets: true,
        };

        let err_rel = publish_release_multi(&rel, &mk_req(vec!["../secret.env".into()]), |_, _| {})
            .unwrap_err();
        assert!(err_rel.to_string().contains("越界") || err_rel.to_string().contains("非法"));

        let err_abs = publish_release_multi(
            &rel,
            &mk_req(vec![outside.to_string_lossy().to_string()]),
            |_, _| {},
        )
        .unwrap_err();
        assert!(err_abs.to_string().contains("绝对路径") || err_abs.to_string().contains("越界"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(windows)]
    #[test]
    fn test_redteam_upload_ordering_starter_cleanup_and_size_verify() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};

        assert!(upload_order_rank("QDisk_1.0.0_x64-setup.exe") < upload_order_rank("SHA256SUMS.txt"));
        assert!(upload_order_rank("SHA256SUMS.txt") < upload_order_rank("latest.json"));

        let base = std::env::temp_dir().join(format!(
            "shipforge_rt_pub_mock_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(99)
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let bin_path = base.join("App_1.0.0_x64-setup.exe");
        let bin_data = b"MZ-redteam-installer-bytes-1234567890";
        std::fs::write(&bin_path, bin_data).unwrap();
        let bin_sha = crate::packager::sha256_file(&bin_path).unwrap();
        let sums_path = base.join("SHA256SUMS.txt");
        std::fs::write(&sums_path, format!("{}  App_1.0.0_x64-setup.exe\n", bin_sha)).unwrap();
        let mf_path = base.join("latest.json");
        let mf_body = serde_json::json!({
            "version": "1.0.0",
            "installer": {
                "file": "App_1.0.0_x64-setup.exe",
                "url": "https://github.com/acme/app/releases/download/v1.0.0/App_1.0.0_x64-setup.exe",
                "size": bin_data.len(),
                "sha256": bin_sha,
            }
        });
        std::fs::write(&mf_path, serde_json::to_string(&mf_body).unwrap()).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let api_root = format!("http://127.0.0.1:{}", port);

        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let ev_srv = events.clone();
        let api_srv = api_root.clone();

        let srv = std::thread::spawn(move || {
            let mut bin_upload_attempts = 0usize;
            for _ in 0..15 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut raw = Vec::new();
                let mut tmp = [0u8; 4096];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let Ok(n) = stream.read(&mut tmp) else { break };
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&tmp[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                            let hdrs = String::from_utf8_lossy(&raw[..pos]);
                            for line in hdrs.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(he) = header_end {
                        if raw.len() >= he + content_len {
                            break;
                        }
                    }
                }
                let head = String::from_utf8_lossy(&raw[..header_end.unwrap_or(raw.len())]);
                let first_line = head.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("");
                let target = parts.next().unwrap_or("");

                let (status, resp_body) = match (method, target) {
                    ("GET", "/repos/acme/app/releases/tags/v1.0.0") => (
                        "200 OK",
                        format!(
                            r#"{{"id":77,"tag_name":"v1.0.0","upload_url":"{}/upload/77{{?name}}","html_url":"https://github.com/acme/app/releases/tag/v1.0.0","assets":[{{"id":501,"name":"App_1.0.0_x64-setup.exe"}},{{"id":502,"name":"latest.json"}}]}}"#,
                            api_srv
                        ),
                    ),
                    ("PATCH", "/repos/acme/app/releases/77") => (
                        "200 OK",
                        format!(
                            r#"{{"id":77,"tag_name":"v1.0.0","upload_url":"{}/upload/77{{?name}}","html_url":"https://github.com/acme/app/releases/tag/v1.0.0"}}"#,
                            api_srv
                        ),
                    ),
                    ("DELETE", p) if p.starts_with("/repos/acme/app/releases/assets/") => {
                        ev_srv.lock().unwrap().push(format!("DELETE {}", p));
                        ("204 No Content", String::new())
                    }
                    ("GET", p) if p.starts_with("/repos/acme/app/releases/77/assets") => (
                        "200 OK",
                        r#"[{"id":601,"name":"App_1.0.0_x64-setup.exe","state":"starter"}]"#.to_string(),
                    ),
                    ("POST", p) if p.starts_with("/upload/77?name=") => {
                        let fname = p.trim_start_matches("/upload/77?name=");
                        ev_srv.lock().unwrap().push(format!("UPLOAD {}", fname));
                        if fname == "App_1.0.0_x64-setup.exe" {
                            bin_upload_attempts += 1;
                            if bin_upload_attempts == 1 {
                                // Simulate GitHub returning broken starter / truncated asset on first upload
                                (
                                    "201 Created",
                                    r#"{"id":601,"name":"App_1.0.0_x64-setup.exe","state":"starter","size":10}"#.to_string(),
                                )
                            } else {
                                (
                                    "201 Created",
                                    format!(
                                        r#"{{"id":602,"name":"App_1.0.0_x64-setup.exe","state":"uploaded","size":{}}}"#,
                                        content_len
                                    ),
                                )
                            }
                        } else {
                            (
                                "201 Created",
                                format!(
                                    r#"{{"id":603,"name":"{}","state":"uploaded","size":{}}}"#,
                                    fname, content_len
                                ),
                            )
                        }
                    }
                    _ => ("404 Not Found", "{}".to_string()),
                };

                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    resp_body.len(),
                    resp_body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                if target.contains("latest.json") && method == "POST" {
                    break;
                }
            }
        });

        // Intentionally pass latest.json FIRST to verify atomic sorting + per-file deferred delete!
        let mut files = vec![
            ("latest.json".to_string(), mf_path.clone(), std::fs::metadata(&mf_path).unwrap().len()),
            ("SHA256SUMS.txt".to_string(), sums_path.clone(), std::fs::metadata(&sums_path).unwrap().len()),
            ("App_1.0.0_x64-setup.exe".to_string(), bin_path.clone(), bin_data.len() as u64),
        ];
        files.sort_by_key(|(n, _, _)| upload_order_rank(n));

        let req = MultiPublishRequest {
            project_id: "rt".into(),
            provider: "github".into(),
            base_url: api_root.clone(),
            credential_id: String::new(),
            repo: "acme/app".into(),
            tag: "v1.0.0".into(),
            title: "v1.0.0".into(),
            notes: "notes".into(),
            files: vec!["latest.json".into(), "SHA256SUMS.txt".into(), "App_1.0.0_x64-setup.exe".into()],
            prerelease: false,
            draft: false,
            overwrite_assets: true,
        };

        let url = publish_github(&api_root, "tok", "acme/app", "v1.0.0", &req, &files, &mut |_, _| {})
            .expect("should clean up broken starter asset, retry, and succeed atomically");
        assert!(url.ends_with("/v1.0.0"));
        let _ = srv.join();

        let log = events.lock().unwrap().clone();
        let pos_bin_ok = log.iter().rposition(|e| e == "UPLOAD App_1.0.0_x64-setup.exe").unwrap();
        let pos_del_latest = log.iter().position(|e| e == "DELETE /repos/acme/app/releases/assets/502").unwrap();
        let pos_up_latest = log.iter().position(|e| e == "UPLOAD latest.json").unwrap();
        let pos_del_starter = log.iter().position(|e| e == "DELETE /repos/acme/app/releases/assets/601").unwrap();
        assert!(pos_del_starter < pos_bin_ok, "starter asset 601 must be deleted before retry succeeds: {:?}", log);
        assert!(pos_bin_ok < pos_del_latest, "old latest.json (502) must NOT be deleted before binary upload succeeds: {:?}", log);
        assert!(pos_del_latest < pos_up_latest, "old latest.json must be deleted right before new latest.json upload: {:?}", log);

        let _ = std::fs::remove_dir_all(&base);
    }
}
