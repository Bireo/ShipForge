use crate::guardrails::detect_sensitive_reason;
use crate::models::{
    FileCategoryStat, LargeFileEntry, ProjectFileAnalysis, SensitiveWarning,
};
use crate::storage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use walkdir::WalkDir;

const SKIP_HEAVY_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".nuxt",
    ".cargo",
    ".gradle",
    "bin",
    "obj",
];

fn classify_file(rel: &str) -> (&'static str, &'static str) {
    let lower = rel.replace('\\', "/").to_ascii_lowercase();
    if detect_sensitive_reason(rel).is_some() {
        return ("sensitive", "敏感与凭据文件");
    }
    let file_name = lower.rsplit('/').next().unwrap_or(&lower);
    if matches!(file_name, "license" | "copying" | "readme" | "notice") {
        return ("doc", "文档与说明");
    }
    if matches!(file_name, "makefile" | "dockerfile" | "cmakelists.txt") {
        return ("source", "源码与配置");
    }
    let ext = Path::new(file_name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match ext {
        "exe" | "msi" | "appimage" | "dmg" | "deb" | "rpm" | "bat" | "cmd" | "ps1" | "sh" => {
            ("executable", "可执行与安装程序")
        }
        "dll" | "so" | "dylib" | "lib" | "a" | "node" | "pyd" | "wasm" => {
            ("library", "动态库与运行时")
        }
        "pdb" | "map" | "dsym" | "ilk" | "exp" | "log" | "tmp" => ("debug", "调试符号与日志"),
        "png" | "jpg" | "jpeg" | "webp" | "ico" | "svg" | "gif" | "mp4" | "mp3" | "wav"
        | "ttf" | "otf" | "woff" | "woff2" => ("asset", "图片字体与媒体资源"),
        "zip" | "7z" | "tar" | "gz" | "xz" | "rar" | "whl" | "jar" | "nupkg" => {
            ("archive", "归档与分发包")
        }
        "md" | "txt" | "pdf" | "html" | "htm" | "rst" => ("doc", "文档与说明"),
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py" | "go" | "cs" | "java"
        | "kt" | "c" | "cc" | "cpp" | "h" | "hpp" | "dart" | "css" | "scss" | "vue"
        | "svelte" | "json" | "toml" | "yaml" | "yml" | "xml" | "ini" => ("source", "源码与配置"),
        _ => ("other", "其他文件"),
    }
}

/// Fast, bounded-memory file analyzer for a project directory.
/// Avoids traversing giant build-cache / VCS trees and nested sibling projects while reporting their presence.
#[allow(dead_code)]
pub fn analyze_project_files(project_id: &str, root: &Path) -> ProjectFileAnalysis {
    analyze_project_files_isolated(project_id, root, &[])
}

pub fn analyze_project_files_isolated(
    project_id: &str,
    root: &Path,
    other_project_roots: &[PathBuf],
) -> ProjectFileAnalysis {
    let started = Instant::now();
    let mut skipped_dirs: Vec<String> = Vec::new();
    let mut cat_map: HashMap<&'static str, (&'static str, usize, u64)> = HashMap::new();
    let mut largest: Vec<LargeFileEntry> = Vec::new();
    let mut sensitive: Vec<SensitiveWarning> = Vec::new();
    let mut total_files = 0usize;
    let mut total_bytes = 0u64;
    let mut truncated = false;

    const MAX_FILES_BUDGET: usize = 80_000;

    let self_key = storage::normalize_root_key(root);
    let self_prefix = format!("{}\\", self_key);
    let nested_others: Vec<String> = other_project_roots
        .iter()
        .map(|p| storage::normalize_root_key(p))
        .filter(|k| !k.is_empty() && k != &self_key && k.starts_with(&self_prefix))
        .collect();

    let mut it = WalkDir::new(root).follow_links(false).into_iter();
    while let Some(entry_res) = it.next() {
        let Ok(entry) = entry_res else { continue };
        let path = entry.path();
        if path == root {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        if entry.file_type().is_dir() {
            if crate::guardrails::is_reparse_point(path) {
                it.skip_current_dir();
                continue;
            }
            if !nested_others.is_empty() {
                let ek = storage::normalize_root_key(path);
                if nested_others.iter().any(|ok| &ek == ok) {
                    if skipped_dirs.len() < 30 {
                        skipped_dirs.push(format!("{} (已隔离其他项目)", rel));
                    }
                    it.skip_current_dir();
                    continue;
                }
            }
            let name = entry.file_name().to_string_lossy();
            let lower = name.to_ascii_lowercase();
            if SKIP_HEAVY_DIRS.iter().any(|d| *d == lower) {
                if skipped_dirs.len() < 30 {
                    skipped_dirs.push(rel);
                }
                it.skip_current_dir();
            }
            continue;
        }

        if crate::guardrails::is_reparse_point(path) || !entry.file_type().is_file() {
            continue;
        }
        if total_files >= MAX_FILES_BUDGET {
            truncated = true;
            break;
        }
        total_files += 1;

        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        total_bytes = total_bytes.saturating_add(size);

        if let Some(reason) = detect_sensitive_reason(&rel) {
            if sensitive.len() < 40 {
                sensitive.push(SensitiveWarning {
                    rel_path: rel.clone(),
                    reason: reason.to_string(),
                    size_bytes: size,
                });
            }
        }

        let (cat_id, cat_label) = classify_file(&rel);
        let e = cat_map.entry(cat_id).or_insert((cat_label, 0, 0));
        e.1 += 1;
        e.2 = e.2.saturating_add(size);

        if largest.len() < 20 {
            largest.push(LargeFileEntry {
                rel_path: rel,
                size_bytes: size,
                category: cat_id.to_string(),
            });
            largest.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
        } else if let Some(last) = largest.last() {
            if size > last.size_bytes {
                largest.pop();
                largest.push(LargeFileEntry {
                    rel_path: rel,
                    size_bytes: size,
                    category: cat_id.to_string(),
                });
                largest.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
            }
        }
    }

    let mut categories: Vec<FileCategoryStat> = cat_map
        .into_iter()
        .map(|(cat, (label, count, bytes))| FileCategoryStat {
            category: cat.to_string(),
            label: label.to_string(),
            file_count: count,
            total_bytes: bytes,
        })
        .collect();
    categories.sort_by(|a, b| b.total_bytes.cmp(&a.total_bytes));

    ProjectFileAnalysis {
        project_id: project_id.to_string(),
        total_files,
        total_bytes,
        skipped_dirs,
        categories,
        largest_files: largest,
        sensitive_warnings: sensitive,
        truncated,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_analyzer_classifies_and_flags_sensitive_files() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_ana_test_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(31)
        ));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(dir.join(".env"), "API_KEY=secret").unwrap();
        fs::write(dir.join("app.exe"), vec![0u8; 4096]).unwrap();
        fs::write(dir.join("node_modules/pkg/index.js"), "ignored").unwrap();

        let rep = analyze_project_files("p1", &dir);
        assert_eq!(rep.total_files, 3);
        assert!(rep.skipped_dirs.iter().any(|d| d == "node_modules"));
        assert_eq!(rep.sensitive_warnings.len(), 1);
        assert_eq!(rep.sensitive_warnings[0].rel_path, ".env");
        assert_eq!(rep.largest_files[0].rel_path, "app.exe");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_analyzer_isolates_nested_projects_and_classifies_extensionless_files() {
        let dir = std::env::temp_dir().join(format!(
            "shipforge_ana_iso_{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(33)
        ));
        let nested = dir.join("sub_project");
        fs::create_dir_all(dir.join("docs")).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.join("exe"), b"plain text named exe").unwrap();
        fs::write(dir.join("docs/LICENSE"), b"MIT").unwrap();
        fs::write(dir.join("app.dSYM"), b"symbols").unwrap();
        fs::write(nested.join(".env"), b"NESTED_SECRET=1").unwrap();
        fs::write(nested.join("huge.bin"), vec![0u8; 8192]).unwrap();

        let rep = analyze_project_files_isolated("p1", &dir, &[nested]);
        assert_eq!(rep.total_files, 3);
        assert!(!rep.truncated);
        assert!(
            rep.sensitive_warnings.is_empty(),
            "Nested sibling project .env must NOT leak into parent analysis"
        );
        assert_eq!(classify_file("exe").0, "other", "Extensionless 'exe' must not match .exe");
        assert_eq!(classify_file("docs/LICENSE").0, "doc");
        assert_eq!(classify_file("app.dSYM").0, "debug");

        let _ = fs::remove_dir_all(&dir);
    }
}
