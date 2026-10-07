#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod control_panel;

#[cfg(windows)]
fn main() {
    if let Err(error) = control_panel::run() {
        control_panel::show_error(&error.to_string());
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Touhou WASD supports Windows 10 and Windows 11 only.");
}
