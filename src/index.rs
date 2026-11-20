//! Builds the list of launchable things by scanning Start Menu folders.

use crate::search::Entry;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const EXTENSIONS: &[&str] = &["lnk", "exe", "url", "appref-ms"];
const MAX_DEPTH: usize = 6;

fn roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("ProgramData") {
        v.push(PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Some(p) = std::env::var_os("APPDATA") {
        v.push(PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Some(p) = std::env::var_os("USERPROFILE") {
        v.push(PathBuf::from(p).join("Desktop"));
    }
    if let Some(p) = std::env::var_os("PUBLIC") {
        v.push(PathBuf::from(p).join("Desktop"));
    }
    v
}

pub fn scan() -> Vec<Entry> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for root in roots() {
        walk(&root, 0, &mut out, &mut seen);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<Entry>, seen: &mut HashSet<String>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for item in rd.flatten() {
        let path = item.path();
        let Ok(ft) = item.file_type() else { continue };
        if ft.is_dir() {
            walk(&path, depth + 1, out, seen);
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase());
        let Some(ext) = ext else { continue };
        if !EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let lower = name.to_lowercase();
        if lower.contains("uninstall") || lower.starts_with("remove ") {
            continue;
        }
        if seen.insert(lower) {
            out.push(Entry { name: name.to_string(), path: path.to_string_lossy().into_owned() });
        }
    }
}
