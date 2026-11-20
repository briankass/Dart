# Dart

A tiny, instant app launcher for Windows, written in Rust.

- Press **Alt+Space**, type, press **Enter**.
- Fuzzy search over Start Menu, Desktop and Store (UWP) apps, with icons, ranked by how often you launch things.
- Built-in calculator: type `2*21` and press Enter to copy `42`.
- Web search: type `? something` (or `g something`). If nothing matches, Enter searches the web.
- A single small exe, no runtime. It sits in the tray using almost no CPU or RAM.
- **Auto-updates** from GitHub Releases.

## Usage

| Key | Action |
| --- | --- |
| Alt+Space | Show / hide |
| Up / Down | Move selection |
| Enter or click | Launch |
| Ctrl+Enter | Show the file in Explorer |
| Ctrl+Shift+Enter | Run as administrator |
| Left / Right, Home / End | Move the cursor (Ctrl+Left/Right jumps by word) |
| Ctrl+Backspace | Delete the previous word |
| Ctrl+A / Ctrl+C / Ctrl+V | Select all / copy / paste the search text |
| Esc | Hide |

Newly installed apps show up automatically: Dart rescans in the background when you open it
(at most every 30 seconds).

Right-click the tray icon for "Check for updates", "Start with Windows" and "Exit".
Launch counts are stored in `%APPDATA%\Dart\usage.txt`.

## Install

Download `dart.exe` from the [latest release](https://github.com/briankass/Dart/releases/latest)
and put it somewhere you keep programs (for example `%LOCALAPPDATA%\Dart\`). To start it at login,
tick "Start with Windows" in the tray menu (it uses the per-user Run key, so no admin rights are needed).
Keep it in a folder you can write to, since the updater replaces the exe in place.

## Auto-update

Dart checks the latest GitHub release 20 seconds after start and then every 6 hours. If the tag is
newer than the running version it downloads `dart.exe` and `dart.exe.sha256`, verifies the SHA-256,
renames the running exe to `dart.old`, puts the new one in its place, and restarts itself (waiting
until the launcher window is closed if it's open). A tray notification confirms the new version. Only `github.com` download URLs are accepted.

## Releasing

Releases are automatic. Bump `version` in `Cargo.toml` and push to `main`. The `Release`
workflow sees that the version has no release yet, then builds `dart.exe`, creates the `vX.Y.Z`
tag, and publishes the release with its checksum. Pushes that don't change the version only run CI.
You can also start it by hand from the Actions tab ("Run workflow").

## Building

```
cargo build --release
```

On Windows this produces a statically linked `target\release\dart.exe`. Release profile settings
(`opt-level = "z"`, LTO, `panic = "abort"`, strip) keep it small.

## Roadmap

- Configurable hotkey, theme and search engine
- Rescan when apps are installed or removed
- Code signing
