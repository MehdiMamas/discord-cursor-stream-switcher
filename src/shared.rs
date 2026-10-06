//! State shared between the tray (UI) thread and the mirror (render) thread.

use crate::config::Config;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted to the tray window whenever `Status` changes.
pub const WM_STATUS_CHANGED: u32 = WM_APP + 2;

pub enum Command {
    Reload(Config),
    SetPaused(bool),
    SetLocked(bool),
    DisplaysChanged,
    Quit,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// The virtual display the stream is drawn on, if it exists.
    pub stream_display: Option<String>,
    /// Label of the monitor currently shown.
    pub source: Option<String>,
    pub paused: bool,
    pub locked: bool,
    pub source_hidden: bool,
    /// Last capture problem, for the tray tooltip.
    pub problem: Option<String>,
    /// How many times the stream display was moved out from between monitors.
    pub layout_fixes: u32,
}

pub struct Shared {
    status: Mutex<Status>,
    tray_hwnd: AtomicIsize,
    wake: HANDLE,
    commands: Mutex<Vec<Command>>,
}

// SAFETY: the event HANDLE is a kernel object usable from any thread.
unsafe impl Send for Shared {}
// SAFETY: see above; everything else is behind a Mutex or atomic.
unsafe impl Sync for Shared {}

impl Shared {
    pub fn new() -> Self {
        // SAFETY: auto-reset unnamed event; lives for the whole process.
        let wake = unsafe { CreateEventW(None, false, false, None) }.expect("CreateEventW failed");
        Self {
            status: Mutex::new(Status::default()),
            tray_hwnd: AtomicIsize::new(0),
            wake,
            commands: Mutex::new(Vec::new()),
        }
    }

    pub fn wake_handle(&self) -> HANDLE {
        self.wake
    }

    pub fn set_tray_hwnd(&self, hwnd: HWND) {
        self.tray_hwnd.store(hwnd.0 as isize, Ordering::Release);
    }

    pub fn send(&self, cmd: Command) {
        self.commands.lock().unwrap_or_else(|e| e.into_inner()).push(cmd);
        // SAFETY: valid event handle.
        unsafe {
            let _ = SetEvent(self.wake);
        }
    }

    pub fn take_commands(&self) -> Vec<Command> {
        std::mem::take(&mut *self.commands.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Updates the status and tells the tray when something actually changed.
    pub fn update_status(&self, f: impl FnOnce(&mut Status)) {
        let changed = {
            let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            let before = s.clone();
            f(&mut s);
            *s != before
        };
        let hwnd = self.tray_hwnd.load(Ordering::Acquire);
        if changed && hwnd != 0 {
            // SAFETY: posting to a window owned by this process; failure is harmless.
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_STATUS_CHANGED, WPARAM(0), LPARAM(0));
            }
        }
    }
}
