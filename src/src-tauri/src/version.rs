use crate::models::{ChangelogData, ChangelogEntry, VersionFileSpec, VersionReading, VersionState};
use anyhow::{anyhow, Context, Result};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn semver_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$").unwrap()
    })
}

fn toml_ver_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"^(\s*version\s*=\s*")([^"]+)(".*)$"#).unwrap())
}

fn toml_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"^\s*name\s*=\s*"([^"]+)""#).unwrap())
}

fn csproj_ver_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(<Version(?:Prefix)?>\s*)([^<\s]+)(\s*</Version(?:Prefix)?>)").unwrap())
}

fn pubspec_ver_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^(version\s*:\s*)([^\s+#]+)").unwrap())
}

fn changelog_heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^#{2,3}\s+(?:\*\*|__)?(?:\[|【)?(?:version\s+|版本\s*)?v?(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)(?:\]|】)?(?:\*\*|__)?(?:$|[^\d.])",
        )
        .unwrap()
    })
}

fn readme_changelog_section_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^##\s+(?:[^\w\u4e00-\u9fa5]*\s*)?(?:changelog|release\s*notes|what'?s\s*new|version\s*history|updates?|更新日志|更新内容|更新记录|版本历史|版本更新|版本记录|变更记录)\b").unwrap()
    })
}

fn unreleased_heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^##\s+(?:\[|【)?(unreleased|next|待发布|未发布|下个版本|下一版本)(?:\]|】)?(?:\s|$|[（(—\-:：])").unwrap()
    })
}

fn url_cred_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(https?://)([^/\s@:]+):([^/\s@]+)@").unwrap())
}

fn user_profile_path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b[A-Z]:\\(?:Users|Documents and Settings)\\[^\\\s`'\x22),;>]+(?:\\[^\s`'\x22),;>]+)?").unwrap()
    })
}

fn secret_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(?:ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|glpat-[A-Za-z0-9\-_]{16,})\b").unwrap())
}

pub fn validate_semver(ver: &str) -> Result<()> {
    if !semver_re().is_match(ver.trim()) {
        return Err(anyhow!("版本号格式无效：{}（应为 1.2.3 或 1.2.3-beta.1）", ver));
    }
    Ok(())
}

pub fn bump_semver(current: &str, kind: &str) -> Result<String> {
    let cur = current.trim();
    let caps = semver_re()
        .captures(cur)
        .ok_or_else(|| anyhow!("无法解析当前版本号：{}", cur))?;
    let major: u64 = caps[1].parse()?;
    let minor: u64 = caps[2].parse()?;
    let patch: u64 = caps[3].parse()?;
    let pre = caps.get(4).map(|m| m.as_str()).unwrap_or("");

    match kind {
        "major" => Ok(format!("{}.0.0", major + 1)),
        "minor" => Ok(format!("{}.{}.0", major, minor + 1)),
        "patch" => {
            if !pre.is_empty() {
                Ok(format!("{}.{}.{}", major, minor, patch))
            } else {
                Ok(format!("{}.{}.{}", major, minor, patch + 1))
            }
        }
        "beta" => {
            if let Some(rest) = pre.strip_prefix("beta.") {
                let n: u64 = rest.parse().unwrap_or(0);
                Ok(format!("{}.{}.{}-beta.{}", major, minor, patch, n + 1))
            } else if pre.is_empty() {
                Ok(format!("{}.{}.{}-beta.1", major, minor, patch + 1))
            } else {
                Ok(format!("{}.{}.{}-beta.1", major, minor, patch))
            }
        }
        _ => Err(anyhow!("未知的版本递增类型：{}", kind)),
    }
}

pub fn read_version_file(root: &Path, spec: &VersionFileSpec) -> VersionReading {
    let full = match crate::guardrails::resolve_confined_path(root, &spec.path) {
        Ok(p) => p,
        Err(e) => {
            return VersionReading {
                path: spec.path.clone(),
                format: spec.format.clone(),
                version: String::new(),
                exists: false,
                error: Some(e.to_string()),
            };
        }
    };
    if !full.exists() {
        return VersionReading {
            path: spec.path.clone(),
            format: spec.format.clone(),
            version: String::new(),
            exists: false,
            error: Some("文件不存在".into()),
        };
    }

    let raw = match fs::read_to_string(&full) {
        Ok(s) => s,
        Err(e) => {
            return VersionReading {
                path: spec.path.clone(),
                format: spec.format.clone(),
                version: String::new(),
                exists: true,
                error: Some(format!("读取失败：{}", e)),
            };
        }
    };

    match extract_version_from_text(&raw, &spec.format, &spec.pattern) {
        Ok(v) => VersionReading {
            path: spec.path.clone(),
            format: spec.format.clone(),
            version: v,
            exists: true,
            error: None,
        },
        Err(e) => VersionReading {
            path: spec.path.clone(),
            format: spec.format.clone(),
            version: String::new(),
            exists: true,
            error: Some(e.to_string()),
        },
    }
}

pub fn extract_version_from_text(raw: &str, format: &str, pattern: &str) -> Result<String> {
    match format {
        "tauri_json" => {
            let val: serde_json::Value =
                serde_json::from_str(raw.trim_start_matches('\u{feff}'))
                    .context("JSON 解析失败")?;
            // Support both Tauri v2 (top-level version) and Tauri v1 (package.version)
            val.get("version")
                .and_then(|v| v.as_str())
                .or_else(|| {
                    val.get("package")
                        .and_then(|p| p.get("version"))
                        .and_then(|v| v.as_str())
                })
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow!("tauri.conf.json 中未找到 version 或 package.version"))
        }
        "package_json" => {
            let val: serde_json::Value =
                serde_json::from_str(raw.trim_start_matches('\u{feff}'))
                    .context("JSON 解析失败")?;
            val.get("version")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow!("package.json 中未找到顶层 version 字段"))
        }
        "cargo_toml" => extract_toml_section_version(raw, "package")
            .or_else(|_| extract_toml_section_version(raw, "workspace.package")),
        "pyproject_toml" => extract_toml_section_version(raw, "project")
            .or_else(|_| extract_toml_section_version(raw, "tool.poetry")),
        "csproj" => csproj_ver_re()
            .captures(raw)
            .map(|c| c[2].to_string())
            .ok_or_else(|| anyhow!("csproj 中未找到 <Version> 或 <VersionPrefix> 标签")),
        "pubspec_yaml" => pubspec_ver_re()
            .captures(raw)
            .map(|c| c[2].trim().to_string())
            .ok_or_else(|| anyhow!("pubspec.yaml 中未找到 version 字段")),
        "regex" => {
            let re = Regex::new(pattern).context("自定义正则无效")?;
            re.captures(raw)
                .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
                .ok_or_else(|| anyhow!("未匹配到自定义版本正则"))
        }
        other => Err(anyhow!("不支持的版本文件格式：{}", other)),
    }
}

fn parse_toml_header(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') || trimmed.starts_with("[[") {
        return None;
    }
    let end = trimmed.find(']')?;
    // Ignore anything after ']' (inline comments like `[package] # main`)
    Some(trimmed[1..end].trim())
}

fn extract_toml_section_version(raw: &str, section: &str) -> Result<String> {
    let mut in_section = false;
    for line in raw.lines() {
        if let Some(hdr) = parse_toml_header(line) {
            in_section = hdr == section;
            continue;
        }
        if in_section {
            if let Some(caps) = toml_ver_re().captures(line) {
                return Ok(caps[2].to_string());
            }
        }
    }
    Err(anyhow!("TOML 的 [{}] 段中未找到 version", section))
}

