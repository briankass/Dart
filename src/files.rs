//! File search: a compact in-memory index of file and folder names under the
//! user's folders, plus fuzzy matching over it.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_FILES: usize = 100_000;
const MAX_DEPTH: usize = 12;

/// Directory names that are never worth descending into.
const SKIP_DIRS: &[&str] = &[
    "node_modules", "__pycache__", "appdata", "$recycle.bin", "system volume information",
    "my music", "my pictures", "my videos", // legacy junctions inside Documents
];

pub struct FileEntry {
    /// Full path.
    pub path: Box<str>,
    /// Byte offset where the file name starts within `path`.
    name_at: u32,
    /// Last modified, in days since the Unix epoch (0 = unknown).
    days: u32,
    #[cfg_attr(not(test), allow(dead_code))]
    pub is_dir: bool,
}

impl FileEntry {
    pub fn name(&self) -> &str {
        &self.path[self.name_at as usize..]
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn parent(&self) -> &str {
        self.path[..self.name_at as usize].trim_end_matches(['\\', '/'])
    }
}

/// Default folders to index: the usual user folders plus any OneDrive roots.
pub fn default_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) else { return Vec::new() };
    let mut roots: Vec<PathBuf> = ["Desktop", "Documents", "Downloads", "Pictures", "Music", "Videos"]
        .iter()
        .map(|d| home.join(d))
        .collect();
    if let Ok(rd) = std::fs::read_dir(&home) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with("onedrive") && e.file_type().is_ok_and(|t| t.is_dir()) {
                roots.push(e.path());
            }
        }
    }
    roots
}

pub fn scan(roots: &[PathBuf]) -> Vec<FileEntry> {
    let mut out = Vec::new();
    let mut seen_roots = HashSet::new();
    let today = days_since_epoch(SystemTime::now());
    for root in roots {
        let key = root.to_string_lossy().to_lowercase();
        if !seen_roots.insert(key) || !root.is_dir() {
            continue;
        }
        walk(root, 0, &mut out, today);
        if out.len() >= MAX_FILES {
            break;
        }
    }
    out
}

fn days_since_epoch(t: SystemTime) -> u32 {
    t.duration_since(UNIX_EPOCH).map(|d| (d.as_secs() / 86_400) as u32).unwrap_or(0)
}

#[cfg(windows)]
fn hidden_or_system(md: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    md.file_attributes() & (HIDDEN | SYSTEM) != 0
}

#[cfg(not(windows))]
fn hidden_or_system(_md: &std::fs::Metadata) -> bool {
    false
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<FileEntry>, today: u32) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut subdirs = Vec::new();
    for item in rd.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let Ok(ft) = item.file_type() else { continue };
        if ft.is_symlink() {
            continue; // avoids junction loops
        }
        let name = item.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') || name.starts_with('~') || name.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let is_dir = ft.is_dir();
        // Shortcuts are already in the app index; don't list them twice.
        let lower = name.to_lowercase();
        if !is_dir && (lower.ends_with(".lnk") || lower.ends_with(".url")) {
            continue;
        }
        let Ok(md) = item.metadata() else { continue };
        if hidden_or_system(&md) {
            continue;
        }
        if is_dir && SKIP_DIRS.contains(&lower.as_str()) {
            continue;
        }
        let path = item.path();
        let Some(path_str) = path.to_str() else { continue };
        let days = md.modified().map(days_since_epoch).unwrap_or(0).min(today);
        out.push(FileEntry {
            name_at: (path_str.len() - name.len()) as u32,
            path: path_str.into(),
            days,
            is_dir,
        });
        if is_dir {
            subdirs.push(path);
        }
    }
    for d in subdirs {
        walk(&d, depth + 1, out, today);
    }
}

pub struct FileSearcher {
    matcher: Matcher,
    buf: Vec<char>,
}

impl FileSearcher {
    pub fn new() -> Self {
        Self { matcher: Matcher::new(Config::DEFAULT.match_paths()), buf: Vec::new() }
    }

