//! HurshNote 主程序入口。

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod crash;
#[cfg(windows)]
mod dialogs;
#[cfg(windows)]
mod menu;
#[cfg(windows)]
mod selftest;
#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    std::process::exit(app::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("HurshNote 只支持 Windows。");
    std::process::exit(1);
}