/// Replace `"version": "..."` at a specific JSON brace depth (depth 1 = top-level, depth 2 inside `"package"` for Tauri v1).
fn replace_json_version_scoped(raw: &str, new_version: &str, allow_package_nested: bool) -> Result<String> {
    // Track brace depth outside strings so we never touch nested dependency versions.
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut last_key = String::new();
    let mut in_package_obj_depth: Option<i32> = None;

    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        match b {
            b'{' => {
                depth += 1;
                if allow_package_nested && depth == 2 && last_key == "package" {
                    in_package_obj_depth = Some(2);
                }
                i += 1;
            }
            b'}' => {
                if in_package_obj_depth == Some(depth) {
                    in_package_obj_depth = None;
                }
                depth -= 1;
                i += 1;
            }
            b'"' => {
                // Parse string token
                let start = i + 1;
                let mut j = start;
                let mut esc = false;
                while j < bytes.len() {
                    if esc {
                        esc = false;
                    } else if bytes[j] == b'\\' {
                        esc = true;
                    } else if bytes[j] == b'"' {
                        break;
                    }
                    j += 1;
                }
                let token = &raw[start..j];
                // Check if followed by ':' (it's a key)
                let mut k = j + 1;
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if k < bytes.len() && bytes[k] == b':' {
                    last_key = token.to_string();
                    let in_target_scope = depth == 1
                        || (allow_package_nested && in_package_obj_depth == Some(depth));
                    if token == "version" && in_target_scope {
                        // Advance past ':' and whitespace to value opening quote
                        let mut vstart = k + 1;
                        while vstart < bytes.len() && bytes[vstart].is_ascii_whitespace() {
                            vstart += 1;
                        }
                        if vstart < bytes.len() && bytes[vstart] == b'"' {
                            let val_inner_start = vstart + 1;
                            let mut vend = val_inner_start;
                            let mut vesc = false;
                            while vend < bytes.len() {
                                if vesc {
                                    vesc = false;
                                } else if bytes[vend] == b'\\' {
                                    vesc = true;
                                } else if bytes[vend] == b'"' {
                                    break;
                                }
                                vend += 1;
                            }
                            if vend < bytes.len() {
                                let mut out = String::with_capacity(raw.len() + 16);
                                out.push_str(&raw[..val_inner_start]);
                                out.push_str(new_version);
                                out.push_str(&raw[vend..]);
                                return Ok(out);
                            }
                        }
                    }
                }
                i = j + 1;
            }
            _ => i += 1,
        }
    }

    Err(anyhow!("JSON 中未找到可更新的顶层 version 字段"))
}

pub fn replace_version_in_text(
    raw: &str,
    format: &str,
    pattern: &str,
    new_version: &str,
) -> Result<String> {
    match format {
        "tauri_json" => replace_json_version_scoped(raw, new_version, true),
        "package_json" => replace_json_version_scoped(raw, new_version, false),
        "cargo_toml" => replace_toml_section_version(raw, "package", new_version)
            .or_else(|_| replace_toml_section_version(raw, "workspace.package", new_version)),
        "pyproject_toml" => replace_toml_section_version(raw, "project", new_version)
            .or_else(|_| replace_toml_section_version(raw, "tool.poetry", new_version)),
        "csproj" => {
            if !csproj_ver_re().is_match(raw) {
                return Err(anyhow!("csproj 中未找到 <Version> 或 <VersionPrefix> 标签"));
            }
            Ok(csproj_ver_re()
                .replace(raw, format!("${{1}}{}${{3}}", new_version))
                .into_owned())
        }
        "pubspec_yaml" => {
            if !pubspec_ver_re().is_match(raw) {
                return Err(anyhow!("pubspec.yaml 中未找到 version 字段"));
            }
            Ok(pubspec_ver_re()
                .replace(raw, format!("${{1}}{}", new_version))
                .into_owned())
        }
        "regex" => {
            let re = Regex::new(pattern).context("自定义正则无效")?;
            if !re.is_match(raw) {
                return Err(anyhow!("未匹配到自定义版本正则"));
            }
            Ok(re.replace(raw, new_version).into_owned())
        }
        other => Err(anyhow!("不支持的版本文件格式：{}", other)),
    }
}

fn replace_toml_section_version(raw: &str, section: &str, new_version: &str) -> Result<String> {
    let mut in_section = false;
    let mut replaced = false;
    let line_ending = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let ends_with_newline = raw.ends_with('\n');

    let mut out_lines = Vec::new();
    for line in raw.lines() {
        if let Some(hdr) = parse_toml_header(line) {
            in_section = hdr == section;
        }
        if in_section && !replaced {
            if let Some(caps) = toml_ver_re().captures(line) {
                out_lines.push(format!("{}{}{}", &caps[1], new_version, &caps[3]));
                replaced = true;
                continue;
            }
        }
        out_lines.push(line.to_string());
    }

    if !replaced {
        return Err(anyhow!("TOML 的 [{}] 段中未找到 version", section));
    }

    let mut joined = out_lines.join(line_ending);
    if ends_with_newline {
        joined.push_str(line_ending);
    }
    Ok(joined)
}

