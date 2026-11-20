#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod calc;
mod index;
mod search;
mod textedit;
mod update;
mod web;
#[cfg(windows)]
mod apps;
#[cfg(windows)]
mod startup;
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
