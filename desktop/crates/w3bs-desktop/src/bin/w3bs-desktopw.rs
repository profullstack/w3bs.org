//! `w3bs-desktopw`: the same program as `w3bs-desktop`, linked for the Windows GUI subsystem so
//! that running the broker (`daemon`) or the tray at logon opens no console window. Its output
//! goes to `%LOCALAPPDATA%\w3bs\desktop\state\logs\<command>.log` unless the caller supplied
//! standard handles. On other systems it behaves exactly like `w3bs-desktop`.
#![windows_subsystem = "windows"]

fn main() {
    w3bs_desktop::run_background()
}