pub fn read_project_versions(root: &Path, specs: &[VersionFileSpec]) -> VersionState {
    let readings: Vec<VersionReading> = specs.iter().map(|s| read_version_file(root, s)).collect();
    let valid: Vec<&str> = readings
        .iter()
        .filter(|r| r.exists && r.error.is_none() && !r.version.is_empty())
        .map(|r| r.version.as_str())
        .collect();
    let current = valid.first().copied().unwrap_or("0.0.0").to_string();
    // Every configured version file must exist, parse cleanly, and agree on the same version.
    let synced = !readings.is_empty()
        && valid.len() == readings.len()
        && valid.iter().all(|v| *v == current);
    VersionState {
        current,
        synced,
        readings,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SemverPreIdent {
    Num(u64),
    Str(String),
}

fn semver_sort_key(ver: &str) -> Option<(u64, u64, u64, bool, Vec<SemverPreIdent>)> {
    let v = ver.trim().trim_start_matches('v').trim_start_matches('V');
    let caps = semver_re().captures(v)?;
    let maj: u64 = caps[1].parse().ok()?;
    let min: u64 = caps[2].parse().ok()?;
    let pat: u64 = caps[3].parse().ok()?;
    let pre_raw = caps.get(4).map(|m| m.as_str()).unwrap_or("");
    let pre_parts: Vec<SemverPreIdent> = if pre_raw.is_empty() {
        vec![]
    } else {
        pre_raw
            .split('.')
            .map(|seg| match seg.parse::<u64>() {
                Ok(n) => SemverPreIdent::Num(n),
                Err(_) => SemverPreIdent::Str(seg.to_string()),
            })
            .collect()
    };
    Some((maj, min, pat, pre_parts.is_empty(), pre_parts))
}

/// Compares two SemVer 2.0.0 version strings (with optional leading `v`/`V`).
pub fn compare_semver_str(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let ka = semver_sort_key(a)?;
    let kb = semver_sort_key(b)?;
    Some(ka.cmp(&kb))
}

/// Selects the highest valid SemVer 2.0.0 among all readings in `state` (numeric prerelease
/// identifiers compare numerically so `1.0.0-beta.10` > `1.0.0-beta.2`, and stable > prerelease).
pub fn highest_valid_version(state: &VersionState) -> Option<String> {
    let mut best: Option<(String, (u64, u64, u64, bool, Vec<SemverPreIdent>))> = None;
    for r in &state.readings {
        if !r.exists || r.error.is_some() || r.version.is_empty() {
            continue;
        }
        let v = r.version.trim();
        let Some(key) = semver_sort_key(v) else {
            if best.is_none() {
                best = Some((v.to_string(), (0, 0, 0, false, vec![])));
            }
            continue;
        };
        match &best {
            None => best = Some((v.to_string(), key)),
            Some((_, bk)) if key > *bk => best = Some((v.to_string(), key)),
            _ => {}
        }
    }
    best.map(|(v, _)| v)
}

fn extract_cargo_package_name(raw: &str) -> Option<String> {
    let mut in_pkg = false;
    for line in raw.lines() {
        if let Some(hdr) = parse_toml_header(line) {
            in_pkg = hdr == "package";
            continue;
        }
        if in_pkg {
            if let Some(caps) = toml_name_re().captures(line) {
                return Some(caps[1].to_string());
            }
        }
    }
    None
}

/// Updates ONLY the local (non-registry, i.e. no `source = ...`) `[[package]]` entry matching
/// `pkg_name` in `Cargo.lock`, preserving original line endings and leaving all third-party crates untouched.
pub fn replace_cargo_lock_package_version(
    raw: &str,
    pkg_name: &str,
    new_version: &str,
) -> Option<String> {
    if pkg_name.trim().is_empty() {
        return None;
    }
    let line_ending = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let ends_with_newline = raw.ends_with('\n');
    let lines: Vec<&str> = raw.lines().collect();

    // Locate `[[package]]` block ranges
    let mut block_starts: Vec<usize> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if line.trim() == "[[package]]" {
            block_starts.push(idx);
        }
    }
    if block_starts.is_empty() {
        return None;
    }

    let mut target_ver_line: Option<usize> = None;
    for (bi, &start) in block_starts.iter().enumerate() {
        let end = block_starts.get(bi + 1).copied().unwrap_or(lines.len());
        let mut matched_name = false;
        let mut has_source = false;
        let mut ver_idx: Option<usize> = None;

        for idx in (start + 1)..end {
            let trimmed = lines[idx].trim();
            if trimmed.starts_with('[') {
                break;
            }
            if let Some(caps) = toml_name_re().captures(lines[idx]) {
                if &caps[1] == pkg_name {
                    matched_name = true;
                }
            } else if trimmed.starts_with("source ") || trimmed.starts_with("source=") {
                has_source = true;
            } else if ver_idx.is_none() && toml_ver_re().is_match(lines[idx]) {
                ver_idx = Some(idx);
            }
        }

        if matched_name && !has_source {
            target_ver_line = ver_idx;
            break;
        }
    }

    let v_idx = target_ver_line?;
    let caps = toml_ver_re().captures(lines[v_idx])?;
    if &caps[2] == new_version {
        return None;
    }

    let mut out_lines: Vec<String> = lines.iter().map(|s| (*s).to_string()).collect();
    out_lines[v_idx] = format!("{}{}{}", &caps[1], new_version, &caps[3]);
    let mut joined = out_lines.join(line_ending);
    if ends_with_newline {
        joined.push_str(line_ending);
    }
    Some(joined)
}

/// Returns existing sibling/workspace `Cargo.lock` paths associated with any `cargo_toml` version specs
/// so both atomic version writes and transaction rollback backups cover them.
pub fn cargo_lock_paths_for_specs(root: &Path, specs: &[VersionFileSpec]) -> Vec<PathBuf> {
    let mut locks: Vec<PathBuf> = Vec::new();
    for spec in specs {
        if spec.format != "cargo_toml" {
            continue;
        }
        let Ok(full) = crate::guardrails::resolve_confined_path(root, &spec.path) else {
            continue;
        };
        let candidates = [
            full.parent().map(|p| p.join("Cargo.lock")),
            Some(root.join("Cargo.lock")),
        ];
        for cand in candidates.into_iter().flatten() {
            if cand.exists() && cand.is_file() && !locks.iter().any(|x| x == &cand) {
                locks.push(cand);
            }
        }
    }
    locks
}

/// Atomic multi-file write with rollback on partial failure.
/// Also atomically syncs the matching local crate version inside sibling/root `Cargo.lock`
/// so subsequent `cargo test`/`cargo build` runs never re-dirty `Cargo.lock`.
pub fn write_project_versions(
    root: &Path,
    specs: &[VersionFileSpec],
    new_version: &str,
) -> Result<VersionState> {
    validate_semver(new_version)?;

    let mut pending: Vec<(PathBuf, Vec<u8>, String)> = Vec::new();
    let mut cargo_pkg_names: Vec<(PathBuf, String)> = Vec::new();

    for spec in specs {
        let full = crate::guardrails::resolve_confined_path(root, &spec.path)
            .with_context(|| format!("版本文件路径非法：{}", spec.path))?;
        if !full.exists() {
            continue;
        }
        let orig_bytes = fs::read(&full)
            .with_context(|| format!("读取版本文件失败：{}", spec.path))?;
        let raw = String::from_utf8(orig_bytes.clone())
            .with_context(|| format!("版本文件非 UTF-8 编码：{}", spec.path))?;
        let updated = replace_version_in_text(&raw, &spec.format, &spec.pattern, new_version)
            .with_context(|| format!("更新版本文件失败：{}", spec.path))?;
        if spec.format == "cargo_toml" {
            if let Some(pkg_name) = extract_cargo_package_name(&raw) {
                cargo_pkg_names.push((full.clone(), pkg_name));
            }
        }
        pending.push((full, orig_bytes, updated));
    }

    if pending.is_empty() {
        return Err(anyhow!("未找到可写入的版本文件"));
    }

    // Sync local crate version in sibling/workspace Cargo.lock (best-effort, same atomic rollback pool)
    let mut seen_locks: Vec<PathBuf> = Vec::new();
    for (cargo_toml_path, pkg_name) in cargo_pkg_names {
        let candidates = [
            cargo_toml_path.parent().map(|p| p.join("Cargo.lock")),
            Some(root.join("Cargo.lock")),
        ];
        for lock_path in candidates.into_iter().flatten() {
            if !lock_path.exists()
                || !lock_path.is_file()
                || seen_locks.iter().any(|x| x == &lock_path)
            {
                continue;
            }
            if let Ok(lock_bytes) = fs::read(&lock_path) {
                if let Ok(lock_raw) = String::from_utf8(lock_bytes.clone()) {
                    if let Some(lock_updated) =
                        replace_cargo_lock_package_version(&lock_raw, &pkg_name, new_version)
                    {
                        seen_locks.push(lock_path.clone());
                        pending.push((lock_path, lock_bytes, lock_updated));
                    }
                }
            }
        }
    }

    let mut written: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for (path, orig_bytes, content) in pending {
        if let Err(e) = crate::guardrails::atomic_write_bytes(&path, content.as_bytes()) {
            // Rollback previously written files so version sources stay consistent
            for (rb_path, rb_bytes) in written.into_iter().rev() {
                let _ = crate::guardrails::atomic_write_bytes(&rb_path, &rb_bytes);
            }
            return Err(anyhow!("写入文件失败（已回滚）：{} ({})", path.display(), e));
        }
        written.push((path, orig_bytes));
    }

    Ok(read_project_versions(root, specs))
}

pub fn read_changelog(root: &Path, rel_path: &str) -> ChangelogData {
    let effective_rel = if rel_path.trim().is_empty() {
        "CHANGELOG.md"
    } else {
        rel_path.trim()
    };
    let full = match crate::guardrails::resolve_confined_path(root, effective_rel) {
        Ok(p) => p,
        Err(_) => {
            return ChangelogData {
                path: effective_rel.to_string(),
                exists: false,
                entries: vec![],
                raw_head: String::new(),
            };
        }
    };

    if !full.is_file() {
        return ChangelogData {
            path: effective_rel.to_string(),
            exists: false,
            entries: vec![],
            raw_head: String::new(),
        };
    }

    let raw = match fs::read_to_string(&full) {
        Ok(s) => s,
        Err(_) => {
            return ChangelogData {
                path: effective_rel.to_string(),
                exists: false,
                entries: vec![],
                raw_head: String::new(),
            };
        }
    };

    let entries = parse_changelog_entries(&raw);
    let raw_head: String = raw.chars().take(4000).collect();

    ChangelogData {
        path: effective_rel.to_string(),
        exists: true,
        entries,
        raw_head,
    }
}

pub fn parse_changelog_entries(raw: &str) -> Vec<ChangelogEntry> {
    let mut entries: Vec<ChangelogEntry> = Vec::new();
    let mut in_code_fence = false;
    let mut cur_ver: Option<String> = None;
    let mut cur_level: usize = 0;
    let mut cur_heading = String::new();
    let mut cur_lines: Vec<String> = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
        }
        let heading_level = if !in_code_fence {
            if line.starts_with("## ") {
                2
            } else if line.starts_with("### ") {
                3
            } else {
                0
            }
        } else {
            0
        };
        if heading_level > 0 {
            let can_open_or_switch = cur_ver.is_none() || heading_level <= cur_level;
            if can_open_or_switch {
                if let Some(caps) = changelog_heading_re().captures(line) {
                    if let Some(v) = cur_ver.take() {
                        entries.push(ChangelogEntry {
                            version: v,
                            heading: cur_heading.clone(),
                            body: cur_lines.join("\n").trim().to_string(),
                        });
                        cur_lines.clear();
                    }
                    cur_ver = Some(caps[1].to_string());
                    cur_level = heading_level;
                    cur_heading = line.trim().to_string();
                    cur_lines.push(line.to_string());
                    continue;
                } else if heading_level == 2 {
                    if let Some(v) = cur_ver.take() {
                        entries.push(ChangelogEntry {
                            version: v,
                            heading: cur_heading.clone(),
                            body: cur_lines.join("\n").trim().to_string(),
                        });
                        cur_lines.clear();
                    }
                    cur_level = 0;
                    continue;
                }
            }
        }
        if cur_ver.is_some() {
            cur_lines.push(line.to_string());
        }
    }

    if let Some(v) = cur_ver.take() {
        entries.push(ChangelogEntry {
            version: v,
            heading: cur_heading,
            body: cur_lines.join("\n").trim().to_string(),
        });
    }

    entries
}

