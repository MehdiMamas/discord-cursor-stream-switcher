//! Cursor Stream Switcher: shows the monitor under your mouse on a virtual display, so a
//! Discord screen share of that display follows your cursor across all your monitors.

#![cfg_attr(not(test), windows_subsystem = "windows")]

mod capture;
mod config;
mod cursor_shape;
mod driver;
mod geometry;
mod gpu;
mod guard;
mod hotkey;
mod http;
mod layout;
mod log;
mod mirror;
mod monitors;
mod paths;
mod selector;
mod shared;
mod system;
mod tray;
mod update;

use config::Config;
use shared::Shared;
use std::sync::Arc;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::core::w;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(String::as_str).unwrap_or("");

    // Elevated helper commands started by the tray app (UAC prompt).
    if command.starts_with("driver-") {
        log::init(&paths::data_dir().join("driver.log"));
        std::process::exit(driver::cli(command, &args[2..]));
    }

    log::init(&paths::log_file());
    // The manifest already asks for this; calling it again is harmless and covers odd launchers.
    // SAFETY: process-wide setting made before any window exists.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    // One instance per user session. After an update the old process may still be exiting.
    // The installer looks for this mutex and the tray window to close the app before upgrading.
    // SAFETY: named mutex owned for the life of the process.
    let mutex = unsafe { CreateMutexW(None, true, w!("Local\\CursorStreamSwitcher.SingleInstance")) };
    let Ok(mutex) = mutex else {
        system::info_box("Could not start (mutex).");
        return;
    };
    // SAFETY: reads the error from CreateMutexW above.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let wait_ms = if command == "--after-update" { 15_000 } else { 0 };
        // SAFETY: waiting on our own handle.
        let r = unsafe { WaitForSingleObject(mutex, wait_ms) };
        if r != WAIT_OBJECT_0 && r != WAIT_ABANDONED {
            if wait_ms == 0 {
                system::info_box(
                    "Cursor Stream Switcher is already running. Look for its icon in the taskbar tray.",
                );
            }
            return;
        }
    }

    info!(
        "starting version {} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()
    );
    update::cleanup();
    system::refresh_autostart();

    let (cfg, problem) = Config::load(&paths::config_file());
    if let Some(p) = &problem {
        warn!("{p}");
    }
    let shared = Arc::new(Shared::new());
    guard::start();
    let mirror = mirror::spawn(shared.clone(), cfg.clone());

    let code = tray::run(shared.clone(), cfg, problem);

    shared.send(shared::Command::Quit);
    let _ = mirror.join();
    guard::stop();
    info!("stopped");
    // SAFETY: releasing the mutex this thread owns.
    unsafe {
        let _ = ReleaseMutex(mutex);
        let _ = CloseHandle(mutex);
    }
    std::process::exit(code);
}
