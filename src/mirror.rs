//! The mirror thread. It owns a borderless window that covers the virtual stream display and
//! draws the monitor under the cursor into it.
//!
//! Cost control:
//! - Only the monitor being shown is duplicated; the others cost nothing.
//! - Only changed regions are copied; a still screen means no GPU work and no presents.
//! - Frames are capped at `max_fps` (60 by default, Discord's maximum).
//! - The capture device is created on the GPU that drives the shown monitor, so frames never
//!   leave that GPU on their way to the swap chain.

use crate::capture::{self, Capture, CaptureError, find_output, hr};
use crate::config::Config;
use crate::geometry::{self, RectF, Rotation};
use crate::gpu::{Blend, Gpu, adapter_luid};
use crate::layout;
use crate::monitors::{self, Monitor};
use crate::selector::Selector;
use crate::shared::{Command, Shared};
use crate::{error, guard, info, warn};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DRAW_TEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::Direct3D11::{D3D11_VIEWPORT, ID3D11RenderTargetView, ID3D11Texture2D};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_MWA_NO_ALT_ENTER, DXGI_MWA_NO_WINDOW_CHANGES, DXGI_PRESENT,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter1, IDXGIDevice, IDXGIFactory1, IDXGIFactory2, IDXGIOutput,
    IDXGISurface, IDXGISwapChain1,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, PCWSTR, Result, w};

const CLASS_NAME: PCWSTR = w!("CursorStreamSwitcher.Mirror");

thread_local! {
    static DISPLAY_CHANGED: Cell<bool> = const { Cell::new(false) };
}

