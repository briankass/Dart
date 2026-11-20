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
| Enter | Launch |
| Esc | Hide |

Right-click the tray icon for "Check for updates" and "Exit".
Launch counts are stored in `%APPDATA%\Dart\usage.txt`.

## Install

Download `dart.exe` from the [latest release](https://github.com/briankass/Dart/releases/latest)
and put it somewhere you keep programs (for example `%LOCALAPPDATA%\Dart\`). To start it at login,
put a shortcut to it in `shell:startup`. Keep it in a folder you can write to, since the updater
replaces the exe in place.

## Auto-update

Dart checks the latest GitHub release 20 seconds after start and then every 6 hours. If the tag is
newer than the running version it downloads `dart.exe` and `dart.exe.sha256`, verifies the SHA-256,
renames the running exe to `dart.old`, and puts the new one in its place. The new version runs the
next time Dart starts. Only `github.com` download URLs are accepted.

## Releasing

1. Bump `version` in `Cargo.toml`.
2. Commit, then `git tag v0.2.0 && git push --tags`.
3. The `Release` workflow builds `dart.exe`, attaches it with its checksum, and publishes the release.

The workflow fails if the tag and `Cargo.toml` version differ.

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
