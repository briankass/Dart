//! User settings, stored as simple `key = value` lines in
//! %APPDATA%\Dart\settings.ini so they're easy to edit by hand.

use std::path::PathBuf;

pub const DEFAULT_HOTKEY: &str = "Alt+Space";
pub const HOTKEY_PRESETS: &[&str] = &["Alt+Space", "Ctrl+Space", "Ctrl+Alt+Space", "Alt+Shift+Space"];

// RegisterHotKey modifier flags.
pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    System,
    Dark,
    Light,
}

impl Theme {
    pub fn name(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }

    fn parse(s: &str) -> Option<Theme> {
        match s.trim().to_ascii_lowercase().as_str() {
            "system" | "auto" => Some(Theme::System),
            "dark" => Some(Theme::Dark),
            "light" => Some(Theme::Light),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub hotkey: String,
    pub theme: Theme,
    pub file_search: bool,
    /// Extra folders to index for file search, besides the defaults.
    pub folders: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { hotkey: DEFAULT_HOTKEY.to_string(), theme: Theme::System, file_search: true, folders: Vec::new() }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            match k.trim().to_ascii_lowercase().as_str() {
                "hotkey" if parse_hotkey(v).is_some() => s.hotkey = v.trim().to_string(),
                "theme" => {
                    if let Some(t) = Theme::parse(v) {
                        s.theme = t;
                    }
                }
                "file_search" => match v.trim().to_ascii_lowercase().as_str() {
                    "on" | "true" | "yes" | "1" => s.file_search = true,
                    "off" | "false" | "no" | "0" => s.file_search = false,
                    _ => {}
                },
                "folders" => {
                    s.folders = v
                        .split(';')
                        .map(|f| f.trim().trim_matches('"').to_string())
                        .filter(|f| !f.is_empty())
                        .collect();
                }
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        format!(
            "# Dart settings. Save this file, then reopen the tray menu (or Dart) to apply.\n\
             #\n\
             # hotkey: modifiers (Ctrl, Alt, Shift, Win) plus one key, e.g. Alt+Space,\n\
             #         Ctrl+Shift+D, Win+Alt+K, Ctrl+F12\n\
             # theme:  system, dark or light\n\
             # file_search: on or off\n\
             # folders: extra folders for file search, separated by ; e.g. D:\\Projects; E:\\Music\n\
             \n\
             hotkey = {}\n\
             theme = {}\n\
             file_search = {}\n\
             folders = {}\n",
            self.hotkey,
            self.theme.name(),
            if self.file_search { "on" } else { "off" },
            self.folders.join("; ")
        )
    }
}

pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("Dart").join("settings.ini"))
}

pub fn load() -> Settings {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| Settings::parse(&t))
        .unwrap_or_default()
}

pub fn save(s: &Settings) {
    let Some(p) = path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, s.to_text());
}

/// Last-modified time of the settings file, to notice hand edits.
pub fn modified() -> Option<std::time::SystemTime> {
    std::fs::metadata(path()?).ok()?.modified().ok()
}

/// Parses "Ctrl+Shift+D" into (modifier flags, virtual-key code).
/// Requires at least one modifier so plain typing is never swallowed.
pub fn parse_hotkey(s: &str) -> Option<(u32, u32)> {
    let mut mods = 0;
    let mut key = None;
    for part in s.split('+').map(|p| p.trim()) {
        if part.is_empty() {
            return None;
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "windows" | "super" => mods |= MOD_WIN,
            k => {
                if key.is_some() {
                    return None; // only one non-modifier key
                }
                key = Some(vk_for(k)?);
            }
        }
    }
    if mods == 0 {
        return None;
    }
    Some((mods, key?))
}

fn vk_for(k: &str) -> Option<u32> {
    let named = match k {
        "space" => 0x20,
        "tab" => 0x09,
        "enter" | "return" => 0x0D,
        "esc" | "escape" => 0x1B,
        "backspace" => 0x08,
        "insert" | "ins" => 0x2D,
        "delete" | "del" => 0x2E,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" | "pgup" => 0x21,
        "pagedown" | "pgdn" => 0x22,
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        "`" | "backtick" | "grave" => 0xC0,
        ";" => 0xBA,
        "," => 0xBC,
        "." | "period" => 0xBE,
        "/" | "slash" => 0xBF,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    let chars: Vec<char> = k.chars().collect();
    if chars.len() == 1 {
        let c = chars[0].to_ascii_uppercase();
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys() {
        assert_eq!(parse_hotkey("Alt+Space"), Some((MOD_ALT, 0x20)));
        assert_eq!(parse_hotkey("ctrl + shift + d"), Some((MOD_CONTROL | MOD_SHIFT, 0x44)));
        assert_eq!(parse_hotkey("Win+Alt+K"), Some((MOD_WIN | MOD_ALT, 0x4B)));
        assert_eq!(parse_hotkey("Ctrl+F12"), Some((MOD_CONTROL, 0x7B)));
        assert_eq!(parse_hotkey("Ctrl+1"), Some((MOD_CONTROL, 0x31)));
        assert_eq!(parse_hotkey("Space"), None); // no modifier
        assert_eq!(parse_hotkey("Ctrl+A+B"), None); // two keys
        assert_eq!(parse_hotkey("Ctrl+"), None);
        assert_eq!(parse_hotkey("Ctrl+Banana"), None);
        assert_eq!(parse_hotkey("Ctrl+Alt"), None); // no key
        for p in HOTKEY_PRESETS {
            assert!(parse_hotkey(p).is_some(), "{p}");
        }
    }

    #[test]
    fn settings_roundtrip() {
        let s = Settings {
            hotkey: "Ctrl+Shift+D".into(),
            theme: Theme::Light,
            file_search: false,
            folders: vec![r"D:\Projects".into(), r"E:\My Music".into()],
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
    }

    #[test]
    fn bad_values_fall_back_to_defaults() {
        let s = Settings::parse("hotkey = Banana\ntheme = purple\n# comment\njunk line");
        assert_eq!(s, Settings::default());
        let s = Settings::parse("THEME=Dark\nfolders = \"C:\\A\" ; ;C:\\B");
        assert_eq!(s.theme, Theme::Dark);
        assert_eq!(s.folders, vec![r"C:\A".to_string(), r"C:\B".to_string()]);
        assert!(s.file_search);
    }
}
