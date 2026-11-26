use crate::models::{
    GhPublishRequest, GhReleaseItem, GhStatus, GitCommit, GitFileChange, GitStatus, ProjectConfig,
};
use crate::packager::validate_manifest_for_publish;
use crate::runner::decode_win_bytes;
use anyhow::{anyhow, Result};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn hidden_cmd(prog: &str) -> Command {
    let mut cmd = Command::new(prog);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    if prog.eq_ignore_ascii_case("git") {
        // Prevent git from ever spawning/blocking on an interactive terminal prompt
        cmd.env("GIT_TERMINAL_PROMPT", "0");
    }
    cmd
}

pub fn validate_git_tag_name(tag: &str) -> Result<()> {
    let t = tag.trim();
    if t.is_empty() {
        return Err(anyhow!("标签名不能为空"));
    }
    if t.starts_with('-') || t.starts_with('.') || t.ends_with('.') || t.ends_with(".lock") {
        return Err(anyhow!("标签名不合法（不能以 `-` 或 `.` 开头）：{}", t));
    }
    let bad = [' ', '\t', '\r', '\n', '~', '^', ':', '?', '*', '[', '\\'];
    if t.contains("..") || t.contains("@{") || t.chars().any(|c| c.is_control() || bad.contains(&c)) {
        return Err(anyhow!("标签名包含非法字符：{}", t));
    }
    Ok(())
}