fn heading_has_substantive_title(line: &str) -> bool {
    let Some(caps) = changelog_heading_re().captures(line) else {
        return false;
    };
    let rest = line[caps[0].len()..].trim();
    if rest.is_empty() {
        return false;
    }
    // Strip optional date / channel / semver-kind parens like "(2026-11-26)", "（patch）", "- 2026-11-26"
    let mut cleaned = rest.to_string();
    for pat in [
        "（patch）", "（minor）", "（major）", "（beta）", "(patch)", "(minor)", "(major)", "(beta)",
    ] {
        cleaned = cleaned.replace(pat, " ");
    }
    let stripped: String = cleaned
        .chars()
        .filter(|c| !c.is_ascii_digit() && !matches!(c, '-' | '/' | '.' | '(' | ')' | '（' | '）' | '[' | ']' | '【' | '】' | '—' | ':' | '：' | ' '))
        .collect();
    !stripped.is_empty()
}

pub fn is_bare_default_release_note(notes: &str) -> bool {
    let t = notes.trim();
    if t.is_empty() {
        return true;
    }
    // Strip optional leading bare version/unreleased heading(s) and blank lines so stubs like:
    //   "## 1.47.3 (2026-11-26)\n\nQDisk v1.47.3 正式发布。"
    // are recognized as bare placeholders rather than real changelog notes.
    let mut body_lines: Vec<&str> = Vec::new();
    let mut skipping_head = true;
    let mut heading_had_title = false;
    for raw_line in t.lines() {
        let l = raw_line.trim();
        if skipping_head {
            if l.is_empty() || l == "---" || unreleased_heading_re().is_match(l) {
                continue;
            }
            if changelog_heading_re().is_match(l) {
                if heading_has_substantive_title(l) {
                    heading_had_title = true;
                }
                continue;
            }
            skipping_head = false;
        }
        if !l.is_empty() {
            body_lines.push(l);
        }
    }
    if heading_had_title && !body_lines.is_empty() {
        return false;
    }
    if body_lines.is_empty() {
        return true;
    }
    if body_lines.len() == 1 {
        let single = body_lines[0].trim_start_matches(['-', '*', '•', ' ']).trim();
        let low = single.to_ascii_lowercase();
        if matches!(
            single,
            "常规优化与问题修复。"
                | "常规优化与问题修复"
                | "常规更新与优化。"
                | "常规更新与优化"
                | "更新了版本"
                | "更新了版本。"
                | "更新版本"
                | "更新版本。"
        ) {
            return true;
        }
        if single.chars().count() <= 80
            && (single.ends_with("正式发布。")
                || single.ends_with("正式发布")
                || single.ends_with("版本发布。")
                || low.ends_with(" released.")
                || low.ends_with(" release."))
        {
            return true;
        }
    }
    false
}

pub fn sanitize_public_release_notes(raw: &str) -> String {
    let mut txt = raw.replace("\r\n", "\n").trim().to_string();
    txt = url_cred_re().replace_all(&txt, "${1}***@").to_string();
    txt = secret_token_re().replace_all(&txt, "***").to_string();
    txt = user_profile_path_re()
        .replace_all(&txt, "<user-dir>")
        .to_string();
    if txt.chars().count() > 2400 {
        txt = txt.chars().take(2400).collect::<String>().trim_end().to_string() + "\n…";
    }
    txt
}

pub fn extract_changelog_entry_exact(
    root: &Path,
    changelog_rel: &str,
    version: &str,
) -> Option<String> {
    let ver = version.trim().trim_start_matches('v').trim_start_matches('V');
    if ver.is_empty() {
        return None;
    }
    let data = read_changelog(root, changelog_rel);
    // Use rfind so chronological append-at-bottom changelogs (like 变更记录.md) pick the latest entry if duplicated
    let entry = data
        .entries
        .into_iter()
        .rfind(|e| e.version.eq_ignore_ascii_case(ver))?;
    let body = sanitize_public_release_notes(&entry.body);
    // Reject heading-only empty stub entries AND bare default placeholders ("QDisk vX.Y.Z 正式发布。")
    if body.is_empty() || is_bare_default_release_note(&body) {
        return None;
    }
    Some(body)
}

fn candidate_changelog_paths(changelog_rel: &str) -> Vec<String> {
    let mut list = Vec::new();
    let trimmed = changelog_rel.trim();
    if !trimmed.is_empty() {
        list.push(trimmed.to_string());
    }
    for cand in [
        "CHANGELOG.md",
        "变更记录.md",
        "RELEASES.md",
        "HISTORY.md",
        "docs/CHANGELOG.md",
        "docs/变更记录.md",
    ] {
        if !list.iter().any(|x| x.eq_ignore_ascii_case(cand)) {
            list.push(cand.to_string());
        }
    }
    list
}

/// Extracts and aggregates all substantive (non-placeholder) changelog entries strictly newer than
/// `after_version` (e.g. previous published Git release tag) and `<= up_to_version`.
/// Returns `(combined_notes, matched_versions_newest_first, file_name)`.
pub fn extract_changelog_entries_since(
    root: &Path,
    changelog_rel: &str,
    after_version: Option<&str>,
    up_to_version: &str,
) -> Option<(String, Vec<String>, String)> {
    let target = up_to_version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    let target_key = semver_sort_key(target)?;
    let after_key = after_version.and_then(semver_sort_key);
    // Require a concrete lower bound so we never blindly scoop historical already-published releases.
    let min_exclusive = after_key?;

    for rel in candidate_changelog_paths(changelog_rel) {
        let data = read_changelog(root, &rel);
        if !data.exists || data.entries.is_empty() {
            continue;
        }
        // Keep the latest occurrence per version (matching rfind semantics for ascending changelogs)
        let mut by_ver: Vec<(String, (u64, u64, u64, bool, Vec<SemverPreIdent>), String)> = Vec::new();
        for e in data.entries {
            let Some(k) = semver_sort_key(&e.version) else {
                continue;
            };
            if k <= min_exclusive || k > target_key {
                continue;
            }
            let body = sanitize_public_release_notes(&e.body);
            if body.is_empty() || is_bare_default_release_note(&body) {
                continue;
            }
            if let Some(pos) = by_ver.iter().position(|(v, _, _)| v.eq_ignore_ascii_case(&e.version)) {
                by_ver[pos] = (e.version, k, body);
            } else {
                by_ver.push((e.version, k, body));
            }
        }
        if by_ver.is_empty() {
            continue;
        }
        by_ver.sort_by(|a, b| b.1.cmp(&a.1));
        let picked: Vec<_> = by_ver.into_iter().take(5).collect();
        let vers: Vec<String> = picked.iter().map(|(v, _, _)| v.clone()).collect();
        let combined = sanitize_public_release_notes(
            &picked
                .into_iter()
                .map(|(_, _, b)| b)
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
        if combined.is_empty() || is_bare_default_release_note(&combined) {
            continue;
        }
        let fname = Path::new(&rel)
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("CHANGELOG.md")
            .to_string();
        return Some((combined, vers, fname));
    }
    None
}

/// Returns the newest substantive (non-placeholder) changelog `(version, notes)` entry,
/// used to inform the user and offer one-click import when the target version has no notes yet.
pub fn extract_latest_substantive_changelog_entry(
    root: &Path,
    changelog_rel: &str,
) -> Option<(String, String)> {
    for rel in candidate_changelog_paths(changelog_rel) {
        let data = read_changelog(root, &rel);
        if !data.exists || data.entries.is_empty() {
            continue;
        }
        let mut best: Option<(String, (u64, u64, u64, bool, Vec<SemverPreIdent>), String)> = None;
        for e in data.entries {
            let Some(k) = semver_sort_key(&e.version) else {
                continue;
            };
            let body = sanitize_public_release_notes(&e.body);
            if body.is_empty() || is_bare_default_release_note(&body) {
                continue;
            }
            match &best {
                None => best = Some((e.version, k, body)),
                Some((_, bk, _)) if k >= *bk => best = Some((e.version, k, body)),
                _ => {}
            }
        }
        if let Some((ver, _, body)) = best {
            return Some((ver, body));
        }
    }
    None
}

pub fn extract_unreleased_changelog_notes(root: &Path, changelog_rel: &str) -> Option<String> {
    let full = crate::guardrails::resolve_confined_path(root, changelog_rel).ok()?;
    if !full.is_file() {
        return None;
    }
    let raw = fs::read_to_string(&full).ok()?;
    let mut in_code_fence = false;
    let mut capturing = false;
    let mut lines: Vec<String> = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
        }
        if !in_code_fence && line.starts_with("## ") {
            if capturing {
                break;
            }
            if unreleased_heading_re().is_match(line) {
                capturing = true;
                continue;
            }
        } else if capturing {
            lines.push(line.to_string());
        }
    }

    let body = sanitize_public_release_notes(&lines.join("\n"));
    if body.is_empty() || is_bare_default_release_note(&body) {
        None
    } else {
        Some(body)
    }
}

