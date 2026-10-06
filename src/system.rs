//! Small OS helpers: elevation, autostart, opening files, message boxes.

use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HWND};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONINFORMATION, MB_OK, MESSAGEBOX_STYLE, MessageBoxW, SW_HIDE, SW_SHOWNORMAL,
};
use windows::core::{HSTRING, PCWSTR, w};

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const RUN_VALUE: PCWSTR = w!("CursorStreamSwitcher");

/// Runs this .exe elevated with `args` (UAC prompt) and waits for its exit code.
pub fn run_elevated(args: &str) -> Result<u32, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: strings outlive the call; the returned process handle is closed below.
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
                "cancelled".into()
            } else {
                e.to_string()
            });
        }
        let mut code = 1u32;
        if !info.hProcess.is_invalid() {
            WaitForSingleObject(info.hProcess, INFINITE);
            let _ = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
        }
        Ok(code)
    }
}

fn run_command() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    format!("\"{exe}\"")
}

fn run_value() -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: buffer and size describe the same allocation.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok().then(|| crate::monitors::wide_to_string(&buf))
}

pub fn autostart_enabled() -> bool {
    run_value().is_some()
}

pub fn set_autostart(enable: bool) -> Result<(), String> {
    // SAFETY: registry writes to the current user's Run key with valid buffers.
    unsafe {
        if enable {
            let value: Vec<u16> = run_command().encode_utf16().chain([0]).collect();
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(value.as_ptr() as *const _),
                (value.len() * 2) as u32,
            )
            .ok()
            .map_err(|e| e.to_string())
        } else {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
            if r.is_ok() || r.0 == 2 { Ok(()) } else { Err(format!("{r:?}")) }
        }
    }
}

/// Keeps the autostart entry pointing at this .exe if the user moved it.
pub fn refresh_autostart() {
    if run_value().is_some_and(|v| v != run_command()) {
        let _ = set_autostart(true);
    }
}

pub fn open(target: &str) {
    let target = HSTRING::from(target);
    // SAFETY: valid strings; the shell opens the file or URL with its default handler.
    unsafe {
        ShellExecuteW(None, w!("open"), &target, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn open_with_notepad(path: &str) {
    let path = HSTRING::from(format!("\"{path}\""));
    // SAFETY: valid strings.
    unsafe {
        ShellExecuteW(None, w!("open"), w!("notepad.exe"), &path, PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn message_box(text: &str, style: MESSAGEBOX_STYLE) {
    let text = HSTRING::from(text);
    // SAFETY: valid strings; no owner window.
    unsafe {
        MessageBoxW(None::<HWND>, &text, w!("Cursor Stream Switcher"), style | MB_OK);
    }
}

pub fn info_box(text: &str) {
    message_box(text, MB_ICONINFORMATION);
}