/// Starts the mirror thread. It restarts itself after a panic so one bad frame can't end the stream.
pub fn spawn(shared: Arc<Shared>, cfg: Config) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("mirror".into())
        .spawn(move || {
            let mut cfg = cfg;
            loop {
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match Mirror::new(&shared, &mut cfg) {
                        Ok(mut m) => m.run(),
                        Err(e) => {
                            error!("mirror window could not be created: {}", hr(&e));
                            false
                        }
                    }
                }));
                match run {
                    Ok(true) => break,
                    Ok(false) => std::thread::sleep(Duration::from_secs(2)),
                    Err(_) => {
                        error!("mirror thread panicked; restarting");
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            }
            guard::configure(false, None, Vec::new());
        })
        .expect("failed to start mirror thread")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Card {
    Paused,
    Hidden,
    NoSource,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shown {
    Nothing,
    Mirror,
    Card(Card),
}

struct Presenter {
    gpu: Rc<Gpu>,
    swap: IDXGISwapChain1,
    rtv: Option<ID3D11RenderTargetView>,
    width: u32,
    height: u32,
}

impl Drop for Presenter {
    fn drop(&mut self) {
        // The immediate context retains the bound back buffer even after our RTV is dropped.
        // Release it and flush deferred destruction before creating another flip-model swap
        // chain for this HWND (in particular when the source moves to a different GPU).
        unsafe {
            self.gpu.ctx.ClearState();
            self.gpu.d2d.SetTarget(None);
        }
        self.rtv = None;
        unsafe { self.gpu.ctx.Flush() };
    }
}

impl Presenter {
    fn new(gpu: Rc<Gpu>, hwnd: HWND, width: u32, height: u32) -> Result<Self> {
        // SAFETY: COM calls with valid arguments; the window belongs to this thread.
        unsafe {
            let factory: IDXGIFactory2 = gpu.device.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let swap = factory.CreateSwapChainForHwnd(&gpu.device, hwnd, &desc, None, None)?;
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER | DXGI_MWA_NO_WINDOW_CHANGES)?;
            Ok(Self { gpu, swap, rtv: None, width, height })
        }
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        if (width, height) == (self.width, self.height) {
            return Ok(());
        }
        // SAFETY: unbind the context's reference as well as our own before resizing.
        unsafe { self.gpu.ctx.OMSetRenderTargets(None, None) };
        self.rtv = None;
        // SAFETY: no views of the back buffers are alive.
        unsafe { self.swap.ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))? };
        self.width = width;
        self.height = height;
        Ok(())
    }

    fn begin(&mut self) -> Result<()> {
        if self.rtv.is_none() {
            // SAFETY: buffer 0 is the current back buffer of a D3D11 flip-model swap chain.
            unsafe {
                let back: ID3D11Texture2D = self.swap.GetBuffer(0)?;
                let mut rtv = None;
                self.gpu.device.CreateRenderTargetView(&back, None, Some(&mut rtv))?;
                self.rtv = rtv;
            }
        }
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: self.width as f32,
            Height: self.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        // SAFETY: the RTV belongs to this presenter's device.
        unsafe {
            self.gpu.ctx.OMSetRenderTargets(Some(std::slice::from_ref(&self.rtv)), None);
            self.gpu.ctx.RSSetViewports(Some(&[viewport]));
            if let Some(rtv) = &self.rtv {
                self.gpu.ctx.ClearRenderTargetView(rtv, &[0.0, 0.0, 0.0, 1.0]);
            }
        }
        Ok(())
    }

    fn present(&self) -> Result<()> {
        // Sync interval 0: the vblank that Windows simulates for virtual displays is unreliable
        // (presents were seen blocking for 70-250 ms), so frames are paced by the mirror loop.
        // SAFETY: plain present of the current back buffer.
        unsafe { self.swap.Present(0, DXGI_PRESENT(0)).ok() }
    }

    /// Fills the screen with a short message (paused, hidden screen...).
    fn draw_card(&mut self, title: &str, subtitle: &str) -> Result<()> {
        self.rtv = None;
        let gpu = &self.gpu;
        // SAFETY: D2D draws into the swap chain's current back buffer; the bitmap is released
        // before the next ResizeBuffers or Present.
        unsafe {
            let surface: IDXGISurface = self.swap.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let bitmap = gpu.d2d.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            gpu.d2d.SetDpi(96.0, 96.0);
            gpu.d2d.SetTarget(&bitmap);
            gpu.d2d.BeginDraw();
            gpu.d2d.Clear(Some(&D2D1_COLOR_F { r: 0.08, g: 0.085, b: 0.1, a: 1.0 }));
            let (w, h) = (self.width as f32, self.height as f32);
            let text = |s: &str, size: f32, bold: bool, top: f32, bottom: f32, gray: f32| -> Result<()> {
                if s.is_empty() {
                    return Ok(());
                }
                let format = gpu.dwrite.CreateTextFormat(
                    w!("Segoe UI"),
                    None,
                    if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL },
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )?;
                format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                let brush = gpu
                    .d2d
                    .CreateSolidColorBrush(&D2D1_COLOR_F { r: gray, g: gray, b: gray, a: 1.0 }, None)?;
                let wide: Vec<u16> = s.encode_utf16().collect();
                let rect = D2D_RECT_F { left: 0.0, top, right: w, bottom };
                gpu.d2d.DrawText(
                    &wide,
                    &format,
                    &rect,
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                Ok(())
            };
            let drawn = text(title, h * 0.055, true, h * 0.38, h * 0.5, 0.95)
                .and_then(|_| text(subtitle, h * 0.028, false, h * 0.5, h * 0.57, 0.65));
            let ended = gpu.d2d.EndDraw(None, None);
            gpu.d2d.SetTarget(None);
            drawn?;
            ended?;
        }
        self.present()
    }
}

/// 1 ms timer resolution while frames are flowing, so 16 ms waits don't round up to 31 ms
/// (the default 15.6 ms tick). Released as soon as the screen goes quiet. Since Windows 10
/// 2004 this only affects this process.
#[derive(Default)]
struct FineTimer(bool);

impl FineTimer {
    fn set(&mut self, on: bool) {
        if on != self.0 {
            // SAFETY: balanced begin/end calls, tracked by self.0.
            unsafe {
                if on {
                    windows::Win32::Media::timeBeginPeriod(1);
                } else {
                    windows::Win32::Media::timeEndPeriod(1);
                }
            }
            self.0 = on;
        }
    }
}

impl Drop for FineTimer {
    fn drop(&mut self) {
        self.set(false);
    }
}

