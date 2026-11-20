#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod index;
mod search;
mod update;
#[cfg(windows)]
mod ui;

#[cfg(windows)]
fn main() {
    ui::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Dart is a Windows application (v{}).", update::CURRENT);
}