/// Search for a dedicated per-version release notes Markdown file such as
/// `RELEASE_v1.43.18.md`, `docs/01-当前版/RELEASE_v1.43.18.md`, or `releases/v1.43.18.md`.
pub fn extract_dedicated_release_doc_notes(
    root: &Path,
    changelog_rel: &str,
    version: &str,
) -> Option<(String, String)> {
    let ver = version.trim().trim_start_matches('v').trim_start_matches('V');
    if ver.is_empty() || validate_semver(ver).is_err() {
        return None;
    }

    let mut search_dirs: Vec<String> = vec![String::new()];
    if let Some(parent) = Path::new(changelog_rel.trim()).parent() {
        let p = parent.to_string_lossy().replace('\\', "/");
        if !p.is_empty() && p != "." && !search_dirs.iter().any(|x| x.eq_ignore_ascii_case(&p)) {
            search_dirs.push(p);
        }
    }
    for d in ["docs", "docs/releases", "releases", "release-notes", "changelogs"] {
        if !search_dirs.iter().any(|x| x.eq_ignore_ascii_case(d)) {
            search_dirs.push(d.to_string());
        }
    }

    let file_patterns = [
        format!("RELEASE_v{}.md", ver),
        format!("RELEASE_{}.md", ver),
        format!("release-v{}.md", ver),
        format!("release_{}.md", ver),
        format!("v{}.md", ver),
        format!("{}.md", ver),
    ];

    for dir_rel in search_dirs {
        for fname in &file_patterns {
            let rel = if dir_rel.is_empty() {
                fname.clone()
            } else {
                format!("{}/{}", dir_rel, fname)
            };
            let Ok(full) = crate::guardrails::resolve_confined_path(root, &rel) else {
                continue;
            };
            if !full.is_file() {
                continue;
            }
            if let Ok(raw) = fs::read_to_string(&full) {
                let body = sanitize_public_release_notes(&raw);
                if !body.is_empty() && !is_bare_default_release_note(&body) {
                    return Some((body, rel));
                }
            }
        }
    }
    None
}

/// Extract version-specific or changelog-section release notes from `README.md` (or Chinese README variants).
/// Strictly ignores general marketing/intro README sections so promo banners/badges never pollute release notes!
pub fn extract_notes_from_readme(root: &Path, version: &str) -> Option<(String, String)> {
    let ver = version.trim().trim_start_matches('v').trim_start_matches('V');
    if ver.is_empty() {
        return None;
    }

    for candidate in ["README.md", "README.zh.md", "README_CN.md", "README.zh-CN.md"] {
        let Ok(full) = crate::guardrails::resolve_confined_path(root, candidate) else {
            continue;
        };
        if !full.is_file() {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&full) else {
            continue;
        };

        // 1. Exact version heading anywhere in README (`## v1.2.3` or `### 1.2.3`, outside code fences)
        let entries = parse_changelog_entries(&raw);
        if let Some(entry) = entries.into_iter().rfind(|e| e.version.eq_ignore_ascii_case(ver)) {
            let body = sanitize_public_release_notes(&entry.body);
            let has_content = body.lines().skip(1).any(|l| !l.trim().is_empty());
            if !body.is_empty() && has_content && !is_bare_default_release_note(&body) {
                return Some((body, candidate.to_string()));
            }
        }

        // 2. Look inside an explicit Changelog / 更新日志 H2 section in README for an inline bullet for `ver`
        //    or an Unreleased / 待发布 block
        let mut in_fence = false;
        let mut in_cl_sec = false;
        let mut sec_lines: Vec<String> = Vec::new();
        for line in raw.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                in_fence = !in_fence;
            }
            if !in_fence && line.starts_with("## ") {
                if in_cl_sec {
                    break;
                }
                if readme_changelog_section_re().is_match(line) {
                    in_cl_sec = true;
                    continue;
                }
            } else if in_cl_sec {
                sec_lines.push(line.to_string());
            }
        }

        if !sec_lines.is_empty() {
            // Check inline bullet matching `- **v1.2.3**` or `- v1.2.3` inside the Changelog section
            let ver_lower = ver.to_ascii_lowercase();
            let mut matched_bullets = Vec::new();
            let mut capturing_indent: Option<usize> = None;
            for line in &sec_lines {
                let indent = line.len() - line.trim_start().len();
                let t = line.trim();
                if let Some(base_indent) = capturing_indent {
                    if t.is_empty() {
                        continue;
                    }
                    if indent > base_indent {
                        matched_bullets.push(line.clone());
                        continue;
                    } else {
                        break;
                    }
                }
                let t_low = t.to_ascii_lowercase();
                if (t_low.starts_with("- ") || t_low.starts_with("* "))
                    && (t_low.contains(&format!("v{}", ver_lower))
                        || t_low.contains(&format!("**{}**", ver_lower))
                        || t_low.contains(&format!("[{}]", ver_lower))
                        || t_low.contains(&format!("【{}】", ver_lower)))
                {
                    matched_bullets.push(line.clone());
                    capturing_indent = Some(indent);
                }
            }
            if !matched_bullets.is_empty() {
                let body = sanitize_public_release_notes(&matched_bullets.join("\n"));
                if !body.is_empty() && !is_bare_default_release_note(&body) {
                    return Some((body, format!("{} 更新日志区", candidate)));
                }
            }
        }
    }
    None
}

#[cfg(test)]
pub fn extract_notes_for_version(
    root: &Path,
    changelog_rel: &str,
    version: &str,
    product_name: &str,
) -> String {
    if let Some(txt) = extract_changelog_entry_exact(root, changelog_rel, version) {
        return txt;
    }
    let name = if product_name.is_empty() {
        "Software"
    } else {
        product_name
    };
    format!("{} v{} 正式发布。", name, version)
}

#[allow(dead_code)]
pub fn upsert_changelog_entry(
    root: &Path,
    rel_path: &str,
    version: &str,
    body: &str,
) -> Result<String> {
    upsert_changelog_entry_with_summary(root, rel_path, version, "", body)
}