struct Source {
    monitor: Monitor,
    gpu: Rc<Gpu>,
    output: IDXGIOutput,
    capture: Option<Capture>,
    retry_at: Instant,
    failures: u32,
    /// When this monitor was picked, to log how long the first frame took.
    picked_at: Instant,
    first_frame_logged: bool,
}

struct Mirror<'a> {
    shared: &'a Shared,
    cfg: &'a mut Config,
    hwnd: HWND,
    paused: bool,
    locked: bool,
    monitors: Vec<Monitor>,
    stream: Option<Monitor>,
    selector: Selector,
    gpus: Vec<Rc<Gpu>>,
    presenter: Option<Presenter>,
    source: Option<Source>,
    shown: Shown,
    redraw: bool,
    /// A change is waiting to be drawn (it arrived before the next frame slot).
    frame_pending: bool,
    last_present: Instant,
    /// Last cursor position (desktop pixels) and when it last changed. While the mouse moves,
    /// the loop samples it every frame: Desktop Duplication reports pure cursor moves only
    /// about 12 times a second, which would make the streamed cursor stutter.
    cursor_pos: Option<(i32, i32)>,
    cursor_moved_at: Instant,
    fine_timer: FineTimer,
    last_layout_fix: Option<Instant>,
    /// Frames drawn since `stats_since`, for a once-a-minute log line.
    frames: u32,
    stats_since: Instant,
}

