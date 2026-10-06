//! Keeps the mouse off the virtual stream display. The display has no physical screen, so a
//! cursor that wandered onto it would simply vanish for the user.
//!
//! A low-level mouse hook runs on its own thread (it does nothing but compare rectangles) and
//! blocks any move that would land on the stream display, pinning the cursor to the nearest
//! point on a real monitor instead.

use crate::geometry::{Rect, nearest_point};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, HHOOK, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, SetCursorPos, SetWindowsHookExW, UnhookWindowsHookEx, WH_MOUSE_LL, WM_APP,
    WM_MOUSEMOVE, WM_QUIT,
};

struct Zones {
    blocked: Option<Rect>,
    allowed: Vec<Rect>,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static ZONES: Mutex<Zones> = Mutex::new(Zones { blocked: None, allowed: Vec::new() });
static THREAD_ID: AtomicU32 = AtomicU32::new(0);

/// Thread message asking the guard thread to install or remove the hook to match ENABLED.
const WM_GUARD_SYNC: u32 = WM_APP + 1;

/// Updates what the hook blocks. Cheap; called whenever the display layout changes. The hook is
/// only installed while it is enabled, so a disabled guard costs nothing per mouse move.
pub fn configure(enabled: bool, blocked: Option<Rect>, allowed: Vec<Rect>) {
    {
        let mut z = ZONES.lock().unwrap_or_else(|e| e.into_inner());
        z.blocked = blocked;
        z.allowed = allowed;
    }
    let enabled = enabled && blocked.is_some();
    if ENABLED.swap(enabled, Ordering::AcqRel) != enabled {
        post(WM_GUARD_SYNC);
    }
}

fn post(msg: u32) {
    let id = THREAD_ID.load(Ordering::Acquire);
    if id != 0 {
        // SAFETY: posting to our own guard thread.
        unsafe {
            let _ = PostThreadMessageW(id, msg, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe extern "system" fn hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && wp.0 as u32 == WM_MOUSEMOVE && ENABLED.load(Ordering::Acquire) {
        // SAFETY: for WH_MOUSE_LL with HC_ACTION, lParam points to an MSLLHOOKSTRUCT.
        let info = unsafe { &*(lp.0 as *const MSLLHOOKSTRUCT) };
        // try_lock: never stall the system mouse while configure() holds the lock.
        if let Ok(z) = ZONES.try_lock()
            && let Some(blocked) = z.blocked
            && blocked.contains(info.pt.x, info.pt.y)
            && let Some((x, y)) = nearest_point(&z.allowed, info.pt.x, info.pt.y)
        {
            // SAFETY: plain cursor move; returning non-zero swallows the original move.
            unsafe {
                let _ = SetCursorPos(x, y);
            }
            return LRESULT(1);
        }
    }
    // SAFETY: pass everything else down the hook chain unchanged.
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

/// Starts the guard thread. It lives until `stop()` or process exit.
pub fn start() {
    std::thread::Builder::new()
        .name("mouse-guard".into())
        .spawn(|| {
            // SAFETY: installs and removes a hook owned by this thread, which pumps messages.
            unsafe {
                let mut msg = MSG::default();
                // Make sure the thread has a message queue before others post to it.
                let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                THREAD_ID.store(GetCurrentThreadId(), Ordering::Release);
                // The system waits on this thread for every mouse move; keep it responsive.
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
                let module = GetModuleHandleW(None).ok();
                let mut handle: Option<HHOOK> = None;
                loop {
                    let want = ENABLED.load(Ordering::Acquire);
                    if want && handle.is_none() {
                        match SetWindowsHookExW(WH_MOUSE_LL, Some(hook), module.map(Into::into), 0) {
                            Ok(h) => handle = Some(h),
                            Err(e) => crate::error!("mouse guard hook failed: {e}"),
                        }
                    } else if !want && let Some(h) = handle.take() {
                        let _ = UnhookWindowsHookEx(h);
                    }
                    if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        break;
                    }
                }
                if let Some(h) = handle {
                    let _ = UnhookWindowsHookEx(h);
                }
            }
        })
        .expect("failed to start mouse guard thread");
}

pub fn stop() {
    post(WM_QUIT);
}