pub fn upsert_changelog_entry_with_summary(
    root: &Path,
    rel_path: &str,
    version: &str,
    summary: &str,
    body: &str,
) -> Result<String> {
    let ver = version.trim().trim_start_matches('v').trim_start_matches('V');
    validate_semver(ver)?;
    let clean_body = body.trim().replace("\r\n", "\n");
    if clean_body.is_empty() {
        return Err(anyhow!("更新内容不能为空，无法写入变更记录"));
    }
    if summary.trim().is_empty() && is_bare_default_release_note(&clean_body) {
        return Err(anyhow!(
            "当前仅为默认占位文案，请先补充实质更新内容后再写入变更记录"
        ));
    }

    let target_rel = if rel_path.trim().is_empty() {
        "CHANGELOG.md"
    } else {
        rel_path.trim()
    };
    let full = crate::guardrails::resolve_confined_path(root, target_rel)
        .with_context(|| format!("变更日志路径越界或非法：{}", target_rel))?;

    // Avoid duplicating a `## <ver>` heading if user text already starts with one for this version
    let first_line = clean_body.lines().next().unwrap_or("").trim();
    let already_has_ver_heading = changelog_heading_re()
        .captures(first_line)
        .map(|c| c[1].eq_ignore_ascii_case(ver))
        .unwrap_or(false);
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let clean_summary = summary.trim();
    let block_lf = if already_has_ver_heading {
        format!("{}\n", clean_body)
    } else if !clean_summary.is_empty() {
        format!("## {} ({}) —— {}\n\n{}\n", ver, date, clean_summary, clean_body)
    } else {
        format!("## {} ({})\n\n{}\n", ver, date, clean_body)
    };

    if !full.exists() {
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = format!("# 变更记录\n\n{}", block_lf);
        crate::guardrails::atomic_write_bytes(&full, content.as_bytes())?;
        return Ok(target_rel.to_string());
    }

    let existing = fs::read_to_string(&full)?;
    // Preserve untouched historical lines byte-for-byte (never convert unrelated CRLF lines to LF!)
    let raw_lines: Vec<&str> = existing.split_inclusive('\n').collect();
    let crlf_count = raw_lines.iter().filter(|l| l.ends_with("\r\n")).count();
    let lf_only_count = raw_lines.len().saturating_sub(crlf_count);
    let eol = if crlf_count > lf_only_count { "\r\n" } else { "\n" };
    let block = if eol == "\r\n" {
        block_lf.replace('\n', "\r\n")
    } else {
        block_lf
    };

    // 1. Check if an entry for `ver` ALREADY exists (find the last occurrence, matching rfind extraction)
    let mut in_fence = false;
    let mut match_span: Option<(usize, usize)> = None;
    let mut cur_span: Option<(usize, usize)> = None; // (start_idx, heading_level)
    let mut all_vers: Vec<String> = Vec::new();
    let mut first_h2_idx: Option<usize> = None;

    for (idx, raw_line) in raw_lines.iter().enumerate() {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
        }
        let h_level = if !in_fence {
            if line.starts_with("## ") {
                if first_h2_idx.is_none() {
                    first_h2_idx = Some(idx);
                }
                2
            } else if line.starts_with("### ") {
                3
            } else {
                0
            }
        } else {
            0
        };
        if h_level > 0 {
            if let Some((start_idx, start_lvl)) = cur_span {
                if h_level <= start_lvl {
                    match_span = Some((start_idx, idx));
                    cur_span = None;
                }
            }
            if h_level == 2 || (h_level == 3 && cur_span.is_none()) {
                if let Some(caps) = changelog_heading_re().captures(line) {
                    let v = caps[1].to_string();
                    all_vers.push(v.clone());
                    if v.eq_ignore_ascii_case(ver) {
                        cur_span = Some((idx, h_level));
                    }
                }
            }
        }
    }
    if let Some((start_idx, _)) = cur_span {
        match_span = Some((start_idx, raw_lines.len()));
    }

    let updated = if let Some((s, e)) = match_span {
        let mut out = String::with_capacity(existing.len() + block.len() + 16);
        for l in &raw_lines[..s] {
            out.push_str(l);
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(eol);
        }
        out.push_str(&block);
        if e < raw_lines.len() {
            out.push_str(eol);
            for l in &raw_lines[e..] {
                out.push_str(l);
            }
        }
        out
    } else {
        // 2. Auto-detect chronological direction (ascending bottom-append like QDisk 变更记录.md vs descending top-prepend CHANGELOG.md)
        let is_ascending = existing.contains("越往下越新")
            || (all_vers.len() >= 2
                && match (
                    semver_sort_key(all_vers.first().unwrap()),
                    semver_sort_key(all_vers.last().unwrap()),
                ) {
                    (Some(a), Some(b)) => a < b,
                    _ => false,
                });
        if is_ascending {
            let trimmed_existing = existing.trim_end_matches(['\r', '\n']);
            format!("{}{}{}{}{}", trimmed_existing, eol, eol, eol, block)
        } else if let Some(idx) = first_h2_idx {
            let mut out = String::with_capacity(existing.len() + block.len() + 16);
            for l in &raw_lines[..idx] {
                out.push_str(l);
            }
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(eol);
            }
            out.push_str(&block);
            out.push_str(eol);
            for l in &raw_lines[idx..] {
                out.push_str(l);
            }
            out
        } else {
            let trimmed_existing = existing.trim_end_matches(['\r', '\n']);
            format!("{}{}{}{}", trimmed_existing, eol, eol, block)
        }
    };

    crate::guardrails::atomic_write_bytes(&full, updated.as_bytes())?;
    Ok(target_rel.to_string())
}