impl Drop for Mirror<'_> {
    fn drop(&mut self) {
        self.source = None;
        self.presenter = None;
        // SAFETY: the window was created by this thread.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

impl<'a> Mirror<'a> {
    fn new(shared: &'a Shared, cfg: &'a mut Config) -> Result<Self> {
        let hwnd = create_window()?;
        let status = shared.status();
        Ok(Self {
            shared,
            cfg,
            hwnd,
            paused: status.paused,
            locked: status.locked,
            monitors: Vec::new(),
            stream: None,
            selector: Selector::default(),
            gpus: Vec::new(),
            presenter: None,
            source: None,
            shown: Shown::Nothing,
            redraw: true,
            frame_pending: false,
            last_present: Instant::now(),
            cursor_pos: None,
            cursor_moved_at: Instant::now(),
            fine_timer: FineTimer::default(),
            last_layout_fix: None,
            frames: 0,
            stats_since: Instant::now(),
        })
    }

    /// Returns true when asked to quit, false to be restarted.
    fn run(&mut self) -> bool {
        self.refresh_displays();
        loop {
            if !pump_messages() {
                return true;
            }
            if DISPLAY_CHANGED.with(|c| c.replace(false)) {
                self.refresh_displays();
            }
            for cmd in self.shared.take_commands() {
                match cmd {
                    Command::Quit => return true,
                    Command::Reload(cfg) => {
                        let display_changed = cfg.stream_display != self.cfg.stream_display;
                        *self.cfg = cfg;
                        if display_changed {
                            self.selector.reset();
                            self.source = None;
                        }
                        self.refresh_displays();
                        let hidden = self.source.as_ref().is_some_and(|s| self.cfg.is_hidden(&s.monitor.id));
                        self.shared.update_status(|s| s.source_hidden = hidden);
                    }
                    Command::SetPaused(p) => {
                        self.paused = p;
                        self.redraw = true;
                        self.shared.update_status(|s| s.paused = p);
                        info!("stream {}", if p { "paused" } else { "resumed" });
                    }
                    Command::SetLocked(l) => {
                        self.locked = l;
                        self.shared.update_status(|s| s.locked = l);
                        info!("lock {}", if l { "on" } else { "off" });
                    }
                    Command::DisplaysChanged => self.refresh_displays(),
                }
            }
            if let Err(e) = self.tick() {
                if capture::is_device_lost(&e) {
                    warn!("graphics device lost ({}); recreating", hr(&e));
                    self.source = None;
                    self.presenter = None;
                    self.gpus.clear();
                    self.selector.reset();
                    self.shown = Shown::Nothing;
                } else {
                    error!("mirror error: {}", hr(&e));
                }
                self.wait(500);
            }
        }
    }

    fn refresh_displays(&mut self) {
        self.monitors = monitors::enumerate(&self.cfg.stream_display);
        let stream = self.monitors.iter().find(|m| m.is_stream).cloned();
        let physical: Vec<_> = self.monitors.iter().filter(|m| !m.is_stream).map(|m| m.rect).collect();

        // With the mouse guard on, a stream display between two monitors would trap the mouse.
        if let Some(s) = &stream
            && self.cfg.keep_mouse_off_stream_display
            && layout::stream_blocks_path(&physical)
            && self.last_layout_fix.is_none_or(|t| t.elapsed() > Duration::from_secs(10))
            && let Some((x, y)) = layout::edge_position(&physical)
        {
            self.last_layout_fix = Some(Instant::now());
            match layout::move_display(&s.gdi_name, x, y) {
                Ok(()) => {
                    info!(
                        "moved {} to the right edge ({x},{y}) so it no longer sits between monitors",
                        s.label()
                    );
                    self.shared.update_status(|st| st.layout_fixes += 1);
                    // Re-read the new layout on the next loop pass.
                    DISPLAY_CHANGED.with(|c| c.set(true));
                    return;
                }
                Err(e) => warn!("could not move the stream display out of the way: {e}"),
            }
        }

        if stream != self.stream || self.shown == Shown::Nothing {
            let names: Vec<String> = self.monitors.iter().map(Monitor::label).collect();
            info!("monitors: {}", names.join("; "));
            match &stream {
                Some(s) => info!("stream display: {} at {:?}", s.label(), s.rect),
                None => info!("no stream display found"),
            }
        }
        // SAFETY: the window belongs to this thread.
        unsafe {
            match &stream {
                Some(s) => {
                    let _ = SetWindowPos(
                        self.hwnd,
                        Some(HWND_TOPMOST),
                        s.rect.left,
                        s.rect.top,
                        s.rect.width(),
                        s.rect.height(),
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
                None => {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                    self.presenter = None;
                }
            }
        }
        if let (Some(p), Some(s)) = (self.presenter.as_mut(), &stream)
            && let Err(e) = p.resize(s.rect.width() as u32, s.rect.height() as u32)
        {
            warn!("resize failed ({}); recreating swap chain", hr(&e));
            self.presenter = None;
        }
        // Drop the source if its monitor moved, changed size or went away.
        if let Some(src) = &self.source
            && !self
                .monitors
                .iter()
                .any(|m| !m.is_stream && m.gdi_name == src.monitor.gdi_name && m.rect == src.monitor.rect)
        {
            self.source = None;
            self.selector.reset();
        }
        guard::configure(
            self.cfg.keep_mouse_off_stream_display && stream.is_some() && !physical.is_empty(),
            stream.as_ref().map(|s| s.rect),
            physical,
        );
        let name = stream.as_ref().map(|s| s.label());
        self.shared.update_status(|s| s.stream_display = name);
        self.stream = stream;
        self.redraw = true;
    }

    fn tick(&mut self) -> Result<()> {
        let Some(stream) = self.stream.clone() else {
            self.source = None;
            self.fine_timer.set(false);
            self.wait(1000);
            return Ok(());
        };
        if self.paused {
            self.source = None;
            self.selector.reset();
            return self.show_card(Card::Paused, &stream, 250);
        }

        let mut pt = POINT::default();
        // SAFETY: plain out-param. Fails on the secure desktop; then the selection stays put.
        let under = if unsafe { GetCursorPos(&mut pt) }.is_ok() {
            if self.cursor_pos != Some((pt.x, pt.y)) {
                self.cursor_pos = Some((pt.x, pt.y));
                self.cursor_moved_at = Instant::now();
                if self.cfg.show_cursor {
                    self.frame_pending = true;
                }
            }
            self.monitors
                .iter()
                .find(|m| !m.is_stream && m.rect.contains(pt.x, pt.y))
                .map(|m| m.gdi_name.clone())
        } else {
            None
        };
        let delay = Duration::from_millis(self.cfg.switch_delay_ms);
        let target =
            self.selector.update(under.as_deref(), Instant::now(), delay, self.locked).map(str::to_owned);
        if target.as_deref() != self.source.as_ref().map(|s| s.monitor.gdi_name.as_str()) {
            self.switch_to(target)?;
        }

        let Some(src) = self.source.as_mut() else {
            return self.show_card(Card::NoSource, &stream, 100);
        };
        if self.cfg.is_hidden(&src.monitor.id) {
            // Release the duplication: a hidden screen is never captured at all.
            src.capture = None;
            return self.show_card(Card::Hidden, &stream, 50);
        }
        let gpu = src.gpu.clone();
        self.ensure_presenter(&gpu, &stream)?;
        if !self.ensure_capture()? {
            self.fine_timer.set(false);
            self.wait(50);
            return Ok(());
        }

        let (pw, ph) =
            self.presenter.as_ref().map(|p| (p.width as f32, p.height as f32)).unwrap_or((1.0, 1.0));
        // Frames are capped at max_fps; a change that arrives early is drawn at the next slot.
        let frame_interval = Duration::from_micros(1_000_000 / u64::from(self.cfg.max_fps.clamp(10, 240)));
        let moving = self.cursor_moved_at.elapsed() < Duration::from_millis(250);
        let busy = moving || self.frame_pending || self.selector.is_pending();
        self.fine_timer.set(busy);
        let mut timeout: u32 = if busy { (frame_interval.as_millis() as u32).clamp(1, 16) } else { 50 };
        let has_image =
            self.source.as_ref().and_then(|s| s.capture.as_ref()).is_some_and(|c| c.image().is_some());
        if self.frame_pending && has_image {
            // Wake up for the next frame slot (never 0 ms, so this can't spin).
            let wait = frame_interval.saturating_sub(self.last_present.elapsed());
            timeout = timeout.min((wait.as_micros().div_ceil(1000) as u32).max(1));
        }
        // Allow 1 ms of slack so timer rounding doesn't skip a whole frame slot.
        let frame_due = frame_interval.saturating_sub(Duration::from_millis(1));
        let force = self.redraw || self.shown != Shown::Mirror;
        let show_cursor = self.cfg.show_cursor;
        let src = self.source.as_mut().expect("source checked above");
        let (dw, dh) = (src.monitor.rect.width().max(1) as f32, src.monitor.rect.height().max(1) as f32);
        let want_mips = (pw / dw).min(ph / dh) < 0.75;
        let capture = src.capture.as_mut().expect("capture ensured above");
        let outcome = capture.next_frame(&gpu, timeout, want_mips);
        let has_image = capture.image().is_some();
        match outcome {
            Ok(update) => {
                if update.image || (update.pointer && show_cursor) || force {
                    self.frame_pending = true;
                }
                if self.frame_pending && has_image && self.last_present.elapsed() >= frame_due {
                    self.render_mirror()?;
                }
            }
            Err(CaptureError::AccessLost) => {
                src.capture = None;
                src.retry_at = Instant::now() + Duration::from_millis(50);
            }
            Err(CaptureError::Other(e)) => {
                if capture::is_device_lost(&e) {
                    return Err(e);
                }
                warn!("capture of {} failed: {}", src.monitor.label(), hr(&e));
                src.capture = None;
                src.retry_at = Instant::now() + Duration::from_millis(500);
            }
        }
        Ok(())
    }

    fn switch_to(&mut self, target: Option<String>) -> Result<()> {
        self.source = None;
        let monitor = target.and_then(|name| self.monitors.iter().find(|m| m.gdi_name == name).cloned());
        if let Some(monitor) = monitor {
            let picked_at = Instant::now();
            match find_output(&monitor.gdi_name)? {
                Some((adapter, output, _)) => {
                    let gpu = self.gpu_for(&adapter)?;
                    info!(
                        "now streaming {} (gpu {:x}, lookup {} ms)",
                        monitor.label(),
                        gpu.luid,
                        picked_at.elapsed().as_millis()
                    );
                    self.source = Some(Source {
                        monitor,
                        gpu,
                        output,
                        capture: None,
                        retry_at: Instant::now(),
                        failures: 0,
                        picked_at,
                        first_frame_logged: false,
                    });
                }
                None => {
                    warn!("no graphics output found for {}; re-reading displays", monitor.label());
                    self.selector.reset();
                    DISPLAY_CHANGED.with(|c| c.set(true));
                }
            }
        }
        let (label, hidden) = match &self.source {
            Some(s) => (Some(s.monitor.label()), self.cfg.is_hidden(&s.monitor.id)),
            None => (None, false),
        };
        self.shared.update_status(|s| {
            s.source = label;
            s.source_hidden = hidden;
        });
        self.redraw = true;
        Ok(())
    }

    fn gpu_for(&mut self, adapter: &IDXGIAdapter1) -> Result<Rc<Gpu>> {
        let luid = adapter_luid(adapter)?;
        if let Some(g) = self.gpus.iter().find(|g| g.luid == luid) {
            return Ok(g.clone());
        }
        let gpu = Rc::new(Gpu::new(adapter)?);
        self.gpus.push(gpu.clone());
        Ok(gpu)
    }

    /// Any GPU, for drawing message cards when no monitor is being captured.
    fn card_gpu(&mut self, stream: &Monitor) -> Result<Rc<Gpu>> {
        if let Some(p) = &self.presenter {
            return Ok(p.gpu.clone());
        }
        if let Some((adapter, _, _)) = find_output(&stream.gdi_name)? {
            return self.gpu_for(&adapter);
        }
        // SAFETY: plain DXGI enumeration.
        let adapter = unsafe { CreateDXGIFactory1::<IDXGIFactory1>()?.EnumAdapters1(0)? };
        self.gpu_for(&adapter)
    }

    fn ensure_presenter(&mut self, gpu: &Rc<Gpu>, stream: &Monitor) -> Result<()> {
        let (w, h) = (stream.rect.width() as u32, stream.rect.height() as u32);
        if self.presenter.as_ref().is_some_and(|p| p.gpu.luid != gpu.luid) {
            let old = self.presenter.take().expect("presenter checked above");
            let old_gpu = old.gpu.clone();
            drop(old);
            // SAFETY: finish deferred destruction of the old swap chain itself before its
            // replacement is associated with the same window on the new adapter.
            unsafe { old_gpu.ctx.Flush() };
        }
        match self.presenter.as_mut() {
            Some(p) => p.resize(w, h)?,
            None => {
                self.presenter = Some(Presenter::new(gpu.clone(), self.hwnd, w, h)?);
                self.shown = Shown::Nothing;
            }
        }
        Ok(())
    }

    /// Returns whether a capture is ready.
    fn ensure_capture(&mut self) -> Result<bool> {
        let Some(src) = self.source.as_mut() else {
            return Ok(false);
        };
        if src.capture.is_some() {
            return Ok(true);
        }
        if Instant::now() < src.retry_at {
            return Ok(false);
        }
        let started = Instant::now();
        match Capture::new(&src.gpu, &src.output) {
            Ok(c) => {
                info!("capture of {} ready in {} ms", src.monitor.label(), started.elapsed().as_millis());
                if src.failures > 0 {
                    info!("capture of {} works again", src.monitor.label());
                }
                src.capture = Some(c);
                src.failures = 0;
                self.redraw = true;
                self.shared.update_status(|s| s.problem = None);
                Ok(true)
            }
            Err(e) if capture::is_device_lost(&e) => Err(e),
            Err(e) => {
                src.failures += 1;
                src.retry_at = Instant::now() + Duration::from_millis(u64::from(src.failures.min(20)) * 100);
                if src.failures == 1 || src.failures % 50 == 0 {
                    // E_ACCESSDENIED is normal while the lock screen or a UAC prompt is up.
                    warn!("can't capture {} yet: {}", src.monitor.label(), hr(&e));
                }
                let problem = format!("Can't capture {} right now", src.monitor.name);
                self.shared.update_status(|s| s.problem = Some(problem));
                Ok(false)
            }
        }
    }

    fn render_mirror(&mut self) -> Result<()> {
        let (Some(presenter), Some(src)) = (self.presenter.as_mut(), self.source.as_mut()) else {
            return Ok(());
        };
        let Some(capture) = src.capture.as_ref() else {
            return Ok(());
        };
        let Some(image) = capture.image() else {
            return Ok(());
        };
        presenter.begin()?;
        let gpu = &presenter.gpu;
        let (pw, ph) = (presenter.width, presenter.height);
        let (dw, dh) = (src.monitor.rect.width().max(1) as u32, src.monitor.rect.height().max(1) as u32);
        let fit = geometry::fit(dw, dh, pw, ph);
        gpu.draw(image, geometry::to_ndc(fit, pw, ph), capture.rotation, Blend::Opaque);

        let pointer = &capture.pointer;
        if self.cfg.show_cursor
            && pointer.visible
            && let Some(shape) = &pointer.shape
        {
            let scale = fit.w / dw as f32;
            // Prefer the live cursor position; fall back to the one Desktop Duplication reported.
            let rect = src.monitor.rect;
            let (px, py) = match self.cursor_pos {
                Some((x, y)) if rect.contains(x, y) => {
                    (x - rect.left - shape.hotspot.0, y - rect.top - shape.hotspot.1)
                }
                _ => (pointer.x, pointer.y),
            };
            let r = RectF {
                x: fit.x + px as f32 * scale,
                y: fit.y + py as f32 * scale,
                w: (shape.width as f32 * scale).max(1.0),
                h: (shape.height as f32 * scale).max(1.0),
            };
            let ndc = geometry::to_ndc(r, pw, ph);
            gpu.draw(&shape.color, ndc, Rotation::Identity, Blend::Alpha);
            if let Some(invert) = &shape.invert {
                gpu.draw(invert, ndc, Rotation::Identity, Blend::Invert);
            }
        }
        presenter.present()?;
        if !src.first_frame_logged {
            src.first_frame_logged = true;
            info!(
                "first frame of {} on stream after {} ms",
                src.monitor.label(),
                src.picked_at.elapsed().as_millis()
            );
        }
        self.shown = Shown::Mirror;
        self.redraw = false;
        self.frame_pending = false;
        self.last_present = Instant::now();
        self.frames += 1;
        let elapsed = self.stats_since.elapsed();
        if elapsed >= Duration::from_secs(60) {
            info!(
                "last {} s: {} frames ({:.1} fps average)",
                elapsed.as_secs(),
                self.frames,
                f64::from(self.frames) / elapsed.as_secs_f64()
            );
            self.frames = 0;
            self.stats_since = Instant::now();
        }
        Ok(())
    }

    fn show_card(&mut self, card: Card, stream: &Monitor, wait_ms: u32) -> Result<()> {
        self.fine_timer.set(false);
        if self.shown != Shown::Card(card) || self.redraw {
            let gpu = self.card_gpu(stream)?;
            self.ensure_presenter(&gpu, stream)?;
            let (title, subtitle) = match card {
                Card::Paused => (
                    "Stream paused",
                    if self.cfg.hotkey_pause.is_empty() {
                        "Resume from the tray icon".to_owned()
                    } else {
                        format!("Press {} to resume", self.cfg.hotkey_pause)
                    },
                ),
                Card::Hidden => ("This screen is private", "Move the mouse to another screen".to_owned()),
                Card::NoSource => ("Move the mouse onto a screen", String::new()),
            };
            self.presenter.as_mut().expect("presenter ensured above").draw_card(title, &subtitle)?;
            self.shown = Shown::Card(card);
            self.redraw = false;
        }
        self.wait(wait_ms);
        Ok(())
    }

    /// Sleeps until the timeout, a command, or a window message.
    fn wait(&self, ms: u32) {
        // SAFETY: valid event handle owned by Shared.
        unsafe {
            MsgWaitForMultipleObjects(Some(&[self.shared.wake_handle()]), false, ms, QS_ALLINPUT);
        }
    }
}

fn pump_messages() -> bool {
    let mut msg = MSG::default();
    // SAFETY: standard message loop on this thread's queue.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return false;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    true
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_ERASEBKGND => LRESULT(1),
        // The stream window only goes away when the app quits.
        WM_CLOSE => LRESULT(0),
        WM_DISPLAYCHANGE => {
            DISPLAY_CHANGED.with(|c| c.set(true));
            LRESULT(0)
        }
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn create_window() -> Result<HWND> {
    // SAFETY: registers a class (once per process) and creates a window owned by this thread.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        // Fails harmlessly with "class already exists" after a restart.
        RegisterClassExW(&class);
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("Stream Screen on Cursor"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}