    /// Indices into `files`, best first. Matches on the file name only.
    pub fn search(
        &mut self,
        files: &[FileEntry],
        usage: &HashMap<String, u32>,
        query: &str,
        limit: usize,
    ) -> Vec<usize> {
        let query = query.trim();
        if query.chars().count() < 2 || limit == 0 {
            return Vec::new();
        }
        let today = days_since_epoch(SystemTime::now());
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let mut scored: Vec<(u32, usize)> = Vec::new();
        for (i, f) in files.iter().enumerate() {
            let hay = Utf32Str::new(f.name(), &mut self.buf);
            let Some(score) = pattern.score(hay, &mut self.matcher) else { continue };
            let age = today.saturating_sub(f.days);
            let recency = if f.days == 0 { 0 } else if age <= 7 { 12 } else if age <= 30 { 6 } else { 0 };
            let used = usage.get(&*f.path).copied().unwrap_or(0).min(50) * 4;
            scored.push((score + recency + used, i));
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| files[a.1].name().len().cmp(&files[b.1].name().len())));
        scored.truncate(limit);
        scored.into_iter().map(|(_, i)| i).collect()
    }
}

/// "f query" searches files only.
pub fn explicit(query: &str) -> Option<&str> {
    let rest = query.trim_start().strip_prefix("f ")?.trim();
    (!rest.is_empty()).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tree(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("dart-files-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["Docs/Taxes", "Docs/.git", "Docs/node_modules/pkg", "Photos"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in [
            "Docs/Taxes/tax-return-2025.pdf",
            "Docs/budget.xlsx",
            "Docs/.git/config",
            "Docs/node_modules/pkg/index.js",
            "Docs/.hidden-note.txt",
            "Photos/beach.jpg",
            "Photos/Firefox.lnk",
        ] {
            fs::write(root.join(f), b"x").unwrap();
        }
        root
    }

    #[test]
    fn scan_skips_junk_and_finds_files() {
        let root = tree("scan");
        let files = scan(std::slice::from_ref(&root));
        let names: HashSet<&str> = files.iter().map(|f| f.name()).collect();
        for want in ["Docs", "Taxes", "tax-return-2025.pdf", "budget.xlsx", "Photos", "beach.jpg"] {
            assert!(names.contains(want), "missing {want}: {names:?}");
        }
        for skip in [".git", "config", "node_modules", "index.js", ".hidden-note.txt", "Firefox.lnk"] {
            assert!(!names.contains(skip), "should skip {skip}");
        }
        let taxes = files.iter().find(|f| f.name() == "Taxes").unwrap();
        assert!(taxes.is_dir);
        assert!(taxes.parent().ends_with("Docs"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_ranks_by_name() {
        let root = tree("search");
        let files = scan(std::slice::from_ref(&root));
        let mut s = FileSearcher::new();
        let hits = s.search(&files, &HashMap::new(), "taxret", 5);
        assert_eq!(files[hits[0]].name(), "tax-return-2025.pdf");
        assert!(s.search(&files, &HashMap::new(), "b", 5).is_empty(), "1-char queries skip files");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn prefix() {
        assert_eq!(explicit("f budget"), Some("budget"));
        assert_eq!(explicit("firefox"), None);
        assert_eq!(explicit("f "), None);
    }
}

#[cfg(test)]
mod perf {
    use super::*;

    /// Run with: cargo test --release -- --ignored perf
    #[test]
    #[ignore]
    fn perf_100k() {
        let words = ["report", "invoice", "photo", "budget", "notes", "draft", "final", "scan", "resume", "project"];
        let files: Vec<FileEntry> = (0..MAX_FILES)
            .map(|i| {
                let name = format!("{}_{}_{}.pdf", words[i % 10], words[(i / 10) % 10], i);
                let path = format!("C:\\Users\\me\\Documents\\Folder{}\\{name}", i % 500);
                FileEntry { name_at: (path.len() - name.len()) as u32, path: path.into(), days: 20000, is_dir: false }
            })
            .collect();
        let mut s = FileSearcher::new();
        let usage = HashMap::new();
        for q in ["bu", "budg", "invoice fin", "xyz"] {
            let t = std::time::Instant::now();
            let hits = s.search(&files, &usage, q, 5);
            println!("{q:>12}: {:>6.1} ms, {} hits", t.elapsed().as_secs_f64() * 1000.0, hits.len());
        }
    }
}