fn run_git(root: &Path, args: &[&str]) -> Result<String> {
    let out = hidden_cmd("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| anyhow!("无法调用 git：{}", e))?;
    if !out.status.success() {
        let err = decode_win_bytes(&out.stderr);
        let msg = if err.trim().is_empty() {
            decode_win_bytes(&out.stdout)
        } else {
            err
        };
        return Err(anyhow!("git {} 失败：{}", args.join(" "), msg.trim()));
    }
    Ok(decode_win_bytes(&out.stdout))
}

/// Runs a git command and returns combined stdout + stderr (essential for `git push`, which writes
/// progress and status messages like `Everything up-to-date` to stderr even when exiting 0).
fn run_git_combined(root: &Path, args: &[&str]) -> Result<String> {
    let out = hidden_cmd("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| anyhow!("无法调用 git：{}", e))?;
    let stdout = decode_win_bytes(&out.stdout);
    let stderr = decode_win_bytes(&out.stderr);
    let mut combined = String::new();
    if !stdout.trim().is_empty() {
        combined.push_str(stdout.trim());
    }
    if !stderr.trim().is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(stderr.trim());
    }
    if !out.status.success() {
        return Err(anyhow!(
            "git {} 失败：{}",
            args.join(" "),
            if combined.is_empty() { "未知错误" } else { combined.as_str() }
        ));
    }
    Ok(combined)
}

pub fn parse_remote_tag_names(ls_remote_out: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    for line in ls_remote_out.lines() {
        if let Some(ref_part) = line.split_whitespace().nth(1) {
            if let Some(rest) = ref_part.strip_prefix("refs/tags/") {
                let clean = rest.strip_suffix("^{}").unwrap_or(rest).trim();
                if !clean.is_empty() {
                    set.insert(clean.to_string());
                }
            }
        }
    }
    set
}

pub fn parse_ahead_behind(rev_list_out: &str) -> Option<(usize, usize)> {
    let mut parts = rev_list_out.split_whitespace();
    let behind: usize = parts.next()?.parse().ok()?;
    let ahead: usize = parts.next()?.parse().ok()?;
    Some((ahead, behind))
}

#[allow(dead_code)]
pub fn inspect_git(root: &Path) -> GitStatus {
    inspect_git_for_project(root, "")
}

pub fn inspect_git_for_project(root: &Path, release_dir_rel: &str) -> GitStatus {
    if !root.join(".git").exists() {
        return GitStatus {
            is_repo: false,
            branch: String::new(),
            head_sha: String::new(),
            remote_url: String::new(),
            dirty_count: 0,
            ahead_count: 0,
            behind_count: 0,
            has_upstream: false,
            changes: vec![],
            recent_commits: vec![],
            tags: vec![],
            last_tag: None,
        };
    }

    let mut rel_clean = release_dir_rel.trim().replace('\\', "/");
    while let Some(rest) = rel_clean.strip_prefix("./") {
        rel_clean = rest.to_string();
    }
    let rel_clean = rel_clean.trim_matches('/').to_string();
    let exclude_release_opt = if !rel_clean.is_empty()
        && rel_clean != "."
        && !rel_clean.contains(':')
        && !rel_clean.starts_with("..")
    {
        Some(rel_clean.as_str())
    } else {
        None
    };

    // Run independent read-only git probes concurrently (with --no-optional-locks on status so
    // background auto-watch never contends with `.git/index.lock`, and core.quotepath=false for readable CJK paths).
    let (branch, head_sha, remote_url, porcelain, log_out, tag_out, upstream_rev) =
        std::thread::scope(|s| {
            let h_meta = s.spawn(|| {
                let mut b = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                // Support freshly initialized repos (unborn HEAD with 0 commits)
                if b.is_empty() {
                    b = run_git(root, &["symbolic-ref", "--short", "HEAD"])
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                }
                let sha = run_git(root, &["rev-parse", "--short", "HEAD"])
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let rem = run_git(root, &["remote", "get-url", "origin"])
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                (b, sha, rem)
            });
            let h_status = s.spawn(|| {
                run_git(
                    root,
                    &[
                        "-c",
                        "core.quotepath=false",
                        "--no-optional-locks",
                        "status",
                        "--porcelain",
                    ],
                )
            });
            let h_log = s.spawn(|| {
                run_git(
                    root,
                    &[
                        "log",
                        "-n",
                        "20",
                        "--pretty=format:%h%x1f%ad%x1f%an%x1f%s%x1f%D",
                        "--date=short",
                    ],
                )
            });
            let h_tags = s.spawn(|| run_git(root, &["tag", "--sort=-creatordate"]));
            let h_up = s.spawn(|| run_git(root, &["rev-list", "--left-right", "--count", "@{u}...HEAD"]));

            let (b, sha, rem) = h_meta.join().unwrap_or_default();
            (
                b,
                sha,
                rem,
                h_status.join().unwrap_or_else(|_| Err(anyhow!("status thread"))),
                h_log.join().unwrap_or_else(|_| Err(anyhow!("log thread"))),
                h_tags.join().unwrap_or_else(|_| Err(anyhow!("tags thread"))),
                h_up.join().unwrap_or_else(|_| Err(anyhow!("upstream thread"))),
            )
        });

    let mut changes = Vec::new();
    if let Ok(porcelain) = porcelain {
        for line in porcelain.lines() {
            if line.len() < 4 {
                continue;
            }
            let st = line[..2].trim().to_string();
            let path = line[3..].trim().trim_matches('"').to_string();
            // Align with `git_commit_for_release`: untracked artifacts under release_dir are intentionally
            // excluded from source commits, so do not report them as permanent uncommitted source changes.
            if st == "??" {
                if let Some(rel) = exclude_release_opt {
                    if is_path_under_rel(&path, rel) {
                        continue;
                    }
                }
            }
            changes.push(GitFileChange { status: st, path });
        }
    }

    let mut recent_commits = Vec::new();
    if let Ok(log_out) = log_out {
        for line in log_out.lines() {
            let parts: Vec<&str> = line.split('\x1f').collect();
            if parts.len() >= 4 {
                recent_commits.push(GitCommit {
                    sha: parts[0].to_string(),
                    date: parts[1].to_string(),
                    author: parts[2].to_string(),
                    subject: parts[3].to_string(),
                    refs: parts.get(4).copied().unwrap_or("").to_string(),
                });
            }
        }
    }

    let tags: Vec<String> = tag_out
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .take(30)
        .collect();
    let last_tag = tags.first().cloned();

    // Inspect upstream sync state (`@{u}...HEAD` -> `<behind>\t<ahead>`), with fallback to `origin/<branch>`
    let (ahead_count, behind_count, has_upstream) = match upstream_rev {
        Ok(out) => match parse_ahead_behind(&out) {
            Some((ahead, behind)) => (ahead, behind, true),
            None => (0, 0, false),
        },
        Err(_) => {
            let fallback_ref = format!("origin/{}...HEAD", branch);
            if !branch.is_empty() && branch != "HEAD" {
                match run_git(root, &["rev-list", "--left-right", "--count", &fallback_ref]) {
                    Ok(out) => match parse_ahead_behind(&out) {
                        Some((ahead, behind)) => (ahead, behind, true),
                        None => (0, 0, false),
                    },
                    Err(_) => (0, 0, false),
                }
            } else {
                (0, 0, false)
            }
        }
    };

    GitStatus {
        is_repo: true,
        branch,
        head_sha,
        remote_url,
        dirty_count: changes.len(),
        ahead_count,
        behind_count,
        has_upstream,
        changes,
        recent_commits,
        tags,
        last_tag,
    }
}

#[derive(Debug, Clone)]
pub struct FileDiffSummary {
    pub path: String,
    pub category: &'static str,
    pub added: Option<usize>,
    pub deleted: Option<usize>,
    pub status_tag: String,
}

fn is_automated_release_commit_subject(subject: &str) -> bool {
    let low = subject.trim().to_ascii_lowercase();
    low.is_empty()
        || low.starts_with("pre-build ")
        || low.starts_with("chore(release):")
        || low.starts_with("chore: release")
        || low.starts_with("chore: update workspace")
        || low.starts_with("bump version")
        || low.starts_with("merge branch ")
        || low.starts_with("merge pull request ")
        || low.starts_with("merge remote-tracking ")
}

fn is_pure_version_meta_file(rel_path: &str, project: &ProjectConfig) -> bool {
    let norm = rel_path.trim().replace('\\', "/");
    let low = norm.trim_start_matches("./").to_ascii_lowercase();
    let fname = low.rsplit('/').next().unwrap_or(&low);
    if matches!(fname, "cargo.lock" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock") {
        return true;
    }
    for vf in &project.version_files {
        let vp = vf
            .path
            .trim()
            .replace('\\', "/")
            .trim_start_matches("./")
            .to_ascii_lowercase();
        if !vp.is_empty() && low == vp {
            return true;
        }
    }
    false
}

fn resolve_target_tag_ref(root: &Path, target_version: &str) -> Option<String> {
    let clean = target_version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    if clean.is_empty() {
        return None;
    }
    for candidate in [format!("v{}", clean), clean.to_string()] {
        let full_ref = format!("refs/tags/{}", candidate);
        if run_git(root, &["rev-parse", "--verify", "--quiet", &full_ref]).is_ok() {
            return Some(candidate);
        }
    }
    None
}

fn find_previous_release_tag(
    root: &Path,
    target_version: &str,
    end_ref: &str,
) -> Option<String> {
    let clean_target = target_version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    let end_sha = run_git(root, &["rev-parse", "--verify", "--quiet", &format!("{}^{{commit}}", end_ref)])
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let end_is_tag = end_ref != "HEAD";

    if let Ok(raw) = run_git(root, &["tag", "--sort=-v:refname"]) {
        for tag in raw.lines().map(str::trim).filter(|s| !s.is_empty()).take(50) {
            let low = tag.to_ascii_lowercase();
            if low.starts_with("build-") {
                continue;
            }
            let stripped = tag.trim_start_matches('v').trim_start_matches('V');
            if !clean_target.is_empty() && stripped.eq_ignore_ascii_case(clean_target) {
                continue;
            }
            if end_is_tag && tag.eq_ignore_ascii_case(end_ref) {
                continue;
            }
            // Only use tags that are ancestors of end_ref (prevents picking unrelated branch/future tags)
            if run_git(root, &["merge-base", "--is-ancestor", tag, end_ref]).is_ok() {
                if end_is_tag && !end_sha.is_empty() {
                    let tag_sha = run_git(
                        root,
                        &["rev-parse", "--verify", "--quiet", &format!("{}^{{commit}}", tag)],
                    )
                    .ok()
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                    if tag_sha == end_sha {
                        continue;
                    }
                }
                return Some(tag.to_string());
            }
        }
    }
    // Fallback to git describe excluding current tag
    let mut args: Vec<String> = vec![
        "describe".into(),
        "--tags".into(),
        "--abbrev=0".into(),
        end_ref.to_string(),
    ];
    if !clean_target.is_empty() {
        args.push(format!("--exclude=v{}", clean_target));
        args.push(format!("--exclude={}", clean_target));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_git(root, &refs)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Returns the clean SemVer (without leading `v`/`V`) of the previous formal release tag
/// (skipping `build-*` tags) strictly preceding `target_version`.
pub fn find_previous_release_tag_for_project(root: &Path, target_version: &str) -> Option<String> {
    if !root.join(".git").exists() {
        return None;
    }
    let existing_tag = resolve_target_tag_ref(root, target_version);
    let end_ref = existing_tag.as_deref().unwrap_or("HEAD");
    let tag = find_previous_release_tag(root, target_version, end_ref)?;
    let clean = tag.trim().trim_start_matches('v').trim_start_matches('V');
    if clean.is_empty() || crate::version::validate_semver(clean).is_err() {
        return None;
    }
    Some(clean.to_string())
}

fn is_changelog_rel_path(rel_path: &str, project: &ProjectConfig) -> bool {
    let norm = rel_path
        .trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase();
    let cfg_cl = project
        .changelog_path
        .trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase();
    if !cfg_cl.is_empty() && norm == cfg_cl {
        return true;
    }
    let fname = norm.rsplit('/').next().unwrap_or(&norm);
    matches!(
        fname,
        "changelog.md" | "变更记录.md" | "releases.md" | "history.md"
    )
}

/// Extracts newly added human-written lines from the project's changelog file inside Git diff
/// (`prev_tag..end_ref` and optional uncommitted tree), ignoring CRLF-only noise and bare stubs.
fn extract_changelog_additions_from_git_diff(
    root: &Path,
    project: &ProjectConfig,
    prev_tag: Option<&str>,
    end_ref: &str,
    include_dirty: bool,
) -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    let cfg_cl = project.changelog_path.trim().replace('\\', "/");
    if !cfg_cl.is_empty() {
        candidates.push(cfg_cl);
    }
    for d in ["CHANGELOG.md", "变更记录.md", "RELEASES.md", "HISTORY.md"] {
        if !candidates.iter().any(|c| c.eq_ignore_ascii_case(d)) {
            candidates.push(d.to_string());
        }
    }

    for cl_rel in candidates {
        let Ok(full) = crate::guardrails::resolve_confined_path(root, &cl_rel) else {
            continue;
        };
        if !full.is_file() {
            continue;
        }
        let mut raw_diffs: Vec<String> = Vec::new();
        if let Some(prev) = prev_tag {
            let range = format!("{}..{}", prev, end_ref);
            if let Ok(d) = run_git(
                root,
                &[
                    "-c",
                    "core.quotepath=false",
                    "diff",
                    "--ignore-cr-at-eol",
                    "-U0",
                    &range,
                    "--",
                    &cl_rel,
                ],
            ) {
                if !d.trim().is_empty() {
                    raw_diffs.push(d);
                }
            }
        }
        if include_dirty {
            for extra in [
                vec!["-c", "core.quotepath=false", "diff", "--ignore-cr-at-eol", "-U0", "--cached", "--", &cl_rel],
                vec!["-c", "core.quotepath=false", "diff", "--ignore-cr-at-eol", "-U0", "--", &cl_rel],
            ] {
                if let Ok(d) = run_git(root, &extra) {
                    if !d.trim().is_empty() {
                        raw_diffs.push(d);
                    }
                }
            }
        }
        if raw_diffs.is_empty() {
            continue;
        }

        // Collect truly added lines (`+...`), excluding lines that were identically removed (`-...`)
        // in the same diff (e.g. moved or line-ending-normalized blocks).
        let mut removed_set: HashSet<String> = HashSet::new();
        let mut added_lines: Vec<String> = Vec::new();
        for diff_text in &raw_diffs {
            for line in diff_text.lines() {
                if line.starts_with("---") || line.starts_with("+++") || line.starts_with("@@") {
                    continue;
                }
                if let Some(rest) = line.strip_prefix('-') {
                    let t = rest.trim();
                    if !t.is_empty() {
                        removed_set.insert(t.to_string());
                    }
                }
            }
        }
        for diff_text in &raw_diffs {
            for line in diff_text.lines() {
                if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
                    continue;
                }
                if let Some(rest) = line.strip_prefix('+') {
                    let t = rest.trim();
                    if t.is_empty() {
                        if !added_lines.is_empty() && !added_lines.last().map(|l| l.is_empty()).unwrap_or(true) {
                            added_lines.push(String::new());
                        }
                        continue;
                    }
                    if removed_set.contains(t) {
                        continue;
                    }
                    added_lines.push(rest.trim_end().to_string());
                }
            }
        }
        let body = crate::version::sanitize_public_release_notes(&added_lines.join("\n"));
        if !body.is_empty() && !crate::version::is_bare_default_release_note(&body) {
            return Some(body);
        }
    }
    None
}

pub fn normalize_git_rename_path(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('"');
    if let (Some(lbrace), Some(rbrace)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if lbrace < rbrace {
            let inside = &trimmed[lbrace + 1..rbrace];
            if let Some((_, new_part)) = inside.split_once(" => ") {
                let joined = format!(
                    "{}{}{}",
                    &trimmed[..lbrace],
                    new_part,
                    &trimmed[rbrace + 1..]
                );
                return joined.replace("//", "/").trim_start_matches('/').to_string();
            }
        }
    }
    if let Some((_, right)) = trimmed.rsplit_once(" => ") {
        return right.trim().trim_matches('"').to_string();
    }
    trimmed.to_string()
}

pub fn format_categorized_file_changes(
    commits: &[String],
    diffs: &[FileDiffSummary],
) -> Option<String> {
    if commits.is_empty() && diffs.is_empty() {
        return None;
    }
    let cat_order: [(&str, &str); 5] = [
        ("source", "核心功能与源码"),
        ("engine", "子引擎与核心服务"),
        ("config", "构建脚本与配置"),
        ("docs", "文档与说明"),
        ("other", "资源与界面素材"),
    ];
    let mut grouped: BTreeMap<&'static str, Vec<&FileDiffSummary>> = BTreeMap::new();
    for d in diffs {
        if d.category == "artifact" {
            continue;
        }
        grouped.entry(d.category).or_default().push(d);
    }
    if commits.is_empty() && grouped.is_empty() {
        return None;
    }

    let mut out = Vec::new();
    if !commits.is_empty() {
        out.push("### 更新内容".to_string());
        for c in commits.iter().take(12) {
            out.push(format!("- {}", c));
        }
        if commits.len() > 12 {
            out.push(format!("- …及其他 {} 项提交", commits.len() - 12));
        }
        if !grouped.is_empty() {
            out.push(String::new());
            out.push("#### 涉及改动模块".to_string());
            for (key, label) in cat_order {
                if let Some(items) = grouped.get(key) {
                    let top: Vec<String> = items
                        .iter()
                        .take(4)
                        .map(|d| {
                            let short = d.path.rsplit('/').next().unwrap_or(&d.path);
                            format!("`{}`", short)
                        })
                        .collect();
                    let suffix = if items.len() > 4 {
                        format!(" 等 {} 个文件", items.len())
                    } else {
                        String::new()
                    };
                    out.push(format!("- **{}**：{}{}", label, top.join("、"), suffix));
                }
            }
        }
    } else {
        out.push("### 本次变动摘要".to_string());
        for (key, label) in cat_order {
            if let Some(items) = grouped.get(key) {
                let mut total_add: usize = 0;
                let mut total_del: usize = 0;
                let mut has_stats = false;
                let mut names: Vec<String> = Vec::new();
                for d in items {
                    if let (Some(a), Some(del)) = (d.added, d.deleted) {
                        total_add = total_add.saturating_add(a);
                        total_del = total_del.saturating_add(del);
                        has_stats = true;
                    }
                    if names.len() < 4 {
                        let short = d.path.rsplit('/').next().unwrap_or(&d.path);
                        let token = format!("`{}`", short);
                        if !names.contains(&token) {
                            names.push(token);
                        }
                    }
                }
                let more = if items.len() > names.len() {
                    format!(" 等 {} 个文件", items.len())
                } else {
                    String::new()
                };
                let stat = if has_stats && (total_add > 0 || total_del > 0) {
                    format!("（+{} / -{} 行）", total_add, total_del)
                } else {
                    String::new()
                };
                out.push(format!("- **{}**：更新 {}{}{}", label, names.join("、"), more, stat));
            }
        }
    }

    let rendered = crate::version::sanitize_public_release_notes(&out.join("\n"));
    if rendered.is_empty() {
        None
    } else {
        Some(rendered)
    }
}

/// Automatically synthesize structured release notes from Git commits since the previous release tag
/// combined with real file diff statistics and uncommitted workspace changes (excluding release/ & secrets).
pub fn synthesize_release_notes_from_git(
    root: &Path,
    project: &ProjectConfig,
    target_version: &str,
) -> Option<String> {
    if !root.join(".git").exists() {
        return None;
    }

    let existing_tag = resolve_target_tag_ref(root, target_version);
    let end_ref = existing_tag.as_deref().unwrap_or("HEAD");
    let include_dirty = existing_tag.is_none();

    let prev_tag = find_previous_release_tag(root, target_version, end_ref);

    // 0. If the changelog file itself gained new human-written prose in this range/working tree,
    //    surface that real prose directly instead of summarizing file paths!
    if let Some(cl_prose) =
        extract_changelog_additions_from_git_diff(root, project, prev_tag.as_deref(), end_ref, include_dirty)
    {
        return Some(cl_prose);
    }

    let mut commits: Vec<String> = Vec::new();
    let mut seen_subjects: HashSet<String> = HashSet::new();

    let log_raw = match prev_tag {
        Some(ref t) => {
            let range = format!("{}..{}", t, end_ref);
            run_git(root, &["log", &range, "-n", "20", "--no-merges", "--pretty=format:%s%n%b%n---SF-COMMIT-END---"])
                .unwrap_or_default()
        }
        None => run_git(
            root,
            &["log", end_ref, "-n", "12", "--no-merges", "--pretty=format:%s%n%b%n---SF-COMMIT-END---"],
        )
        .unwrap_or_default(),
    };

    for block in log_raw.split("---SF-COMMIT-END---") {
        let mut lines = block.lines().map(str::trim).filter(|s| !s.is_empty());
        let Some(subj) = lines.next() else {
            continue;
        };
        if !is_automated_release_commit_subject(subj) {
            let key = subj.to_ascii_lowercase();
            if seen_subjects.insert(key) {
                commits.push(subj.to_string());
            }
        } else {
            // Even for automated build commits, inspect body bullet lines if human notes were attached
            for bl in lines {
                let cleaned = bl.trim_start_matches(['-', '*', '•', ' ']).trim();
                if cleaned.len() >= 4
                    && !cleaned.to_ascii_lowercase().starts_with("signed-off-by:")
                    && !cleaned.to_ascii_lowercase().starts_with("co-authored-by:")
                    && !crate::version::is_bare_default_release_note(cleaned)
                {
                    let key = cleaned.to_ascii_lowercase();
                    if seen_subjects.insert(key) {
                        commits.push(cleaned.to_string());
                    }
                }
            }
        }
    }

    // Collect file diffs from `<prev_tag>..<end_ref>` (and uncommitted workspace ONLY for upcoming untagged versions!)
    let rel_clean = project
        .release_dir
        .trim()
        .replace('\\', "/")
        .trim_matches('/')
        .to_string();
    let mut file_map: BTreeMap<String, FileDiffSummary> = BTreeMap::new();

    let diff_raw = match prev_tag {
        Some(ref t) => {
            let range = format!("{}..{}", t, end_ref);
            run_git(root, &["-c", "core.quotepath=false", "diff", "--numstat", &range])
                .unwrap_or_default()
        }
        None => run_git(
            root,
            &[
                "-c",
                "core.quotepath=false",
                "diff-tree",
                "--no-commit-id",
                "--numstat",
                "-r",
                end_ref,
            ],
        )
        .unwrap_or_default(),
    };

    let mut register_file = |raw_path: &str, added: Option<usize>, deleted: Option<usize>, status_tag: &str| {
        let norm = raw_path.trim().trim_matches('"').replace('\\', "/");
        if norm.is_empty() {
            return;
        }
        if !rel_clean.is_empty() && is_path_under_rel(&norm, &rel_clean) {
            return;
        }
        if crate::guardrails::detect_sensitive_reason(&norm).is_some() {
            return;
        }
        let cat = crate::storage::classify_change_path(&norm, project);
        if cat == "artifact" {
            return;
        }
        file_map
            .entry(norm.clone())
            .and_modify(|e| {
                if let Some(a) = added {
                    e.added = Some(e.added.unwrap_or(0) + a);
                }
                if let Some(d) = deleted {
                    e.deleted = Some(e.deleted.unwrap_or(0) + d);
                }
                if e.status_tag.is_empty() && !status_tag.is_empty() {
                    e.status_tag = status_tag.to_string();
                }
            })
            .or_insert(FileDiffSummary {
                path: norm,
                category: cat,
                added,
                deleted,
                status_tag: status_tag.to_string(),
            });
    };

    for line in diff_raw.lines() {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() != 3 {
            continue;
        }
        let a = parts[0].trim().parse::<usize>().ok();
        let d = parts[1].trim().parse::<usize>().ok();
        let clean_path = normalize_git_rename_path(parts[2]);
        register_file(&clean_path, a, d, "修改");
    }

    // Also inspect current uncommitted/staged workspace edits ONLY when resolving an untagged upcoming release
    if include_dirty {
        for numstat_args in [
            vec!["-c", "core.quotepath=false", "diff", "--cached", "--numstat"],
            vec!["-c", "core.quotepath=false", "diff", "--numstat"],
        ] {
            if let Ok(ns_raw) = run_git(root, &numstat_args) {
                for line in ns_raw.lines() {
                    let parts: Vec<&str> = line.splitn(3, '\t').collect();
                    if parts.len() != 3 {
                        continue;
                    }
                    let a = parts[0].trim().parse::<usize>().ok();
                    let d = parts[1].trim().parse::<usize>().ok();
                    let clean_path = normalize_git_rename_path(parts[2]);
                    register_file(&clean_path, a, d, "修改");
                }
            }
        }
        if let Ok(st_raw) = run_git(root, &["-c", "core.quotepath=false", "status", "--porcelain=v1", "-uall"]) {
            for line in st_raw.lines() {
                if line.len() < 4 {
                    continue;
                }
                let code = &line[0..2];
                let p_raw = line[3..].trim();
                let p_eff = if let Some((_, right)) = p_raw.rsplit_once(" -> ") {
                    right.trim()
                } else {
                    p_raw
                };
                let tag = if code.contains('?') || code.contains('A') {
                    "新增"
                } else if code.contains('D') {
                    "删除"
                } else {
                    "修改"
                };
                register_file(p_eff, None, None, tag);
            }
        }
    }

    // Prefer non-version-only files when functional/script/doc changes exist,
    // and never emit a raw changelog path alone when changelog diff had no substantive prose!
    let all_diffs: Vec<FileDiffSummary> = file_map.into_values().collect();
    let substantive: Vec<FileDiffSummary> = all_diffs
        .iter()
        .filter(|d| {
            if is_changelog_rel_path(&d.path, project) {
                return false;
            }
            if !is_pure_version_meta_file(&d.path, project) {
                return true;
            }
            // Keep Cargo.toml / package.json if significant edits (>4 lines) happened (e.g., profile/dependency changes)
            let total_lines = d.added.unwrap_or(0) + d.deleted.unwrap_or(0);
            total_lines > 4 && !d.path.to_ascii_lowercase().ends_with(".lock")
        })
        .cloned()
        .collect();

    // Sort substantive diffs by total diff size descending so most important changed files appear first
    let mut ordered = substantive;
    ordered.sort_by(|a, b| {
        let sa = a.added.unwrap_or(0) + a.deleted.unwrap_or(0);
        let sb = b.added.unwrap_or(0) + b.deleted.unwrap_or(0);
        sb.cmp(&sa).then_with(|| a.path.cmp(&b.path))
    });

    format_categorized_file_changes(&commits, &ordered)
}

pub fn draft_notes_from_git(root: &Path) -> Result<String> {
    if !root.join(".git").exists() {
        return Err(anyhow!("当前项目不是 Git 仓库"));
    }
    let mut dummy = ProjectConfig::default();
    dummy.root = root.to_string_lossy().to_string();
    dummy.release_dir = "release".into();
    if let Some(txt) = synthesize_release_notes_from_git(root, &dummy, "") {
        return Ok(txt);
    }
    Ok("- 常规优化与问题修复。".into())
}

pub fn git_has_remote(root: &Path) -> bool {
    run_git(root, &["remote"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

fn run_git_raw(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = hidden_cmd("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| anyhow!("无法调用 git：{}", e))?;
    if !out.status.success() {
        let err = decode_win_bytes(&out.stderr);
        let msg = if err.trim().is_empty() {
            decode_win_bytes(&out.stdout)
        } else {
            err
        };
        return Err(anyhow!("git {} 失败：{}", args.join(" "), msg.trim()));
    }
    Ok(out.stdout)
}

fn is_path_under_rel(path: &str, rel_clean: &str) -> bool {
    let p = path.replace('\\', "/").trim_matches('/').to_ascii_lowercase();
    let r = rel_clean.to_ascii_lowercase();
    p == r || p.starts_with(&format!("{}/", r))
}

#[derive(Debug, PartialEq, Eq)]
struct PorcelainPlan {
    conflicts: Vec<String>,
    to_stage: Vec<String>,
    to_unstage: Vec<String>,
}

/// Parses NUL-delimited `git -c core.quotepath=false status --porcelain=v1 -z -uall` output.
/// Immune to `.gitignore` pathspec errors (`:(exclude)` bug), Chinese filename quoting, spaces, and renames.
fn parse_porcelain_z_plan(raw: &[u8], exclude_rel: Option<&str>) -> PorcelainPlan {
    let mut plan = PorcelainPlan {
        conflicts: Vec::new(),
        to_stage: Vec::new(),
        to_unstage: Vec::new(),
    };
    let mut tokens = raw.split(|&b| b == 0).peekable();
    while let Some(rec) = tokens.next() {
        if rec.len() < 4 {
            continue;
        }
        let x = rec[0] as char;
        let y = rec[1] as char;
        let path = String::from_utf8_lossy(&rec[3..]).to_string();
        let _orig_path = if matches!(x, 'R' | 'C') {
            tokens
                .next()
                .map(|b| String::from_utf8_lossy(b).to_string())
        } else {
            None
        };

        // Never auto-stage unresolved merge conflicts (would silently commit <<<<<<< markers!)
        if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
            plan.conflicts.push(path);
            continue;
        }

        let excluded = exclude_rel
            .map(|rel| is_path_under_rel(&path, rel))
            .unwrap_or(false);
        if excluded {
            if x != ' ' && x != '?' {
                plan.to_unstage.push(path);
            }
            continue;
        }

        // Only stage `path` (for staged renames `RM`, `orig_path` is already removed from index and disk)
        if y != ' ' || x == '?' {
            plan.to_stage.push(path);
        }
    }
    plan
}

/// Stages & commits release changes (user dirty files + version bumps + lockfiles) while strictly
/// excluding `release_dir_rel` — works whether `release/` IS in `.gitignore` OR NOT!
pub fn git_commit_for_release(root: &Path, release_dir_rel: &str, message: &str) -> Result<String> {
    let msg = message.trim();
    if msg.is_empty() {
        return Err(anyhow!("提交说明不能为空"));
    }
    let mut rel_clean = release_dir_rel.trim().replace('\\', "/");
    while let Some(rest) = rel_clean.strip_prefix("./") {
        rel_clean = rest.to_string();
    }
    let rel_clean = rel_clean.trim_matches('/').to_string();
    let exclude_opt = if !rel_clean.is_empty()
        && rel_clean != "."
        && !rel_clean.contains(':')
        && !rel_clean.starts_with("..")
    {
        Some(rel_clean.as_str())
    } else {
        None
    };

    let raw_status = run_git_raw(
        root,
        &["-c", "core.quotepath=false", "status", "--porcelain=v1", "-z", "-uall"],
    )?;
    let plan = parse_porcelain_z_plan(&raw_status, exclude_opt);

    if !plan.conflicts.is_empty() {
        return Err(anyhow!(
            "检测到未解决的 Git 合并冲突（{}），请先手动解决后再提交",
            plan.conflicts.iter().take(4).cloned().collect::<Vec<_>>().join("、")
        ));
    }

    // If any binary inside release/ was previously staged manually, safely unstage it first
    if !plan.to_unstage.is_empty() {
        let has_head = run_git(root, &["rev-parse", "--verify", "HEAD"]).is_ok();
        for chunk in plan.to_unstage.chunks(60) {
            let mut args: Vec<&str> = if has_head {
                vec!["reset", "-q", "HEAD", "--"]
            } else {
                vec!["rm", "--cached", "-q", "--"]
            };
            args.extend(chunk.iter().map(|s| s.as_str()));
            let _ = run_git(root, &args);
        }
    }

    // Stage all non-release dirty/untracked/deleted paths in safe chunks with `--` flag guard
    for chunk in plan.to_stage.chunks(60) {
        let mut args: Vec<&str> = vec!["add", "-A", "--"];
        args.extend(chunk.iter().map(|s| s.as_str()));
        run_git(root, &args)?;
    }

    // Check specifically staged changes (`diff --cached --name-only`) so excluded untracked release binaries don't cause empty-commit failure!
    let staged = run_git(root, &["diff", "--cached", "--name-only"])?;
    if staged.trim().is_empty() {
        return Ok("工作区源码干净，无需提交".into());
    }
    run_git(root, &["commit", "-m", msg])?;
    let sha = run_git(root, &["rev-parse", "--short", "HEAD"])?;
    Ok(format!("已提交快照 {}", sha.trim()))
}

#[allow(dead_code)]
pub fn git_commit_all(root: &Path, message: &str) -> Result<String> {
    let msg = message.trim();
    if msg.is_empty() {
        return Err(anyhow!("提交说明不能为空"));
    }
    let raw_status = run_git_raw(
        root,
        &["-c", "core.quotepath=false", "status", "--porcelain=v1", "-z"],
    )?;
    let plan = parse_porcelain_z_plan(&raw_status, None);
    if !plan.conflicts.is_empty() {
        return Err(anyhow!(
            "检测到未解决的 Git 合并冲突（{}），请先解决冲突后再提交",
            plan.conflicts.iter().take(4).cloned().collect::<Vec<_>>().join("、")
        ));
    }
    run_git(root, &["add", "-A"])?;
    let staged = run_git(root, &["diff", "--cached", "--name-only"])?;
    if staged.trim().is_empty() {
        return Ok("工作区干净，无需提交".into());
    }
    run_git(root, &["commit", "-m", msg])?;
    let sha = run_git(root, &["rev-parse", "--short", "HEAD"])?;
    Ok(format!("已提交快照 {}", sha.trim()))
}

pub fn git_create_tag(root: &Path, tag: &str, message: &str) -> Result<String> {
    validate_git_tag_name(tag)?;
    let t = tag.trim();
    if message.trim().is_empty() {
        run_git(root, &["tag", "--", t])?;
    } else {
        run_git(root, &["tag", "-a", t, "-m", message.trim()])?;
    }
    Ok(format!("已创建标签 {}", t))
}

/// Safely soft-reset the latest unpushed local commit (`git reset --soft HEAD~1`), keeping all
/// edited files intact in the staging area / working tree. Refuses if already pushed to upstream.
pub fn git_undo_last_commit(root: &Path) -> Result<String> {
    if !root.join(".git").exists() {
        return Err(anyhow!("当前目录不是 Git 仓库"));
    }
    if run_git(root, &["rev-parse", "--verify", "HEAD"]).is_err() {
        return Err(anyhow!("当前仓库尚无任何提交记录，无法撤回"));
    }
    if run_git(root, &["rev-parse", "--verify", "HEAD~1"]).is_err() {
        return Err(anyhow!("当前仓库仅存在初始提交，无法撤回更早历史"));
    }
    let branch = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_string();
    let upstream_check = run_git(root, &["rev-list", "--left-right", "--count", "@{u}...HEAD"])
        .or_else(|_| {
            if !branch.is_empty() && branch != "HEAD" {
                let fallback = format!("origin/{}...HEAD", branch);
                run_git(root, &["rev-list", "--left-right", "--count", &fallback])
            } else {
                Err(anyhow!("no upstream"))
            }
        });
    if let Ok(out) = upstream_check {
        if let Some((ahead, _behind)) = parse_ahead_behind(&out) {
            if ahead == 0 {
                return Err(anyhow!(
                    "最新提交已推送至远端仓库；为保护协作历史，仅支持撤回尚未推送的本地提交"
                ));
            }
        }
    }
    // Secondary safety net (also protects detached HEAD / non-default remotes): block if HEAD exists on any remote-tracking branch
    if let Ok(remote_branches) = run_git(root, &["branch", "-r", "--contains", "HEAD"]) {
        if !remote_branches.trim().is_empty() {
            return Err(anyhow!(
                "最新提交已存在于远端分支（{}）；为保护协作历史，禁止撤回已推送提交",
                remote_branches.lines().next().unwrap_or("").trim()
            ));
        }
    }

    let sha = run_git(root, &["rev-parse", "--short", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_string();
    let subject = run_git(root, &["log", "-1", "--pretty=%s"])
        .unwrap_or_default()
        .trim()
        .to_string();
    run_git(root, &["reset", "--soft", "HEAD~1"])?;
    Ok(format!(
        "已撤回未推送提交 {}（{}），所有文件修改已安全保留在暂存区",
        sha, subject
    ))
}

/// Delete a Git tag ONLY on `origin` without touching the local tag.
pub fn git_delete_remote_tag_only(root: &Path, tag: &str) -> Result<String> {
    validate_git_tag_name(tag)?;
    let t = tag.trim();
    if !root.join(".git").exists() {
        return Err(anyhow!("当前目录不是 Git 仓库"));
    }
    if !git_has_remote(root) {
        return Err(anyhow!("未配置 origin 远程仓库，无法删除远端标签"));
    }
    let del_spec = format!(":refs/tags/{}", t);
    run_git(root, &["push", "origin", "--", &del_spec])?;
    Ok(format!("已删除远端标签 {}", t))
}

/// Delete a Git tag locally, and optionally delete the matching tag on `origin`.
pub fn git_delete_tag(root: &Path, tag: &str, delete_remote: bool) -> Result<String> {
    validate_git_tag_name(tag)?;
    let t = tag.trim();
    if !root.join(".git").exists() {
        return Err(anyhow!("当前目录不是 Git 仓库"));
    }
    let ref_name = format!("refs/tags/{}", t);
    let mut local_deleted = false;
    if run_git(root, &["rev-parse", "--verify", &ref_name]).is_ok() {
        run_git(root, &["tag", "-d", "--", t])?;
        local_deleted = true;
    }

    let mut remote_deleted = false;
    if delete_remote {
        if !git_has_remote(root) {
            return Err(anyhow!("未配置 origin 远程仓库，无法删除远端标签"));
        }
        let del_spec = format!(":refs/tags/{}", t);
        run_git(root, &["push", "origin", "--", &del_spec])?;
        remote_deleted = true;
    }

    if !local_deleted && !remote_deleted {
        return Err(anyhow!("本地不存在标签 {}", t));
    }
    match (local_deleted, remote_deleted) {
        (true, true) => Ok(format!("已删除本地及远端标签 {}", t)),
        (true, false) => Ok(format!("已删除本地标签 {}", t)),
        (false, true) => Ok(format!("已删除远端标签 {}（本地无此标签）", t)),
        (false, false) => unreachable!(),
    }
}

/// Idempotently creates annotated tag on HEAD: if tag already points to HEAD, succeeds without error;
/// if tag exists on a different commit, returns an error to protect history.
pub fn git_ensure_tag(root: &Path, tag: &str, message: &str) -> Result<String> {
    validate_git_tag_name(tag)?;
    let t = tag.trim();
    let head = run_git(root, &["rev-parse", "HEAD"])?.trim().to_string();
    if let Ok(existing) = run_git(root, &["rev-list", "-n", "1", t]) {
        if existing.trim() == head {
            return Ok(format!("标签 {} 已指向当前提交", t));
        }
        return Err(anyhow!(
            "标签 {} 已存在且指向历史提交，请先递增版本号以隔离发布",
            t
        ));
    }
    git_create_tag(root, t, message)
}

/// Opens a dedicated interactive console window for `gh auth login` or `gh auth switch` using hardcoded safe args.
pub fn launch_gh_auth_interactive(mode: &str) -> Result<String> {
    let gh_exe = crate::toolchain::find_binary_native(&["gh.exe", "gh"], &[])
        .ok_or_else(|| anyhow!("未检测到 GitHub CLI (gh)，请先在「工具链体检」中一键安装"))?;
    let sub_args: &[&str] = match mode {
        "switch" => &["auth", "switch"],
        _ => &["auth", "login", "--web", "-p", "https"],
    };
    #[cfg(windows)]
    {
        // Spawn a visible console window so the user can complete interactive device auth / account switching
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", "ShipForge GitHub Auth", "cmd", "/K"]);
        cmd.arg(&gh_exe).args(sub_args);
        cmd.spawn()
            .map_err(|e| anyhow!("无法启动 GitHub CLI 登录窗口：{}", e))?;
        return Ok("已打开 GitHub CLI 认证窗口，完成后点击「刷新状态」即可".into());
    }
    #[cfg(not(windows))]
    {
        let _ = (gh_exe, sub_args);
        Err(anyhow!("当前平台请直接在终端运行 gh auth login"))
    }
}

#[cfg(test)]
pub fn git_push(root: &Path, push_tags: bool) -> Result<String> {
    git_push_with_log(root, push_tags, |_, _, _| {})
}

/// Pushes current branch (and optionally only *unpushed* local tags) with structured progress + log callback.
/// Avoids false `! [rejected] <tag> (already exists)` failures when remote already holds existing release tags.
pub fn git_push_with_log<F>(root: &Path, push_tags: bool, mut on_event: F) -> Result<String>
where
    F: FnMut(u8, &str, &str),
{
    let remotes_raw = run_git(root, &["remote"]).unwrap_or_default();
    let target_remote = remotes_raw
        .lines()
        .map(|l| l.trim())
        .find(|r| *r == "origin")
        .or_else(|| remotes_raw.lines().map(|l| l.trim()).find(|r| !r.is_empty()))
        .ok_or_else(|| anyhow!("当前 Git 仓库未配置任何远端（origin），无法推送"))?
        .to_string();

    let branch = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "HEAD".into());

    on_event(
        15,
        "推送分支到远端",
        &format!("▶ 开始推送分支 `{}` 到远端 `{}`…", branch, target_remote),
    );

    let branch_out = match run_git_combined(root, &["push"]) {
        Ok(s) => s,
        Err(e) => {
            let msg = e.to_string().to_ascii_lowercase();
            if msg.contains("no upstream") || msg.contains("set-upstream") {
                on_event(
                    25,
                    "绑定上游并推送",
                    &format!("检测到分支 `{}` 尚未绑定上游，执行 git push -u {} HEAD…", branch, target_remote),
                );
                run_git_combined(root, &["push", "-u", &target_remote, "HEAD"])?
            } else {
                on_event(100, "推送失败", &format!("✗ {}", e));
                return Err(e);
            }
        }
    };

    for line in branch_out.lines().filter(|l| !l.trim().is_empty()) {
        on_event(45, "分支推送输出", line);
    }
    // Signal immediately once branch commits reached remote so callers mark transaction as remote-pushed
    // even if a subsequent tag push fails partway!
    on_event(48, "branch_pushed", &format!("分支 `{}` 已同步至远端 `{}`", branch, target_remote));

    let branch_summary = if branch_out.contains("Everything up-to-date") {
        format!("分支 {} 已是最新", branch)
    } else {
        format!("分支 {} 推送成功", branch)
    };

    if !push_tags {
        on_event(100, "推送完成", &format!("✓ {}", branch_summary));
        return Ok(branch_summary);
    }

    on_event(
        55,
        "核对远端标签",
        &format!("▶ 正在对比本地与远端 `{}` 标签清单，跳过已存在标签以避免冲突…", target_remote),
    );
    let ls_out = run_git(root, &["ls-remote", "--tags", &target_remote])?;
    let remote_tags = parse_remote_tag_names(&ls_out);
    let local_tags: Vec<String> = run_git(root, &["tag", "--list"])?
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let missing_tags: Vec<String> = local_tags
        .iter()
        .filter(|t| !remote_tags.contains(*t))
        .cloned()
        .collect();
    let skipped = local_tags.len().saturating_sub(missing_tags.len());

    if missing_tags.is_empty() {
        let note = format!(
            "{} · 远端已包含全部 {} 个本地标签（无需重复推送）",
            branch_summary, local_tags.len()
        );
        on_event(100, "推送完成", &format!("✓ {}", note));
        return Ok(note);
    }

    on_event(
        68,
        "推送新增标签",
        &format!(
            "发现 {} 个未推送标签（跳过 {} 个远端已存在标签）：{}",
            missing_tags.len(),
            skipped,
            missing_tags.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
        ),
    );

    let chunks: Vec<&[String]> = missing_tags.chunks(40).collect();
    let total_chunks = chunks.len().max(1);
    for (ci, chunk) in chunks.iter().enumerate() {
        let mut owned_args: Vec<String> = Vec::with_capacity(chunk.len() + 2);
        owned_args.push("push".into());
        owned_args.push(target_remote.clone());
        for t in *chunk {
            owned_args.push(format!("refs/tags/{}", t));
        }
        let arg_refs: Vec<&str> = owned_args.iter().map(|s| s.as_str()).collect();
        let out = match run_git_combined(root, &arg_refs) {
            Ok(s) => s,
            Err(e) => {
                on_event(100, "推送标签失败", &format!("✗ {}", e));
                return Err(e);
            }
        };
        let pct = (70 + ((ci + 1) * 25) / total_chunks).min(95) as u8;
        for line in out.lines().filter(|l| !l.trim().is_empty()) {
            on_event(pct, "标签推送输出", line);
        }
    }

    let summary = if skipped > 0 {
        format!(
            "{} · 新增推送 {} 个标签（跳过 {} 个已存在标签）",
            branch_summary,
            missing_tags.len(),
            skipped
        )
    } else {
        format!("{} · 新增推送 {} 个标签", branch_summary, missing_tags.len())
    };
    on_event(100, "推送完成", &format!("✓ {}", summary));
    Ok(summary)
}

pub fn check_gh_status() -> GhStatus {
    let out = hidden_cmd("gh").args(["auth", "status"]).output();
    let Ok(o) = out else {
        return GhStatus {
            installed: false,
            authenticated: false,
            account: String::new(),
            detail: "未检测到 GitHub CLI (gh)".into(),
        };
    };
    let text = format!(
        "{}\n{}",
        decode_win_bytes(&o.stdout),
        decode_win_bytes(&o.stderr)
    );
    let authenticated = o.status.success();
    let account = text
        .lines()
        .find(|l| l.contains("account "))
        .and_then(|l| l.split("account ").nth(1))
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("")
        .to_string();

    GhStatus {
        installed: true,
        authenticated,
        account,
        detail: text.trim().chars().take(400).collect(),
    }
}

pub fn list_gh_releases(repo: &str) -> Result<Vec<GhReleaseItem>> {
    let clean = repo.trim().trim_matches('/');
    if !clean.contains('/') {
        return Err(anyhow!("请先填写有效的 GitHub 仓库（owner/repo）"));
    }
    let out = hidden_cmd("gh")
        .args([
            "release",
            "list",
            "--repo",
            clean,
            "--limit",
            "15",
            "--json",
            "tagName,name,isPrerelease,isDraft,publishedAt",
        ])
        .output()
        .map_err(|e| anyhow!("无法调用 gh：{}", e))?;
    if !out.status.success() {
        let err = decode_win_bytes(&out.stderr);
        return Err(anyhow!("拉取 GitHub Releases 失败：{}", err.trim()));
    }
    let json_text = decode_win_bytes(&out.stdout);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&json_text).unwrap_or_default();
    let mut list = Vec::new();
    for item in arr {
        let tag = item
            .get("tagName")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        list.push(GhReleaseItem {
            id: String::new(),
            url: format!("https://github.com/{}/releases/tag/{}", clean, tag),
            tag_name: tag,
            name: item
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            body: String::new(),
            is_prerelease: item
                .get("isPrerelease")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            is_draft: item
                .get("isDraft")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            published_at: item
                .get("publishedAt")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            assets: vec![],
        });
    }
    Ok(list)
}

pub fn publish_gh_release(release_dir: &Path, req: &GhPublishRequest) -> Result<String> {
    let repo = req.repo.trim().trim_matches('/');
    if !repo.contains('/') {
        return Err(anyhow!("GitHub 仓库格式无效：{}", repo));
    }
    validate_git_tag_name(&req.tag)?;
    let tag = req.tag.trim();

    let mut file_pairs: Vec<(String, String)> = Vec::new();
    let mut clean_names = Vec::new();
    let mut seen_names = HashSet::new();
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
        clean_names.push(fname.clone());
        file_pairs.push((fname, p.to_string_lossy().to_string()));
    }
    if file_pairs.is_empty() {
        return Err(anyhow!("请至少选择一个要上传的产物文件"));
    }
    file_pairs.sort_by_key(|(fname, _)| crate::repo_client::upload_order_rank(fname));
    let file_paths: Vec<String> = file_pairs.into_iter().map(|(_, p)| p).collect();

    // Red-team pre-publish safety gate: block localhost URLs / mismatched manifest versions / stale SHA-256!
    validate_manifest_for_publish(release_dir, tag, &clean_names)?;

    let exists = hidden_cmd("gh")
        .args(["release", "view", tag, "--repo", repo])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    struct TempFileGuard(std::path::PathBuf);
    impl Drop for TempFileGuard {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    let tmp_notes = std::env::temp_dir().join(format!(
        "shipforge_notes_{}_{}.md",
        std::process::id(),
        chrono::Local::now().timestamp_millis()
    ));
    fs::write(&tmp_notes, format!("{}\n", req.notes.trim()).as_bytes())?;
    let _tmp_guard = TempFileGuard(tmp_notes.clone());

    let result = if exists {
        if !req.overwrite_assets {
            Err(anyhow!(
                "GitHub Release {} 已存在；如需覆盖资产请勾选「覆盖同名资产」",
                tag
            ))
        } else {
            let mut up_args = vec![
                "release".to_string(),
                "upload".to_string(),
                tag.to_string(),
            ];
            up_args.extend(file_paths.clone());
            up_args.extend([
                "--repo".to_string(),
                repo.to_string(),
                "--clobber".to_string(),
            ]);
            let out = hidden_cmd("gh").args(&up_args).output()?;
            if !out.status.success() {
                Err(anyhow!(
                    "上传资产失败：{}",
                    decode_win_bytes(&out.stderr).trim()
                ))
            } else {
                let _ = hidden_cmd("gh")
                    .args([
                        "release",
                        "edit",
                        tag,
                        "--repo",
                        repo,
                        "--title",
                        req.title.trim(),
                        "--notes-file",
                        &tmp_notes.to_string_lossy(),
                    ])
                    .output();
                Ok(format!("https://github.com/{}/releases/tag/{}", repo, tag))
            }
        }
    } else {
        let mut args = vec![
            "release".to_string(),
            "create".to_string(),
            tag.to_string(),
        ];
        args.extend(file_paths);
        args.extend([
            "--repo".to_string(),
            repo.to_string(),
            "--title".to_string(),
            req.title.trim().to_string(),
            "--notes-file".to_string(),
            tmp_notes.to_string_lossy().to_string(),
        ]);
        if req.prerelease {
            args.push("--prerelease".to_string());
        }
        if req.draft {
            args.push("--draft".to_string());
        }
        let out = hidden_cmd("gh").args(&args).output()?;
        if !out.status.success() {
            Err(anyhow!(
                "创建 Release 失败：{}",
                decode_win_bytes(&out.stderr).trim()
            ))
        } else {
            Ok(format!("https://github.com/{}/releases/tag/{}", repo, tag))
        }
    };

    let _ = fs::remove_file(&tmp_notes);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_git_tag_blocks_flag_injection() {
        assert!(validate_git_tag_name("v1.46.0").is_ok());
        assert!(validate_git_tag_name("v1.46.0-beta.1").is_ok());
        assert!(validate_git_tag_name("--delete").is_err());
        assert!(validate_git_tag_name("-f").is_err());
        assert!(validate_git_tag_name("v1.0..v2.0").is_err());
        assert!(validate_git_tag_name("bad tag").is_err());
    }

    #[test]
    fn test_git_commit_for_release_excludes_release_binaries_and_tags_idempotently() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_git_rel_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(7)
        ));
        fs::create_dir_all(base.join("release")).unwrap();
        run_git(&base, &["init"]).unwrap();
        run_git(&base, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(&base, &["config", "user.name", "ShipForge Test"]).unwrap();
        fs::write(base.join("README.md"), "hello\n").unwrap();
        run_git(&base, &["add", "README.md"]).unwrap();
        run_git(&base, &["commit", "-m", "init"]).unwrap();

        // Simulate dirty source + untracked binary installer inside release/ (NO .gitignore!)
        fs::write(base.join("README.md"), "hello v1.0.1\n").unwrap();
        fs::write(base.join("release").join("Demo_1.0.1_setup.exe"), b"MZ-huge-installer").unwrap();

        let msg = git_commit_for_release(&base, "release", "chore(release): v1.0.1").unwrap();
        assert!(msg.contains("已提交快照"));

        // Verify README.md was committed, while release/Demo_1.0.1_setup.exe was NOT committed!
        let committed_files = run_git(&base, &["show", "--name-only", "--pretty=format:", "HEAD"]).unwrap();
        assert!(committed_files.contains("README.md"));
        assert!(!committed_files.contains("Demo_1.0.1_setup.exe"));

        // Verify git_ensure_tag creates tag and is idempotent on same HEAD
        assert!(git_ensure_tag(&base, "v1.0.1", "Release v1.0.1").unwrap().contains("已创建标签"));
        assert!(git_ensure_tag(&base, "v1.0.1", "Release v1.0.1").unwrap().contains("已指向当前提交"));

        // Second commit on clean source with only release binary still untracked must succeed cleanly
        let second = git_commit_for_release(&base, "release", "chore(release): noop").unwrap();
        assert!(second.contains("无需提交"));

        // Case 2: Repo WITH `release` in `.gitignore` (exact bug scenario!) + Chinese filename + deleted file + staged rename + flag-like filename!
        fs::write(base.join(".gitignore"), "release/\n").unwrap();
        fs::create_dir_all(base.join("docs")).unwrap();
        fs::write(base.join("docs").join("变更记录.md"), "## 1.0.2\n").unwrap();
        fs::write(base.join("-dash-file.txt"), "safe\n").unwrap();
        fs::write(base.join("release").join("Demo_1.0.2_setup.exe"), b"MZ-ignored-installer").unwrap();

        // Staged rename (`RM`) followed by working-tree modification must NOT fail on orig_path!
        run_git(&base, &["mv", "README.md", "README_NEW.md"]).unwrap();
        fs::write(base.join("README_NEW.md"), "renamed and edited\n").unwrap();

        let third = git_commit_for_release(&base, "release", "chore(release): v1.0.2").unwrap();
        assert!(third.contains("已提交快照"));
        let committed3 = run_git(&base, &["-c", "core.quotepath=false", "show", "--name-only", "--pretty=format:", "HEAD"]).unwrap();
        assert!(committed3.contains("变更记录.md"));
        assert!(committed3.contains("-dash-file.txt"));
        assert!(committed3.contains("README_NEW.md"));
        assert!(!committed3.contains("Demo_1.0.2_setup.exe"));

        // Case 3: Pre-staged binary inside release/ (when not ignored) must be unstaged & excluded!
        let base2 = base.join("unborn-repo");
        fs::create_dir_all(base2.join("release")).unwrap();
        run_git(&base2, &["init"]).unwrap();
        run_git(&base2, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(&base2, &["config", "user.name", "ShipForge Test"]).unwrap();
        fs::write(base2.join("src.txt"), "initial\n").unwrap();
        fs::write(base2.join("release").join("leaked.zip"), b"PK-binary").unwrap();
        run_git(&base2, &["add", "release/leaked.zip"]).unwrap();
        let unborn_msg = git_commit_for_release(&base2, "release", "initial release").unwrap();
        assert!(unborn_msg.contains("已提交快照"));
        let unborn_files = run_git(&base2, &["show", "--name-only", "--pretty=format:", "HEAD"]).unwrap();
        assert!(unborn_files.contains("src.txt"));
        assert!(!unborn_files.contains("leaked.zip"));

        // Case 4: Unresolved merge conflict (`UU`) must be blocked cleanly!
        let conflict_plan = parse_porcelain_z_plan(b"UU src/main.rs\0 M Cargo.lock\0", Some("release"));
        assert_eq!(conflict_plan.conflicts, vec!["src/main.rs"]);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_parse_remote_tag_names_strips_peeled_suffix() {
        let sample = "570a71a8\trefs/tags/v1.46.2\n9bfcc02a\trefs/tags/v1.46.2^{}\nb7f453fb\trefs/tags/build-1.46.2\n";
        let set = parse_remote_tag_names(sample);
        assert_eq!(set.len(), 2);
        assert!(set.contains("v1.46.2"));
        assert!(set.contains("build-1.46.2"));
    }

    #[test]
    fn test_git_push_skips_already_existing_remote_tags() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_git_push_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(13)
        ));
        let remote_dir = base.join("remote.git");
        let local_dir = base.join("local");
        fs::create_dir_all(&remote_dir).unwrap();
        fs::create_dir_all(&local_dir).unwrap();

        run_git(&remote_dir, &["init", "--bare"]).unwrap();
        run_git(&local_dir, &["init"]).unwrap();
        run_git(&local_dir, &["config", "user.email", "t@example.com"]).unwrap();
        run_git(&local_dir, &["config", "user.name", "Tester"]).unwrap();
        fs::write(local_dir.join("a.txt"), "v1\n").unwrap();
        run_git(&local_dir, &["add", "a.txt"]).unwrap();
        run_git(&local_dir, &["commit", "-m", "c1"]).unwrap();
        run_git(&local_dir, &["remote", "add", "origin", &remote_dir.to_string_lossy()]).unwrap();

        // Push initial branch + v1.0.0 tag
        run_git(&local_dir, &["tag", "v1.0.0"]).unwrap();
        let r1 = git_push(&local_dir, true).unwrap();
        assert!(r1.contains("新增推送 1 个标签"));

        // Re-tag v1.0.0 on a new commit locally (exact conflict scenario!) AND add brand new v1.0.1
        fs::write(local_dir.join("a.txt"), "v2\n").unwrap();
        run_git(&local_dir, &["add", "a.txt"]).unwrap();
        run_git(&local_dir, &["commit", "-m", "c2"]).unwrap();
        run_git(&local_dir, &["tag", "-f", "v1.0.0"]).unwrap();
        run_git(&local_dir, &["tag", "v1.0.1"]).unwrap();

        let mut logs = Vec::new();
        let r2 = git_push_with_log(&local_dir, true, |_, _, line| logs.push(line.to_string())).unwrap();
        // Must succeed: pushes new v1.0.1 while skipping remote-existing v1.0.0 without throwing!
        assert!(r2.contains("新增推送 1 个标签"));
        assert!(r2.contains("跳过 1 个已存在标签"));
        assert!(!logs.is_empty());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_inspect_git_tracks_ahead_behind_and_upstream() {
        assert_eq!(parse_ahead_behind("0\t3\n"), Some((3, 0)));
        assert_eq!(parse_ahead_behind("2\t1\n"), Some((1, 2)));
        assert_eq!(parse_ahead_behind("bad"), None);

        let base = std::env::temp_dir().join(format!(
            "shipforge_git_ahead_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(31)
        ));
        let remote_dir = base.join("remote.git");
        let local_dir = base.join("local");
        fs::create_dir_all(&remote_dir).unwrap();
        fs::create_dir_all(&local_dir).unwrap();

        run_git(&remote_dir, &["init", "--bare"]).unwrap();
        run_git(&local_dir, &["init"]).unwrap();
        run_git(&local_dir, &["config", "user.email", "t@example.com"]).unwrap();
        run_git(&local_dir, &["config", "user.name", "Tester"]).unwrap();
        fs::write(local_dir.join("a.txt"), "1\n").unwrap();
        run_git(&local_dir, &["add", "a.txt"]).unwrap();
        run_git(&local_dir, &["commit", "-m", "c1"]).unwrap();
        run_git(&local_dir, &["remote", "add", "origin", &remote_dir.to_string_lossy()]).unwrap();

        // Before first push: no upstream yet
        let st0 = inspect_git(&local_dir);
        assert!(!st0.has_upstream);
        assert_eq!(st0.ahead_count, 0);

        // Push initial commit with upstream, then create 2 local pre-build commits WITHOUT pushing
        git_push(&local_dir, false).unwrap();
        fs::write(local_dir.join("a.txt"), "2\n").unwrap();
        run_git(&local_dir, &["add", "a.txt"]).unwrap();
        run_git(&local_dir, &["commit", "-m", "pre-build 1.0.1"]).unwrap();
        fs::write(local_dir.join("a.txt"), "3\n").unwrap();
        run_git(&local_dir, &["add", "a.txt"]).unwrap();
        run_git(&local_dir, &["commit", "-m", "pre-build 1.0.2"]).unwrap();

        let st1 = inspect_git(&local_dir);
        assert!(st1.has_upstream);
        assert_eq!(st1.dirty_count, 0);
        assert_eq!(st1.ahead_count, 2);
        assert_eq!(st1.behind_count, 0);

        // After pushing, ahead_count drops back to 0
        git_push(&local_dir, false).unwrap();
        let st2 = inspect_git(&local_dir);
        assert!(st2.has_upstream);
        assert_eq!(st2.ahead_count, 0);
        assert_eq!(st2.behind_count, 0);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_inspect_git_excludes_untracked_release_dir_and_decodes_chinese_paths() {
        let base = std::env::temp_dir().join(format!(
            "shipforge_inspect_rel_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(11)
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("release")).unwrap();
        fs::create_dir_all(base.join("docs")).unwrap();

        run_git(&base, &["init", "-b", "main"]).unwrap();
        run_git(&base, &["config", "user.email", "test@shipforge.local"]).unwrap();
        run_git(&base, &["config", "user.name", "ShipForge Test"]).unwrap();
        fs::write(base.join("README.md"), "init\n").unwrap();
        run_git(&base, &["add", "-A"]).unwrap();
        run_git(&base, &["commit", "-m", "init"]).unwrap();

        // Untracked binary inside release/ (NO .gitignore!) + dirty Chinese doc
        fs::write(base.join("release").join("App_1.0.0_setup.exe"), b"MZ-bin").unwrap();
        fs::write(base.join("docs").join("变更记录.md"), "## 1.0.0\n").unwrap();

        let st_before = inspect_git_for_project(&base, "release");
        assert_eq!(st_before.dirty_count, 1, "Untracked release/ must not count as dirty source");
        assert!(
            st_before.changes[0].path.contains("变更记录.md") || st_before.changes[0].path == "docs/",
            "Chinese path should not be octal-escaped: {:?}",
            st_before.changes
        );

        // After release commit, working tree should report 0 uncommitted source files even without .gitignore
        git_commit_for_release(&base, "release", "chore(release): v1.0.0").unwrap();
        let st_after = inspect_git_for_project(&base, "release");
        assert_eq!(st_after.dirty_count, 0, "Post-release inspect_git_for_project must be clean");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_git_undo_last_commit_soft_resets_unpushed_and_blocks_pushed() {
        use std::fs;
        let base = std::env::temp_dir().join(format!(
            "shipforge_git_undo_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(21)
        ));
        let bare = base.join("remote.git");
        let work = base.join("work");
        fs::create_dir_all(&bare).unwrap();
        fs::create_dir_all(&work).unwrap();

        run_git(&bare, &["init", "--bare"]).unwrap();
        run_git(&work, &["init", "-b", "main"]).unwrap();
        run_git(&work, &["config", "user.email", "test@shipforge.local"]).unwrap();
        run_git(&work, &["config", "user.name", "ShipForge Test"]).unwrap();
        run_git(&work, &["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();

        fs::write(work.join("app.txt"), "v1\n").unwrap();
        run_git(&work, &["add", "-A"]).unwrap();
        run_git(&work, &["commit", "-m", "initial"]).unwrap();
        run_git(&work, &["push", "-u", "origin", "main"]).unwrap();

        // Pushing makes ahead == 0 -> undo MUST be blocked
        fs::write(work.join("app.txt"), "v2\n").unwrap();
        run_git(&work, &["add", "-A"]).unwrap();
        run_git(&work, &["commit", "-m", "second"]).unwrap();
        run_git(&work, &["push", "origin", "main"]).unwrap();
        let blocked = git_undo_last_commit(&work);
        assert!(blocked.is_err(), "Already-pushed commit must not be undone");

        // Create an unpushed commit -> undo MUST succeed and preserve file contents!
        fs::write(work.join("app.txt"), "v3-wip\n").unwrap();
        run_git(&work, &["add", "-A"]).unwrap();
        run_git(&work, &["commit", "-m", "wip v3"]).unwrap();
        let msg = git_undo_last_commit(&work).unwrap();
        assert!(msg.contains("已撤回未推送提交"), "unexpected msg: {}", msg);
        assert_eq!(fs::read_to_string(work.join("app.txt")).unwrap(), "v3-wip\n");
        let st = inspect_git_for_project(&work, "release");
        assert_eq!(st.dirty_count, 1, "Undone commit changes must remain staged/dirty");
        assert_eq!(st.ahead_count, 0);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_git_delete_tag_local_and_remote() {
        use std::fs;
        let base = std::env::temp_dir().join(format!(
            "shipforge_git_deltag_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(23)
        ));
        let bare = base.join("remote.git");
        let work = base.join("work");
        fs::create_dir_all(&bare).unwrap();
        fs::create_dir_all(&work).unwrap();

        run_git(&bare, &["init", "--bare"]).unwrap();
        run_git(&work, &["init", "-b", "main"]).unwrap();
        run_git(&work, &["config", "user.email", "test@shipforge.local"]).unwrap();
        run_git(&work, &["config", "user.name", "ShipForge Test"]).unwrap();
        run_git(&work, &["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();

        fs::write(work.join("README.md"), "ok\n").unwrap();
        run_git(&work, &["add", "-A"]).unwrap();
        run_git(&work, &["commit", "-m", "init"]).unwrap();
        run_git(&work, &["push", "-u", "origin", "main"]).unwrap();

        git_create_tag(&work, "v1.0.0", "release 1.0.0").unwrap();
        git_push(&work, true).unwrap();

        let msg = git_delete_tag(&work, "v1.0.0", true).unwrap();
        assert!(msg.contains("本地及远端标签"), "unexpected msg: {}", msg);
        assert!(run_git(&work, &["rev-parse", "--verify", "refs/tags/v1.0.0"]).is_err());
        let remote_tags = run_git(&work, &["ls-remote", "--tags", "origin"]).unwrap();
        assert!(!remote_tags.contains("v1.0.0"), "Remote tag should be deleted");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_inspect_git_and_undo_on_unborn_head_repo() {
        use std::fs;
        let base = std::env::temp_dir().join(format!(
            "shipforge_git_unborn_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(37)
        ));
        fs::create_dir_all(&base).unwrap();
        run_git(&base, &["init", "-b", "main"]).unwrap();
        fs::write(base.join("README.md"), "hello\n").unwrap();

        let st = inspect_git_for_project(&base, "release");
        assert!(st.is_repo);
        assert_eq!(st.branch, "main", "unborn HEAD should still report branch name via symbolic-ref");
        assert!(st.head_sha.is_empty());
        assert_eq!(st.dirty_count, 1);

        let err = git_undo_last_commit(&base).unwrap_err().to_string();
        assert!(err.contains("尚无任何提交"), "unexpected err: {}", err);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_round2_git_delete_remote_tag_only_preserves_local_tag() {
        use std::fs;
        let base = std::env::temp_dir().join(format!(
            "shipforge_r2_remtag_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(47)
        ));
        let bare = base.join("remote.git");
        let work = base.join("work");
        fs::create_dir_all(&bare).unwrap();
        fs::create_dir_all(&work).unwrap();

        run_git(&bare, &["init", "--bare"]).unwrap();
        run_git(&work, &["init", "-b", "main"]).unwrap();
        run_git(&work, &["config", "user.email", "test@shipforge.local"]).unwrap();
        run_git(&work, &["config", "user.name", "ShipForge Test"]).unwrap();
        run_git(&work, &["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();

        fs::write(work.join("README.md"), "ok\n").unwrap();
        run_git(&work, &["add", "-A"]).unwrap();
        run_git(&work, &["commit", "-m", "init"]).unwrap();
        run_git(&work, &["push", "-u", "origin", "main"]).unwrap();

        git_create_tag(&work, "v2.0.0", "release 2.0.0").unwrap();
        git_push(&work, true).unwrap();

        git_delete_remote_tag_only(&work, "v2.0.0").unwrap();
        assert!(
            run_git(&work, &["rev-parse", "--verify", "refs/tags/v2.0.0"]).is_ok(),
            "local tag MUST remain intact when deleting remote tag only"
        );
        let remote_tags = run_git(&work, &["ls-remote", "--tags", "origin"]).unwrap();
        assert!(!remote_tags.contains("v2.0.0"), "remote tag must be deleted");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_synthesize_release_notes_handles_historical_tags_renames_prebuild_and_secrets() {
        assert_eq!(
            normalize_git_rename_path("src/pages/{old.js => new.js}"),
            "src/pages/new.js"
        );
        assert_eq!(
            normalize_git_rename_path("src/{a/b => c/d}/mod.rs"),
            "src/c/d/mod.rs"
        );
        assert_eq!(
            normalize_git_rename_path("src/{sub => }/a.rs"),
            "src/a.rs"
        );
        assert_eq!(normalize_git_rename_path("old.rs => new.rs"), "new.rs");

        let base = std::env::temp_dir().join(format!(
            "shipforge_git_notes_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(89)
        ));
        let repo = base.join("repo");
        fs::create_dir_all(repo.join("src")).unwrap();
        assert!(run_git(&repo, &["init"]).is_ok());
        assert!(run_git(&repo, &["config", "user.email", "t@t.com"]).is_ok());
        assert!(run_git(&repo, &["config", "user.name", "Tester"]).is_ok());

        // v1.0.0 baseline
        fs::write(repo.join("src/app.rs"), "fn main() {}\n").unwrap();
        assert!(run_git(&repo, &["add", "."]).is_ok());
        assert!(run_git(&repo, &["commit", "-m", "init"]).is_ok());
        assert!(run_git(&repo, &["tag", "v1.0.0"]).is_ok());

        // v1.0.1: automated pre-build commit with real code diff + secret file that MUST be excluded
        fs::write(
            repo.join("src/updater.rs"),
            "pub fn check() {}\npub fn download() {}\n",
        )
        .unwrap();
        fs::write(repo.join(".env"), "API_KEY=supersecret\n").unwrap();
        assert!(run_git(&repo, &["add", "."]).is_ok());
        assert!(run_git(&repo, &["commit", "-m", "pre-build 1.0.1 (full)"]).is_ok());
        assert!(run_git(&repo, &["tag", "v1.0.1"]).is_ok());

        // v1.0.2 (untagged ahead of v1.0.1): real commit + uncommitted edit
        fs::write(repo.join("src/settings.js"), "export const a = 1;\n").unwrap();
        assert!(run_git(&repo, &["add", "src/settings.js"]).is_ok());
        assert!(run_git(&repo, &["commit", "-m", "feat(settings): 新增更新日志自动提取"]).is_ok());
        fs::write(repo.join("src/uncommitted.rs"), "pub const X: i32 = 1;\n").unwrap();

        let mut proj = ProjectConfig::default();
        proj.root = repo.to_string_lossy().to_string();
        proj.release_dir = "release".into();

        // 1. Historical tag v1.0.1 MUST ONLY show v1.0.0..v1.0.1 (updater.rs), NOT v1.0.2's commits or dirty uncommitted.rs or .env!
        let notes_101 = synthesize_release_notes_from_git(&repo, &proj, "1.0.1")
            .expect("v1.0.1 should synthesize from pre-build diff");
        assert!(notes_101.contains("updater.rs"), "notes_101={}", notes_101);
        assert!(!notes_101.contains(".env"), "must never leak sensitive file: {}", notes_101);
        assert!(!notes_101.contains("settings.js"), "historical tag must not leak future v1.0.2: {}", notes_101);
        assert!(!notes_101.contains("uncommitted.rs"), "historical tag must not include current dirty files: {}", notes_101);

        // 2. Upcoming untagged v1.0.2 MUST include real commit subject AND uncommitted workspace edits!
        let notes_102 = synthesize_release_notes_from_git(&repo, &proj, "1.0.2")
            .expect("v1.0.2 should synthesize from commits + uncommitted");
        assert!(notes_102.contains("feat(settings): 新增更新日志自动提取"), "notes_102={}", notes_102);
        assert!(notes_102.contains("uncommitted.rs"), "upcoming version should capture uncommitted edit: {}", notes_102);

        let _ = fs::remove_dir_all(&base);
    }
}
