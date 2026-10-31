//! HurshNote 主程序入口。

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod commands;
#[cfg(windows)]
mod config;
#[cfg(windows)]
mod crash;
#[cfg(windows)]
mod crashdump;
#[cfg(windows)]
mod dialogs;
#[cfg(windows)]
mod dlg;
#[cfg(windows)]
mod docs;
#[cfg(windows)]
mod editops;
#[cfg(windows)]
mod finddlg;
#[cfg(windows)]
mod fsio;
#[cfg(windows)]
mod instance;
#[cfg(windows)]
mod pwdlg;
#[cfg(windows)]
mod selftest;
#[cfg(windows)]
mod statusbar;
#[cfg(windows)]
mod watcher;
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
