use crate::models::{LocalServerStatus, ServerLogEntry};
use crate::packager::encode_uri_component;
use anyhow::{anyhow, Result};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const MAX_CONCURRENT_CONNS: usize = 32;
const MAX_REQUEST_LINE_BYTES: usize = 4096;
const MAX_HEADER_LINE_BYTES: usize = 8192;
const MAX_HEADERS_COUNT: usize = 64;
const MAX_MANIFEST_REWRITE_BYTES: u64 = 4 * 1024 * 1024;

struct ConnSlotGuard(Arc<AtomicUsize>);
impl Drop for ConnSlotGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn read_line_capped<R: BufRead>(reader: &mut R, max_bytes: usize) -> Result<Option<String>> {
    let mut raw = Vec::new();
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            if raw.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let take = pos + 1;
            if raw.len() + take > max_bytes {
                return Err(anyhow!("HTTP line exceeds limit"));
            }
            raw.extend_from_slice(&buf[..take]);
            reader.consume(take);
            break;
        } else {
            let len = buf.len();
            if raw.len() + len > max_bytes {
                return Err(anyhow!("HTTP line exceeds limit"));
            }
            raw.extend_from_slice(buf);
            reader.consume(len);
        }
    }
    Ok(Some(String::from_utf8_lossy(&raw).into_owned()))
}

pub struct LocalFeedServer {
    running: Arc<AtomicBool>,
    project_id: String,
    project_name: String,
    port: u16,
    dir: PathBuf,
    rewrite_manifest: bool,
    hit_count: Arc<AtomicU64>,
    logs: Arc<Mutex<Vec<ServerLogEntry>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Default for LocalFeedServer {
    fn default() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            project_id: String::new(),
            project_name: String::new(),
            port: 18765,
            dir: PathBuf::new(),
            rewrite_manifest: true,
            hit_count: Arc::new(AtomicU64::new(0)),
            logs: Arc::new(Mutex::new(Vec::new())),
            handle: None,
        }
    }
}

impl LocalFeedServer {
    pub fn status(&self) -> LocalServerStatus {
        let running = self.running.load(Ordering::SeqCst);
        let base_url = format!("http://127.0.0.1:{}/", self.port);
        let feed_url = format!("{}latest.json", base_url);
        let logs = self.logs.lock().map(|g| g.clone()).unwrap_or_default();
        LocalServerStatus {
            running,
            project_id: if running { self.project_id.clone() } else { String::new() },
            project_name: if running { self.project_name.clone() } else { String::new() },
            port: self.port,
            base_url,
            feed_url,
            dir: self.dir.to_string_lossy().to_string(),
            rewrite_manifest: self.rewrite_manifest,
            hit_count: self.hit_count.load(Ordering::SeqCst),
            logs,
        }
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        self.project_id.clear();
        self.project_name.clear();
    }

    #[allow(dead_code)]
    pub fn start(
        &mut self,
        dir: PathBuf,
        port: u16,
        rewrite_manifest: bool,
        app: Option<AppHandle>,
    ) -> Result<LocalServerStatus> {
        self.start_for_project(String::new(), String::new(), dir, port, rewrite_manifest, app)
    }

