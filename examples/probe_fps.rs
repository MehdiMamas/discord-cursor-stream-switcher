//! Diagnostic: counts how many new frames a display really produces, the way a screen
//! recorder (or Discord) would see them through Desktop Duplication.
//!
//!   cargo run --release --example probe_fps -- \\.\DISPLAY3 10
//!
//! Prints frames per second for each second of the run.

use std::time::{Duration, Instant};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIFactory1, IDXGIOutput1,
    IDXGIResource,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::core::Interface;

fn main() -> windows::core::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let target = args.get(1).cloned().unwrap_or_else(|| r"\\.\DISPLAY1".into());
    let seconds: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    // SAFETY: plain Win32/DXGI calls with valid arguments.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let mut j = 0;
            while let Ok(output) = adapter.EnumOutputs(j) {
                j += 1;
                let desc = output.GetDesc()?;
                let len = desc.DeviceName.iter().position(|&c| c == 0).unwrap_or(32);
                if !String::from_utf16_lossy(&desc.DeviceName[..len]).eq_ignore_ascii_case(&target) {
                    continue;
                }
                let mut device: Option<ID3D11Device> = None;
                D3D11CreateDevice(
                    &adapter,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )?;
                let device = device.unwrap();
                let dup = output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?;
                println!("probing {target} for {seconds} s");
                let start = Instant::now();
                let mut second = Instant::now();
                let (mut frames, mut total) = (0u32, 0u32);
                while start.elapsed() < Duration::from_secs(seconds) {
                    let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
                    let mut res: Option<IDXGIResource> = None;
                    match dup.AcquireNextFrame(100, &mut info, &mut res) {
                        Ok(()) => {
                            if info.LastPresentTime != 0 {
                                frames += 1;
                                total += 1;
                            }
                            dup.ReleaseFrame()?;
                        }
                        Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
                        Err(e) => return Err(e),
                    }
                    if second.elapsed() >= Duration::from_secs(1) {
                        print!("{frames} ");
                        frames = 0;
                        second = Instant::now();
                    }
                }
                println!("\naverage {:.1} fps", f64::from(total) / start.elapsed().as_secs_f64());
                return Ok(());
            }
        }
    }
    eprintln!("display {target} not found");
    Ok(())
}
