//! Minimal HTTPS GET using the WinHTTP that ships with Windows: system TLS and proxy settings,
//! no bundled TLS stack. Only ever used for github.com (see update.rs).

use windows::Win32::Networking::WinHttp::*;
use windows::core::{HSTRING, PCWSTR, w};

struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: handle was returned by WinHTTP and is closed exactly once.
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn check(h: *mut core::ffi::c_void, what: &str) -> Result<Handle, String> {
    if h.is_null() {
        Err(format!("{what} failed: {}", windows::core::Error::from_thread()))
    } else {
        Ok(Handle(h))
    }
}

/// GETs `https://{host}{path}`, following redirects, and returns the body. Fails on non-200
/// responses and on bodies larger than `max_bytes`.
pub fn get(host: &str, path: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    let agent = HSTRING::from(format!("CursorStreamSwitcher/{}", env!("CARGO_PKG_VERSION")));
    let host = HSTRING::from(host);
    let path = HSTRING::from(path);
    // SAFETY: every handle is checked and closed by `Handle`; buffers outlive the calls.
    unsafe {
        let session = check(
            WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0),
            "WinHttpOpen",
        )?;
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 30_000).map_err(|e| e.to_string())?;
        let connect =
            check(WinHttpConnect(session.0, &host, INTERNET_DEFAULT_HTTPS_PORT, 0), "WinHttpConnect")?;
        let request = check(
            WinHttpOpenRequest(
                connect.0,
                w!("GET"),
                &path,
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ),
            "WinHttpOpenRequest",
        )?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(|e| format!("request failed: {e}"))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| format!("no response: {e}"))?;

        let mut status = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut _),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(|e| e.to_string())?;
        if status != 200 {
            return Err(format!("HTTP {status}"));
        }

        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut available).map_err(|e| e.to_string())?;
            if available == 0 {
                break;
            }
            if body.len() + available as usize > max_bytes {
                return Err(format!("response larger than {max_bytes} bytes"));
            }
            let start = body.len();
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(request.0, body[start..].as_mut_ptr() as *mut _, available, &mut read)
                .map_err(|e| e.to_string())?;
            body.truncate(start + read as usize);
        }
        Ok(body)
    }
}