#[allow(dead_code)]
pub fn prepend_changelog_entry(
    root: &Path,
    rel_path: &str,
    version: &str,
    summary: &str,
    body: &str,
) -> Result<()> {
    let effective_body = if body.trim().is_empty() {
        "- 常规更新与优化。"
    } else {
        body.trim()
    };
    upsert_changelog_entry_with_summary(root, rel_path, version, summary, effective_body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bump_semver() {
        assert_eq!(bump_semver("1.46.0", "patch").unwrap(), "1.46.1");
        assert_eq!(bump_semver("1.46.0", "minor").unwrap(), "1.47.0");
        assert_eq!(bump_semver("1.46.0", "major").unwrap(), "2.0.0");
        assert_eq!(bump_semver("1.46.0", "beta").unwrap(), "1.46.1-beta.1");
        assert_eq!(bump_semver("1.46.1-beta.1", "beta").unwrap(), "1.46.1-beta.2");
        assert_eq!(bump_semver("1.46.1-beta.2", "patch").unwrap(), "1.46.1");
    }

    #[test]
    fn test_cargo_toml_handles_comments_and_workspace() {
        let src = r#"[package] # main crate
name = "demo"
version = "1.2.3"
edition = "2021"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
"#;
        assert_eq!(extract_version_from_text(src, "cargo_toml", "").unwrap(), "1.2.3");
        let out = replace_version_in_text(src, "cargo_toml", "", "1.3.0").unwrap();
        assert!(out.contains("version = \"1.3.0\""));
        assert!(out.contains("serde = { version = \"1.0\""));

        let ws = "[workspace.package]\nversion = \"2.0.0\"\n";
        assert_eq!(extract_version_from_text(ws, "cargo_toml", "").unwrap(), "2.0.0");
        let ws_out = replace_version_in_text(ws, "cargo_toml", "", "2.1.0").unwrap();
        assert!(ws_out.contains("version = \"2.1.0\""));
    }

    #[test]
    fn test_json_scoped_version_never_hits_nested_fields() {
        let pkg = r#"{
  "name": "web-app",
  "meta": { "version": "9.9.9" },
  "version": "1.2.0"
}"#;
        let out = replace_version_in_text(pkg, "package_json", "", "1.3.0").unwrap();
        assert!(out.contains(r#""meta": { "version": "9.9.9" }"#));
        assert!(out.contains(r#""version": "1.3.0""#));

        // Tauri v1 nested package.version support
        let t1 = r#"{
  "build": { "devPath": "../dist" },
  "package": { "productName": "OldApp", "version": "0.8.0" }
}"#;
        assert_eq!(extract_version_from_text(t1, "tauri_json", "").unwrap(), "0.8.0");
        let t1_out = replace_version_in_text(t1, "tauri_json", "", "0.9.0").unwrap();
        assert!(t1_out.contains(r#""version": "0.9.0""#));
    }

    #[test]
    fn test_pubspec_preserves_build_number() {
        let yaml = "name: app\nversion: 1.5.2+42\n";
        assert_eq!(extract_version_from_text(yaml, "pubspec_yaml", "").unwrap(), "1.5.2");
        let out = replace_version_in_text(yaml, "pubspec_yaml", "", "1.6.0").unwrap();
        assert!(out.contains("version: 1.6.0+42"));
    }

    #[test]
    fn test_parse_changelog_ignores_code_fence() {
        let md = "# 变更记录\n\n## 1.46.0（minor）—— 新功能\n- 第一条\n```md\n## 9.9.9 假标题\n```\n- 第二条\n\n## 1.45.0 — 旧版本\n- 旧功能\n";
        let list = parse_changelog_entries(md);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].version, "1.46.0");
        assert!(list[0].body.contains("第二条"));
        assert_eq!(list[1].version, "1.45.0");
    }

    #[test]
    fn test_write_project_versions_syncs_local_cargo_lock_only() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_lock_sync_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(1)
        ));
        let tauri_dir = base.join("src").join("src-tauri");
        fs::create_dir_all(&tauri_dir).unwrap();

        fs::write(
            tauri_dir.join("Cargo.toml"),
            "[package]\nname = \"my-app\"\nversion = \"1.2.3\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(
            tauri_dir.join("Cargo.lock"),
            "version = 3\n\n[[package]]\nname = \"my-app\"\nversion = \"0.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[[package]]\nname = \"my-app\"\nversion = \"1.2.3\"\ndependencies = []\n",
        )
        .unwrap();

        let specs = vec![VersionFileSpec {
            path: "src/src-tauri/Cargo.toml".into(),
            format: "cargo_toml".into(),
            pattern: String::new(),
        }];
        let st = write_project_versions(&base, &specs, "1.3.0").unwrap();
        assert_eq!(st.current, "1.3.0");

        let lock_after = fs::read_to_string(tauri_dir.join("Cargo.lock")).unwrap();
        // Registry package with same name stays 0.1.0; local package updates to 1.3.0
        assert!(lock_after.contains("name = \"my-app\"\nversion = \"0.1.0\"\nsource = \"registry+"));
        assert!(lock_after.contains("name = \"my-app\"\nversion = \"1.3.0\"\ndependencies = []"));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_version_and_changelog_block_path_traversal() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_ver_rt_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(31)
        ));
        let proj = base.join("proj");
        fs::create_dir_all(&proj).unwrap();
        let outside = base.join("outside.md");
        fs::write(&outside, "## 1.0.0\nsecret\n").unwrap();

        // read_changelog must refuse traversal & absolute paths
        let cl_trav = read_changelog(&proj, "../outside.md");
        assert!(!cl_trav.exists);
        let cl_abs = read_changelog(&proj, &outside.to_string_lossy());
        assert!(!cl_abs.exists);

        // prepend_changelog_entry must refuse traversal & absolute paths
        assert!(prepend_changelog_entry(&proj, "../outside.md", "1.1.0", "x", "- y").is_err());
        assert!(
            prepend_changelog_entry(&proj, &outside.to_string_lossy(), "1.1.0", "x", "- y")
                .is_err()
        );

        // write_project_versions must refuse traversal specs
        let bad_specs = vec![VersionFileSpec {
            path: "../Cargo.toml".into(),
            format: "cargo_toml".into(),
            pattern: String::new(),
        }];
        assert!(write_project_versions(&proj, &bad_specs, "1.1.0").is_err());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_read_project_versions_missing_file_never_reports_synced_and_highest_semver_numeric() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_ver_sync_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(51)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("package.json"), r#"{"version":"1.2.0-beta.2"}"#).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"1.2.0-beta.10\"\n").unwrap();

        // Missing 3rd file MUST make synced = false even if valid files existed
        let specs_missing = vec![
            VersionFileSpec { path: "package.json".into(), format: "package_json".into(), pattern: String::new() },
            VersionFileSpec { path: "missing.toml".into(), format: "cargo_toml".into(), pattern: String::new() },
        ];
        let st_missing = read_project_versions(&dir, &specs_missing);
        assert!(!st_missing.synced, "Missing version file must NEVER report synced=true");

        // Numeric prerelease ordering: 1.2.0-beta.10 > 1.2.0-beta.2!
        let specs_pre = vec![
            VersionFileSpec { path: "package.json".into(), format: "package_json".into(), pattern: String::new() },
            VersionFileSpec { path: "Cargo.toml".into(), format: "cargo_toml".into(), pattern: String::new() },
        ];
        let st_pre = read_project_versions(&dir, &specs_pre);
        assert!(!st_pre.synced);
        assert_eq!(highest_valid_version(&st_pre).as_deref(), Some("1.2.0-beta.10"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_changelog_exact_unreleased_and_sanitize_preserves_safe_paths() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_notes_ver_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(53)
        ));
        fs::create_dir_all(&dir).unwrap();
        let md = "# 变更记录\n\n## [待发布]\n- 新增自动读取更新日志能力\n- 修复 `C:\\Windows\\Temp` 清理异常\n\n## 【1.2.3】（patch）—— 重要修复\n- 修复在线更新下载校验（路径 C:\\Users\\Alice\\Desktop\\secret.txt 与 https://me:pass123@example.com/api 与 ghp_1234567890abcdefghij123456）\n\n## 1.2.4\n\n## 1.2.5\n- 正常条目\n";
        fs::write(dir.join("CHANGELOG.md"), md).unwrap();

        // Exact match with full-width brackets + sanitization
        let exact = extract_changelog_entry_exact(&dir, "CHANGELOG.md", "1.2.3").unwrap();
        assert!(exact.contains("重要修复"));
        assert!(exact.contains("<user-dir>"), "personal user dir must be masked: {}", exact);
        assert!(!exact.contains("Alice"));
        assert!(!exact.contains("pass123"));
        assert!(exact.contains("https://***@example.com/api"));
        assert!(!exact.contains("ghp_1234567890"));

        // Heading-only empty stub (1.2.4) MUST return None so fallback cascade can run
        assert!(extract_changelog_entry_exact(&dir, "CHANGELOG.md", "1.2.4").is_none());

        // Unreleased section preserves legitimate system path C:\Windows\Temp
        let unrel = extract_unreleased_changelog_notes(&dir, "CHANGELOG.md").unwrap();
        assert!(unrel.contains("新增自动读取更新日志能力"));
        assert!(unrel.contains(r"C:\Windows\Temp"));

        // Bare fallback detection
        assert!(is_bare_default_release_note(""));
        assert!(is_bare_default_release_note("QDisk v1.46.16 正式发布。"));
        assert!(is_bare_default_release_note("- 常规优化与问题修复。"));
        assert!(!is_bare_default_release_note("## 1.46.16\n- 修复更新日志自动提取"));

        // README.md with marketing content ONLY must NOT pollute release notes
        fs::write(
            dir.join("README.md"),
            "# MyApp\n\n## Features\n- Fast and clean\n\n## Download\n- Link\n",
        )
        .unwrap();
        assert!(extract_notes_from_readme(&dir, "2.0.0").is_none());

        // README.md with `## 更新日志` + `### v2.0.0` MUST extract cleanly without marketing banner
        fs::write(
            dir.join("README.md"),
            "# MyApp\n\n## 简介\n营销口号不应进入更新日志\n\n## 更新日志\n\n### v2.0.0\n- 支持从 README.md 更新日志区自动提取\n- 支持一键清空自写\n\n### v1.9.0\n- 旧版本记录\n",
        )
        .unwrap();
        let (rm_notes, rm_src) = extract_notes_from_readme(&dir, "2.0.0").unwrap();
        assert!(rm_notes.contains("支持从 README.md 更新日志区自动提取"));
        assert!(!rm_notes.contains("营销口号"));
        assert!(!rm_notes.contains("旧版本记录"));
        assert_eq!(rm_src, "README.md");

        // Dedicated per-version release doc `docs/RELEASE_v2.1.0.md` MUST be discovered
        fs::create_dir_all(dir.join("docs")).unwrap();
        fs::write(
            dir.join("docs/RELEASE_v2.1.0.md"),
            "# MyApp v2.1.0\n- 独立版本发版说明文件\n",
        )
        .unwrap();
        let (rel_notes, rel_path) =
            extract_dedicated_release_doc_notes(&dir, "CHANGELOG.md", "2.1.0").unwrap();
        assert!(rel_notes.contains("独立版本发版说明文件"));
        assert_eq!(rel_path, "docs/RELEASE_v2.1.0.md");

        // upsert_changelog_entry: ascending changelog appends at bottom AND replaces in-place without duplicates
        fs::write(
            dir.join("asc.md"),
            "# 变更记录\n\n越往下越新。\n\n## 1.0.0\n- 初始版本\n\n## 1.1.0\n- 第二版\n",
        )
        .unwrap();
        upsert_changelog_entry(&dir, "asc.md", "1.2.0", "- 第三版新增").unwrap();
        let asc1 = fs::read_to_string(dir.join("asc.md")).unwrap();
        assert!(asc1.find("## 1.1.0").unwrap() < asc1.find("## 1.2.0").unwrap(), "ascending file must append at bottom");
        // Re-upsert 1.2.0 with existing heading -> must replace in-place with zero duplicate headers
        upsert_changelog_entry(&dir, "asc.md", "1.2.0", "## 1.2.0（更新）\n- 修订第三版").unwrap();
        let asc2 = fs::read_to_string(dir.join("asc.md")).unwrap();
        assert_eq!(asc2.matches("1.2.0").count(), 1, "must not duplicate version heading: {}", asc2);
        assert!(asc2.contains("修订第三版"));

        // Descending changelog prepends at top
        fs::write(dir.join("desc.md"), "# Changelog\n\n## 2.0.0\n- 新\n\n## 1.0.0\n- 旧\n").unwrap();
        upsert_changelog_entry(&dir, "desc.md", "2.1.0", "- 最新").unwrap();
        let desc1 = fs::read_to_string(dir.join("desc.md")).unwrap();
        assert!(desc1.find("## 2.1.0").unwrap() < desc1.find("## 2.0.0").unwrap(), "descending file must prepend at top");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_prepend_and_upsert_respect_ascending_prerelease_without_duplicates() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_cl_asc_pre_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(88)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("CHANGELOG.md"),
            "# 变更记录\n\n## 1.0.0-beta.1\n- beta 功能\n\n## 1.0.0\n- 首个正式版\n",
        )
        .unwrap();

        prepend_changelog_entry(&dir, "CHANGELOG.md", "1.0.1", "修复汇总", "- 修复问题 A").unwrap();
        let after1 = fs::read_to_string(dir.join("CHANGELOG.md")).unwrap();
        assert!(
            after1.find("## 1.0.0\n").unwrap() < after1.find("## 1.0.1").unwrap(),
            "ascending prerelease->stable changelog must append at bottom, got:\n{}",
            after1
        );

        // Updating same version must replace in place with ZERO duplicate headings
        prepend_changelog_entry(&dir, "CHANGELOG.md", "1.0.1", "修订版", "- 修复问题 A 与 B").unwrap();
        let after2 = fs::read_to_string(dir.join("CHANGELOG.md")).unwrap();
        assert_eq!(after2.matches("## 1.0.1").count(), 1, "must not duplicate heading:\n{}", after2);
        assert!(after2.contains("修复问题 A 与 B"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rejects_bare_placeholder_stubs_aggregates_since_and_preserves_crlf() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_cl_stub_range_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(99)
        ));
        fs::create_dir_all(&dir).unwrap();

        // 1. Bare placeholder detection must catch multi-line heading + bare sentence stubs,
        //    yet preserve headings that carry a substantive title!
        assert!(is_bare_default_release_note(
            "## 1.47.3 (2026-11-26)\n\nQDisk v1.47.3 正式发布。"
        ));
        assert!(is_bare_default_release_note("## 1.47.3 (2026-11-26)\n"));
        assert!(!is_bare_default_release_note(
            "## 1.47.1（patch）—— 修复托盘图标卡顿\n\n- 常规优化与问题修复。"
        ));

        // 2. Write an ascending changelog containing mixed CRLF lines (historical) + real 1.47.0/1.47.1 + bare 1.47.3 stub
        let mixed_cl = "# 变更记录\n\n越往下越新。\n\n## 1.42.12（patch）—— 历史记录\r\n- 保留原 CRLF 行尾\r\n\n## 1.46.16\n- 已发布旧版\n\n## 1.47.0（minor）—— 全站重构\n- 重塑 Apple 原生视觉体系\n\n## 1.47.1（patch）—— 视觉巡检精修\n- 移除输入框默认蓝框\n\n## 1.47.3 (2026-11-26)\n\nQDisk v1.47.3 正式发布。\n";
        fs::write(dir.join("变更记录.md"), mixed_cl).unwrap();

        // Exact lookup on bare 1.47.3 stub MUST return None (not treat bare placeholder as real changelog!)
        assert!(
            extract_changelog_entry_exact(&dir, "变更记录.md", "1.47.3").is_none(),
            "bare placeholder changelog stub must be rejected"
        );

        // Range aggregation after 1.46.16 up to 1.47.2 MUST pull 1.47.1 and 1.47.0 newest-first (skipping 1.46.16 & 1.47.3)
        let (combined, vers, fname) =
            extract_changelog_entries_since(&dir, "变更记录.md", Some("1.46.16"), "1.47.2")
                .expect("should aggregate 1.47.0 and 1.47.1");
        assert_eq!(fname, "变更记录.md");
        assert_eq!(vers, vec!["1.47.1".to_string(), "1.47.0".to_string()]);
        assert!(combined.contains("移除输入框默认蓝框"));
        assert!(combined.contains("重塑 Apple 原生视觉体系"));
        assert!(!combined.contains("已发布旧版"));
        assert!(!combined.contains("正式发布"));

        // Latest substantive entry MUST skip bare 1.47.3 stub and pick 1.47.1!
        let (latest_v, latest_body) =
            extract_latest_substantive_changelog_entry(&dir, "变更记录.md").unwrap();
        assert_eq!(latest_v, "1.47.1");
        assert!(latest_body.contains("视觉巡检精修"));

        // Upsert MUST refuse bare default placeholder when summary is empty
        assert!(
            upsert_changelog_entry(&dir, "变更记录.md", "1.47.3", "QDisk v1.47.3 正式发布。").is_err(),
            "must block polluting changelog with bare default placeholder"
        );

        // Upsert with real notes MUST replace 1.47.3 in-place AND preserve historical CRLF lines byte-for-byte!
        upsert_changelog_entry(&dir, "变更记录.md", "1.47.3", "- 修复自动扫描重复弹窗问题").unwrap();
        let updated_raw = fs::read_to_string(dir.join("变更记录.md")).unwrap();
        assert!(
            updated_raw.contains("## 1.42.12（patch）—— 历史记录\r\n- 保留原 CRLF 行尾\r\n"),
            "untouched historical CRLF lines must remain byte-for-byte intact"
        );
        assert!(updated_raw.contains("修复自动扫描重复弹窗问题"));
        assert_eq!(updated_raw.matches("1.47.3").count(), 1);

        let _ = fs::remove_dir_all(&dir);
    }
}

