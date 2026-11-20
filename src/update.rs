//! Self-update from GitHub Releases.
//!
//! Flow: ask the GitHub API for the latest release, compare its tag with our
//! version, download `dart.exe` + `dart.exe.sha256`, verify the checksum, then
//! swap the running exe (Windows lets you rename a running exe, just not
//! overwrite it). The new version runs the next time Dart starts.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const REPO: &str = "briankass/Dart";
const EXE_ASSET: &str = "dart.exe";
const SUM_ASSET: &str = "dart.exe.sha256";
const MAX_DOWNLOAD: u64 = 32 * 1024 * 1024;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, PartialEq)]
pub enum Outcome {
    UpToDate,
    Updated(String),
}

/// Parses "v1.2.3" / "1.2.3" into comparable numbers.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next().unwrap_or("0").parse().ok()?;
    let c = it.next().unwrap_or("0").parse().ok()?;
    Some((a, b, c))
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

fn agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .timeout(Duration::from_secs(60))
        .user_agent("dart-launcher-updater")
        .build())
}

fn get_bytes(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    resp.into_reader().take(MAX_DOWNLOAD + 1).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() as u64 > MAX_DOWNLOAD {
        return Err("download too large".into());
    }
    Ok(buf)
}

/// Removes leftovers from a previous update. Call once at startup.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("old"));
        let _ = std::fs::remove_file(exe.with_extension("new"));
    }
}

pub fn check_and_apply() -> Result<Outcome, String> {
    let agent = agent()?;
    let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = get_bytes(&agent, &api)?;
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;

    let tag = json["tag_name"].as_str().ok_or("no tag_name")?.to_string();
    if !is_newer(&tag, CURRENT) {
        return Ok(Outcome::UpToDate);
    }

    let find = |name: &str| -> Option<String> {
        json["assets"].as_array()?.iter().find_map(|a| {
            (a["name"].as_str()? == name).then(|| a["browser_download_url"].as_str().map(String::from))?
        })
    };
    let exe_url = find(EXE_ASSET).ok_or("release has no dart.exe")?;
    let sum_url = find(SUM_ASSET).ok_or("release has no checksum")?;

    // Only ever download from GitHub.
    for u in [&exe_url, &sum_url] {
        if !u.starts_with("https://github.com/") {
            return Err("unexpected download host".into());
        }
    }

    let expected = String::from_utf8(get_bytes(&agent, &sum_url)?).map_err(|e| e.to_string())?;
    let expected = expected.split_whitespace().next().unwrap_or("").to_lowercase();
    let data = get_bytes(&agent, &exe_url)?;
    let actual: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
    if expected.len() != 64 || expected != actual {
        return Err("checksum mismatch".into());
    }
    if data.len() < 2 || &data[..2] != b"MZ" {
        return Err("downloaded file is not an executable".into());
    }

    swap_in(&data)?;
    Ok(Outcome::Updated(tag))
}

fn swap_in(data: &[u8]) -> Result<(), String> {
    let exe: PathBuf = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = exe.with_extension("new");
    let old = exe.with_extension("old");
    std::fs::write(&new, data).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe); // roll back
        return Err(e.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10"), Some((0, 10, 0)));
        assert_eq!(parse_version("1.0.0-beta.1"), Some((1, 0, 0)));
        assert_eq!(parse_version("nope"), None);
    }

    #[test]
    fn newer_comparison() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("v0.10.0", "0.9.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }
}