    pub fn start_for_project(
        &mut self,
        project_id: String,
        project_name: String,
        dir: PathBuf,
        port: u16,
        rewrite_manifest: bool,
        app: Option<AppHandle>,
    ) -> Result<LocalServerStatus> {
        if !dir.exists() || !dir.is_dir() {
            return Err(anyhow!("release 目录不存在：{}", dir.display()));
        }
        if port < 1024 {
            return Err(anyhow!("本地端口请使用 1024~65535 范围"));
        }

        // Deterministically join previous accept thread so OS releases the socket before re-binding
        self.stop();

        let listener = TcpListener::bind(("127.0.0.1", port))
            .map_err(|e| anyhow!("无法监听 127.0.0.1:{}（{}）", port, e))?;
        listener.set_nonblocking(true)?;

        let running = Arc::new(AtomicBool::new(true));
        let hit_count = Arc::new(AtomicU64::new(0));
        let logs = Arc::new(Mutex::new(Vec::<ServerLogEntry>::new()));

        self.running = running.clone();
        self.project_id = project_id;
        self.project_name = project_name;
        self.port = port;
        self.dir = dir.clone();
        self.rewrite_manifest = rewrite_manifest;
        self.hit_count = hit_count.clone();
        self.logs = logs.clone();

        let active_conns = Arc::new(AtomicUsize::new(0));
        let join_handle = thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _addr)) => {
                        let prev = active_conns.fetch_add(1, Ordering::SeqCst);
                        if prev >= MAX_CONCURRENT_CONNS {
                            active_conns.fetch_sub(1, Ordering::SeqCst);
                            let _ = stream.set_nonblocking(false);
                            let _ = write_simple(&mut stream, 503, "Service Unavailable", "Too many concurrent connections");
                            continue;
                        }
                        let slot = ConnSlotGuard(active_conns.clone());
                        let dir_c = dir.clone();
                        let hit_c = hit_count.clone();
                        let logs_c = logs.clone();
                        let app_c = app.clone();
                        thread::spawn(move || {
                            let _slot = slot;
                            handle_client(
                                stream,
                                &dir_c,
                                port,
                                rewrite_manifest,
                                &hit_c,
                                &logs_c,
                                app_c.as_ref(),
                            );
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(40));
                    }
                }
            }
            drop(listener);
        });
        self.handle = Some(join_handle);

        Ok(self.status())
    }
}