// ---------------------------------------------------------------- 发布通道裁决（唯一真相源）

/// 版本号是否为 SemVer 预发布（`1.2.3-beta.1`）；构建元数据 `+build-5` 里的连字符不算。
pub fn version_is_prerelease(ver: &str) -> bool {
    let v = ver.trim().trim_start_matches(['v', 'V']);
    let core = v.split('+').next().unwrap_or(v);
    core.contains('-')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelDecision {
    /// 只会是 `"stable"` 或 `"beta"`
    pub channel: &'static str,
    /// 是否作为 GitHub/Gitee 等平台的「预发布」推送
    pub prerelease: bool,
    /// 需要告知用户的修正/风险说明
    pub note: Option<String>,
}

/// 把用户请求的通道（任意大小写/别名/空值）与版本号一起裁决成唯一的发布通道。
///
/// - 带预发布后缀的版本（`1.2.0-beta.1`）一律是 beta，绝不会当作正式版推给所有人；
/// - 请求值做 trim + 大小写 + 常见别名归一，杜绝 `"Beta "`、`"测试版"` 这类写法在不同调用点判断不一致；
/// - 正式号 + 明确选了测试版 → 尊重（方向安全：不会泄漏给正式版用户），但返回风险说明。
pub fn resolve_release_channel(requested: &str, version: &str) -> ChannelDecision {
    let req = requested.trim().to_lowercase();
    let want_beta = matches!(
        req.as_str(),
        "beta" | "test" | "testing" | "preview" | "pre" | "prerelease" | "pre-release"
            | "rc" | "alpha" | "nightly" | "canary" | "dev" | "测试版" | "测试" | "预览版"
    );
    let known_stable = matches!(
        req.as_str(),
        "" | "stable" | "release" | "production" | "prod" | "latest" | "main" | "正式版" | "正式"
    );
    if version_is_prerelease(version) {
        let note = if want_beta {
            None
        } else {
            Some(format!(
                "版本 {} 带预发布后缀，已强制按测试版 (beta) 发布，避免推给正式版用户",
                version.trim()
            ))
        };
        return ChannelDecision { channel: "beta", prerelease: true, note };
    }
    if want_beta {
        return ChannelDecision {
            channel: "beta",
            prerelease: true,
            note: Some(format!(
                "版本 {} 是正式版本号，但通道选了测试版：将作为预发布推送，正式版用户不会收到",
                version.trim()
            )),
        };
    }
    let note = if known_stable {
        None
    } else {
        Some(format!("未识别的发布通道「{}」，已按正式版 (stable) 处理", requested.trim()))
    };
    ChannelDecision { channel: "stable", prerelease: false, note }
}

#[cfg(test)]
mod channel_tests {
    use super::*;

    #[test]
    fn prerelease_detection_ignores_build_metadata_hyphen() {
        assert!(version_is_prerelease("1.2.0-beta.1"));
        assert!(version_is_prerelease("v1.2.0-rc.2"));
        assert!(!version_is_prerelease("1.2.0"));
        assert!(!version_is_prerelease("1.2.0+build-5"));
    }

    #[test]
    fn prerelease_version_can_never_be_stable_even_if_requested() {
        for req in ["stable", "", "STABLE", "garbage"] {
            let d = resolve_release_channel(req, "1.5.0-beta.3");
            assert_eq!(d.channel, "beta", "req={req}");
            assert!(d.prerelease);
            assert!(d.note.is_some());
        }
        let ok = resolve_release_channel("beta", "1.5.0-beta.3");
        assert!(ok.prerelease && ok.note.is_none());
    }

    #[test]
    fn plain_version_normalizes_aliases_and_flags_explicit_beta() {
        for req in ["stable", "Stable ", "", "正式版", "latest"] {
            let d = resolve_release_channel(req, "1.5.0");
            assert_eq!((d.channel, d.prerelease), ("stable", false), "req={req}");
            assert!(d.note.is_none(), "req={req}");
        }
        for req in ["beta", " Beta ", "测试版", "RC", "nightly"] {
            let d = resolve_release_channel(req, "1.5.0");
            assert_eq!((d.channel, d.prerelease), ("beta", true), "req={req}");
            assert!(d.note.is_some(), "explicit beta on plain version must warn: {req}");
        }
        let unk = resolve_release_channel("weird", "1.5.0");
        assert_eq!(unk.channel, "stable");
        assert!(unk.note.is_some());
    }
}
