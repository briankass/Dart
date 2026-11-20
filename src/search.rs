//! Fuzzy matching plus a persisted launch-count ("frecency") boost.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: String,
}

pub struct Searcher {
    matcher: Matcher,
    buf: Vec<char>,
}

impl Searcher {
    pub fn new() -> Self {
        Self { matcher: Matcher::new(Config::DEFAULT), buf: Vec::new() }
    }

    /// Returns indices into `entries`, best first, at most `limit` of them.
    pub fn search(
        &mut self,
        entries: &[Entry],
        usage: &HashMap<String, u32>,
        query: &str,
        limit: usize,
    ) -> Vec<usize> {
        let boost = |e: &Entry| usage.get(&e.path).copied().unwrap_or(0).min(50);
        let query = query.trim();

        let mut scored: Vec<(u32, usize)> = if query.is_empty() {
            // No query: show the most-used entries.
            entries
                .iter()
                .enumerate()
                .filter(|(_, e)| boost(e) > 0)
                .map(|(i, e)| (boost(e), i))
                .collect()
        } else {
            let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
            let mut out = Vec::new();
            for (i, e) in entries.iter().enumerate() {
                let hay = Utf32Str::new(&e.name, &mut self.buf);
                if let Some(s) = pattern.score(hay, &mut self.matcher) {
                    out.push((s + boost(e) * 4, i));
                }
            }
            out
        };

        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| entries[a.1].name.cmp(&entries[b.1].name)));
        scored.truncate(limit);
        scored.into_iter().map(|(_, i)| i).collect()
    }
}

fn usage_file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("Dart").join("usage.txt"))
}

pub fn load_usage() -> HashMap<String, u32> {
    let mut map = HashMap::new();
    if let Some(text) = usage_file().and_then(|p| std::fs::read_to_string(p).ok()) {
        for line in text.lines() {
            if let Some((count, path)) = line.split_once('\t') {
                if let Ok(c) = count.parse() {
                    map.insert(path.to_string(), c);
                }
            }
        }
    }
    map
}

pub fn save_usage(usage: &HashMap<String, u32>) {
    let Some(file) = usage_file() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut out = String::new();
    for (path, count) in usage {
        out.push_str(&format!("{count}\t{path}\n"));
    }
    let _ = std::fs::write(file, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<Entry> {
        ["Visual Studio Code", "Firefox", "File Explorer", "Notepad", "Calculator"]
            .iter()
            .map(|n| Entry { name: n.to_string(), path: format!("C:\\{n}.lnk") })
            .collect()
    }

    #[test]
    fn fuzzy_matches_subsequence() {
        let e = entries();
        let mut s = Searcher::new();
        let hits = s.search(&e, &HashMap::new(), "vsc", 5);
        assert_eq!(e[hits[0]].name, "Visual Studio Code");
    }

    #[test]
    fn usage_boosts_ranking() {
        let e = entries();
        let mut s = Searcher::new();
        let mut usage = HashMap::new();
        usage.insert(e[2].path.clone(), 40); // File Explorer
        let hits = s.search(&e, &usage, "fi", 5);
        assert_eq!(e[hits[0]].name, "File Explorer");
    }

    #[test]
    fn empty_query_lists_most_used() {
        let e = entries();
        let mut s = Searcher::new();
        let mut usage = HashMap::new();
        usage.insert(e[3].path.clone(), 3);
        assert_eq!(s.search(&e, &usage, "", 5), vec![3]);
    }
}