/// Rewrites `latest.json` download URLs in memory to point to `http://127.0.0.1:<port>/<file>`
/// so production `release/latest.json` on disk is NEVER polluted with localhost URLs!
pub fn rewrite_manifest_for_loopback(raw: &str, port: u16) -> Option<String> {
    let mut val: serde_json::Value =
        serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;
    let base = format!("http://127.0.0.1:{}/", port);

    let rewrite_entry = |entry: &mut serde_json::Value| {
        if let Some(obj) = entry.as_object_mut() {
            if let Some(fname) = obj
                .get("file")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
            {
                obj.insert(
                    "url".into(),
                    serde_json::Value::String(format!("{}{}", base, encode_uri_component(&fname))),
                );
            }
        }
    };

    if let Some(inst) = val.get_mut("installer") {
        rewrite_entry(inst);
    }
    if let Some(port_obj) = val.get_mut("portable") {
        rewrite_entry(port_obj);
    }
    if let Some(plats) = val.get_mut("platforms").and_then(|x| x.as_object_mut()) {
        for (_k, v) in plats.iter_mut() {
            rewrite_entry(v);
        }
    }

    serde_json::to_string_pretty(&val).ok().map(|s| s + "\n")
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn sanitize_request_file(raw_target: &str) -> Option<String> {
    let no_query = raw_target.split('?').next().unwrap_or("");
    let decoded = percent_decode(no_query);
    let trimmed = decoded.trim_start_matches('/');
    let file = if trimmed.is_empty() {
        "latest.json"
    } else {
        trimmed
    };
    if file == "."
        || file == ".."
        || file.contains("..")
        || file.contains('/')
        || file.contains('\\')
        || file.contains(':')
        || file.contains('\0')
        || file.chars().any(|c| c.is_control())
        || crate::guardrails::is_win_reserved_name(file)
    {
        return None;
    }
    Some(file.to_string())
}

fn handle_client(
    mut stream: TcpStream,
    dir: &Path,
    port: u16,
    rewrite_manifest: bool,
    hit_count: &AtomicU64,
    logs: &Mutex<Vec<ServerLogEntry>>,
    app: Option<&AppHandle>,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let mut reader = BufReader::new(&stream);
    let request_line = match read_line_capped(&mut reader, MAX_REQUEST_LINE_BYTES) {
        Ok(Some(line)) if !line.trim().is_empty() => line,
        Ok(_) => return,
        Err(_) => {
            let _ = write_simple(&mut stream, 414, "URI Too Long", "Request line too long");
            return;
        }
    };

    let mut header_count = 0usize;
    let mut range_header: Option<String> = None;
    loop {
        match read_line_capped(&mut reader, MAX_HEADER_LINE_BYTES) {
            Ok(Some(h)) if !h.trim().is_empty() => {
                header_count += 1;
                if header_count > MAX_HEADERS_COUNT {
                    let _ = write_simple(
                        &mut stream,
                        431,
                        "Request Header Fields Too Large",
                        "Too many headers",
                    );
                    return;
                }
                if let Some((k, v)) = h.split_once(':') {
                    if k.trim().eq_ignore_ascii_case("range") && range_header.is_none() {
                        range_header = Some(v.trim().to_string());
                    }
                }
            }
            Ok(_) => break,
            Err(_) => {
                let _ = write_simple(
                    &mut stream,
                    431,
                    "Request Header Fields Too Large",
                    "Header line too long",
                );
                return;
            }
        }
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    if method != "GET" && method != "HEAD" {
        let _ = write_simple(&mut stream, 405, "Method Not Allowed", "Method Not Allowed");
        record_log(&method, &target, 405, 0, hit_count, logs, app);
        return;
    }

    let Some(file_name) = sanitize_request_file(&target) else {
        let _ = write_simple(&mut stream, 403, "Forbidden", "Forbidden");
        record_log(&method, &target, 403, 0, hit_count, logs, app);
        return;
    };

    let Ok(full) = crate::guardrails::resolve_confined_path(dir, &file_name) else {
        let _ = write_simple(&mut stream, 403, "Forbidden", "Forbidden");
        record_log(&method, &file_name, 403, 0, hit_count, logs, app);
        return;
    };
    if !full.is_file() {
        let _ = write_simple(&mut stream, 404, "Not Found", "Not Found");
        record_log(&method, &file_name, 404, 0, hit_count, logs, app);
        return;
    }

    // Dynamic in-memory loopback rewrite for latest.json (bounded size)
    if rewrite_manifest && file_name.eq_ignore_ascii_case("latest.json") {
        let fsize = fs::metadata(&full).map(|m| m.len()).unwrap_or(u64::MAX);
        if fsize <= MAX_MANIFEST_REWRITE_BYTES {
            if let Ok(raw) = fs::read_to_string(&full) {
                if let Some(rewritten) = rewrite_manifest_for_loopback(&raw, port) {
                    let bytes = rewritten.as_bytes();
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let sent = if method == "GET" {
                        let _ = stream.write_all(bytes);
                        bytes.len() as u64
                    } else {
                        0
                    };
                    let _ = stream.flush();
                    record_log(
                        &method,
                        &format!("/{} (本地重写)", file_name),
                        200,
                        sent,
                        hit_count,
                        logs,
                        app,
                    );
                    return;
                }
            }
        }
    }

    let Ok(mut f) = File::open(&full) else {
        let _ = write_simple(&mut stream, 500, "Internal Server Error", "Cannot open file");
        return;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let content_type = if file_name.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if file_name.ends_with(".txt") || file_name.ends_with(".md") {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    };

    // Optional single-range HTTP/1.1 Range support (RFC 7233) for resumable updater testing
    let byte_range: Option<(u64, u64)> = if let Some(ref rh) = range_header {
        if let Some(spec) = rh.strip_prefix("bytes=").map(str::trim) {
            if !spec.contains(',') {
                if let Some((s_str, e_str)) = spec.split_once('-') {
                    if len == 0 {
                        let resp = "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = stream.write_all(resp.as_bytes());
                        record_log(&method, &format!("/{}", file_name), 416, 0, hit_count, logs, app);
                        return;
                    }
                    let parsed = if s_str.is_empty() {
                        // Suffix range: bytes=-N
                        e_str.parse::<u64>().ok().filter(|&n| n > 0).map(|n| {
                            let start = len.saturating_sub(n);
                            (start, len - 1)
                        })
                    } else if let Ok(start) = s_str.parse::<u64>() {
                        if start >= len {
                            let resp = format!(
                                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                len
                            );
                            let _ = stream.write_all(resp.as_bytes());
                            record_log(&method, &format!("/{}", file_name), 416, 0, hit_count, logs, app);
                            return;
                        }
                        if e_str.is_empty() {
                            Some((start, len - 1))
                        } else if let Ok(end) = e_str.parse::<u64>() {
                            if end < start {
                                let resp = format!(
                                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                    len
                                );
                                let _ = stream.write_all(resp.as_bytes());
                                record_log(&method, &format!("/{}", file_name), 416, 0, hit_count, logs, app);
                                return;
                            }
                            Some((start, end.min(len - 1)))
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    parsed
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let (status_code, start_off, send_len, header) = if let Some((start, end)) = byte_range {
        let part_len = end - start + 1;
        let hdr = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
            content_type, start, end, len, part_len
        );
        (206u16, start, part_len, hdr)
    } else {
        let hdr = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
            content_type, len
        );
        (200u16, 0u64, len, hdr)
    };

    if start_off > 0 && f.seek(SeekFrom::Start(start_off)).is_err() {
        let _ = write_simple(&mut stream, 500, "Internal Server Error", "Seek failed");
        return;
    }
    if stream.write_all(header.as_bytes()).is_err() {
        return;
    }

    let mut sent = 0u64;
    if method == "GET" {
        let mut buf = [0u8; 65536];
        let mut remaining = send_len;
        while remaining > 0 {
            let want = (remaining as usize).min(buf.len());
            match f.read(&mut buf[..want]) {
                Ok(0) => break,
                Ok(n) => {
                    if stream.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    sent += n as u64;
                    remaining = remaining.saturating_sub(n as u64);
                }
                Err(_) => break,
            }
        }
    }
    let _ = stream.flush();
    record_log(&method, &format!("/{}", file_name), status_code, sent, hit_count, logs, app);
}

fn write_simple(stream: &mut TcpStream, code: u16, reason: &str, body: &str) -> std::io::Result<()> {
    let resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        code,
        reason,
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes())
}

fn record_log(
    method: &str,
    path: &str,
    status: u16,
    bytes: u64,
    hit_count: &AtomicU64,
    logs: &Mutex<Vec<ServerLogEntry>>,
    app: Option<&AppHandle>,
) {
    hit_count.fetch_add(1, Ordering::SeqCst);
    let entry = ServerLogEntry {
        ts: chrono::Local::now().format("%H:%M:%S").to_string(),
        method: method.to_string(),
        path: path.to_string(),
        status,
        bytes,
    };
    if let Ok(mut guard) = logs.lock() {
        guard.push(entry.clone());
        if guard.len() > 80 {
            let drain = guard.len() - 80;
            guard.drain(0..drain);
        }
    }
    if let Some(a) = app {
        let _ = a.emit("local-server-log", &entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_blocks_traversal() {
        assert_eq!(sanitize_request_file("/"), Some("latest.json".into()));
        assert_eq!(
            sanitize_request_file("/DemoApp_1.2.0_x64-setup.exe?t=1"),
            Some("DemoApp_1.2.0_x64-setup.exe".into())
        );
        assert_eq!(sanitize_request_file("/../secret.txt"), None);
        assert_eq!(sanitize_request_file("/%2e%2e/secret.txt"), None);
        assert_eq!(sanitize_request_file("/sub/file.zip"), None);
        assert_eq!(sanitize_request_file("/latest.json::$DATA"), None);
        assert_eq!(sanitize_request_file("/C:win.ini"), None);
        assert_eq!(sanitize_request_file("/."), None);
        assert_eq!(sanitize_request_file("/CON"), None);
        assert_eq!(sanitize_request_file("/nul.json"), None);
        assert_eq!(sanitize_request_file("/AUX"), None);
    }

    #[test]
    fn test_local_server_blocks_ads_and_oversized_headers() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_srv_rt_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(21)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("latest.json"), r#"{"version":"1.0.0"}"#).unwrap();

        let mut srv = LocalFeedServer::default();
        let port = 19881;
        srv.start(dir.clone(), port, false, None).unwrap();
        thread::sleep(Duration::from_millis(60));

        // 1. ADS colon injection -> 403
        let mut s1 = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s1.write_all(b"GET /latest.json::$DATA HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        let mut r1 = String::new();
        s1.read_to_string(&mut r1).unwrap();
        assert!(r1.starts_with("HTTP/1.1 403"), "expected 403, got: {}", r1);

        // 2. Too many headers (>64) -> 431
        let mut s2 = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut flood = String::from("GET /latest.json HTTP/1.1\r\n");
        for i in 0..70 {
            flood.push_str(&format!("X-Flood-{}: aaaa\r\n", i));
        }
        flood.push_str("\r\n");
        s2.write_all(flood.as_bytes()).unwrap();
        let mut r2 = String::new();
        s2.read_to_string(&mut r2).unwrap();
        assert!(r2.starts_with("HTTP/1.1 431"), "expected 431, got: {}", r2);

        // 3. Oversized request line (>4 KiB) -> 414
        let mut s3 = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let huge_uri = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(5000));
        s3.write_all(huge_uri.as_bytes()).unwrap();
        let mut r3 = String::new();
        s3.read_to_string(&mut r3).unwrap();
        assert!(r3.starts_with("HTTP/1.1 414"), "expected 414, got: {}", r3);

        srv.stop();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_local_http_server_rewrites_manifest_in_memory_only() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_srv_test_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(3)
        ));
        fs::create_dir_all(&dir).unwrap();
        let prod_manifest = r#"{"version":"1.0.0","installer":{"file":"App_1.0.0.exe","url":"https://github.com/a/b/releases/download/v1.0.0/App_1.0.0.exe"}}"#;
        fs::write(dir.join("latest.json"), prod_manifest).unwrap();

        let mut srv = LocalFeedServer::default();
        let port = 19877;
        srv.start(dir.clone(), port, true, None).unwrap();
        thread::sleep(Duration::from_millis(80));

        let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        sock.write_all(b"GET /latest.json HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        let mut resp = String::new();
        sock.read_to_string(&mut resp).unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("http://127.0.0.1:19877/App_1.0.0.exe"));

        // Verify on-disk latest.json remained untouched with GitHub production URL!
        let on_disk = fs::read_to_string(dir.join("latest.json")).unwrap();
        assert!(on_disk.contains("https://github.com/a/b/"));
        assert!(!on_disk.contains("127.0.0.1"));

        srv.stop();
        // Rapid immediate restart on the EXACT same port must succeed deterministically!
        srv.start(dir.clone(), port, false, None).unwrap();
        srv.stop();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_local_http_server_range_206_and_416() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_srv_range_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(92)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("payload.bin"), b"0123456789ABCDEF").unwrap();

        let mut srv = LocalFeedServer::default();
        let port = 19882;
        srv.start(dir.clone(), port, false, None).unwrap();
        thread::sleep(Duration::from_millis(80));

        // 1. Valid Range: bytes=4-9 -> 206 Partial Content with "456789"
        let mut s1 = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s1.write_all(b"GET /payload.bin HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes=4-9\r\n\r\n")
            .unwrap();
        let mut r1 = String::new();
        s1.read_to_string(&mut r1).unwrap();
        assert!(r1.starts_with("HTTP/1.1 206 Partial Content"), "resp={r1}");
        assert!(r1.contains("Content-Range: bytes 4-9/16"), "resp={r1}");
        assert!(r1.ends_with("\r\n\r\n456789"), "resp={r1}");

        // 2. Out-of-bounds Range -> 416 Range Not Satisfiable
        let mut s2 = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s2.write_all(b"GET /payload.bin HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes=999-\r\n\r\n")
            .unwrap();
        let mut r2 = String::new();
        s2.read_to_string(&mut r2).unwrap();
        assert!(r2.starts_with("HTTP/1.1 416"), "resp={r2}");

        srv.stop();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_local_server_tracks_project_ownership_and_clears_on_stop() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_srv_owner_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(93)
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut srv = LocalFeedServer::default();
        let port = 19883;
        srv.start_for_project(
            "proj-qdisk".into(),
            "QDisk".into(),
            dir.clone(),
            port,
            true,
            None,
        )
        .unwrap();
        let st = srv.status();
        assert!(st.running);
        assert_eq!(st.project_id, "proj-qdisk");
        assert_eq!(st.project_name, "QDisk");

        srv.stop();
        let st2 = srv.status();
        assert!(!st2.running);
        assert!(st2.project_id.is_empty());
        assert!(st2.project_name.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }
}
