//! Desktop Duplication of one monitor. Frames stay on the GPU: only the regions Windows reports
//! as changed are copied, and nothing happens at all while the screen is still.

use crate::cursor_shape::{self, CursorImage};
use crate::geometry::Rotation;
use crate::gpu::{Gpu, adapter_luid};
use crate::monitors::wide_to_string;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{D3D11_BOX, ID3D11ShaderResourceView, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_MOVE_RECT,
    DXGI_OUTDUPL_POINTER_SHAPE_INFO, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1, IDXGIOutput5,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::{Error, HRESULT, Interface, Result};

/// Finds the DXGI adapter and output that drive the monitor with this GDI name.
pub fn find_output(gdi_name: &str) -> Result<Option<(IDXGIAdapter1, IDXGIOutput, i64)>> {
    // SAFETY: plain DXGI enumeration; errors end the loops.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let mut j = 0;
            while let Ok(output) = adapter.EnumOutputs(j) {
                j += 1;
                let desc = output.GetDesc()?;
                if wide_to_string(&desc.DeviceName).eq_ignore_ascii_case(gdi_name) {
                    let luid = adapter_luid(&adapter)?;
                    return Ok(Some((adapter, output, luid)));
                }
            }
        }
    }
    Ok(None)
}

pub fn is_device_lost(e: &Error) -> bool {
    let c = e.code();
    c == DXGI_ERROR_DEVICE_REMOVED || c == DXGI_ERROR_DEVICE_RESET
}

pub struct CursorTextures {
    pub width: u32,
    pub height: u32,
    /// Hot spot (the pixel that points), relative to the image's top-left.
    pub hotspot: (i32, i32),
    pub color: ID3D11ShaderResourceView,
    pub invert: Option<ID3D11ShaderResourceView>,
}

#[derive(Default)]
pub struct Pointer {
    pub visible: bool,
    /// Top-left of the cursor image, relative to the monitor's top-left, in desktop pixels.
    pub x: i32,
    pub y: i32,
    pub shape: Option<CursorTextures>,
}

#[derive(Default, Clone, Copy)]
pub struct FrameUpdate {
    pub image: bool,
    pub pointer: bool,
}

pub enum CaptureError {
    /// The duplication must be recreated (mode change, secure desktop, fullscreen switch...).
    AccessLost,
    Other(Error),
}

impl From<Error> for CaptureError {
    fn from(e: Error) -> Self {
        if e.code() == DXGI_ERROR_ACCESS_LOST { Self::AccessLost } else { Self::Other(e) }
    }
}

struct Image {
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    width: u32,
    height: u32,
    mips: bool,
}

pub struct Capture {
    dup: IDXGIOutputDuplication,
    pub rotation: Rotation,
    image: Option<Image>,
    full_copy: bool,
    moves: Vec<DXGI_OUTDUPL_MOVE_RECT>,
    dirty: Vec<RECT>,
    shape_buf: Vec<u8>,
    pub pointer: Pointer,
}

impl Capture {
    pub fn new(gpu: &Gpu, output: &IDXGIOutput) -> Result<Self> {
        // SAFETY: COM calls on valid interfaces.
        let dup = unsafe {
            match output.cast::<IDXGIOutput5>() {
                // Asking for 8-bit BGRA makes Windows tone-map HDR monitors for us.
                Ok(o5) => o5.DuplicateOutput1(&gpu.device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM])?,
                Err(_) => output.cast::<IDXGIOutput1>()?.DuplicateOutput(&gpu.device)?,
            }
        };
        // SAFETY: simple getter.
        let desc = unsafe { dup.GetDesc() };
        Ok(Self {
            dup,
            rotation: Rotation::from_dxgi(desc.Rotation.0),
            image: None,
            full_copy: true,
            moves: Vec::new(),
            dirty: Vec::new(),
            shape_buf: Vec::new(),
            pointer: Pointer::default(),
        })
    }

    /// The latest desktop image, once the first frame has arrived.
    pub fn image(&self) -> Option<&ID3D11ShaderResourceView> {
        self.image.as_ref().map(|i| &i.srv)
    }

    /// Waits up to `timeout_ms` for the screen or the mouse to change.
    pub fn next_frame(
        &mut self,
        gpu: &Gpu,
        timeout_ms: u32,
        want_mips: bool,
    ) -> std::result::Result<FrameUpdate, CaptureError> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // SAFETY: valid out-params; every acquired frame is released below.
        match unsafe { self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut resource) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(FrameUpdate::default()),
            Err(e) => return Err(e.into()),
        }
        let result = self.process(gpu, &info, resource, want_mips);
        // SAFETY: a frame is held at this point.
        let released = unsafe { self.dup.ReleaseFrame() };
        let update = result?;
        released?;
        Ok(update)
    }

    fn process(
        &mut self,
        gpu: &Gpu,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: Option<IDXGIResource>,
        want_mips: bool,
    ) -> Result<FrameUpdate> {
        let mut update = FrameUpdate::default();
        // A newly selected monitor still needs its initial desktop image when the acquired
        // frame only reports a pointer update. Otherwise a quiet screen can leave the previous
        // monitor on the stream until a window repaints.
        if (info.LastPresentTime != 0 || self.image.is_none())
            && let Some(resource) = resource
        {
            let frame: ID3D11Texture2D = resource.cast()?;
            self.copy_frame(gpu, &frame, info.TotalMetadataBufferSize, want_mips)?;
            update.image = true;
        }
        if info.LastMouseUpdateTime != 0 {
            self.pointer.visible = info.PointerPosition.Visible.as_bool();
            if self.pointer.visible {
                self.pointer.x = info.PointerPosition.Position.x;
                self.pointer.y = info.PointerPosition.Position.y;
            }
            update.pointer = true;
        }
        if info.PointerShapeBufferSize > 0 {
            self.read_shape(gpu, info.PointerShapeBufferSize)?;
            update.pointer = true;
        }
        Ok(update)
    }

    fn copy_frame(
        &mut self,
        gpu: &Gpu,
        frame: &ID3D11Texture2D,
        metadata_size: u32,
        want_mips: bool,
    ) -> Result<()> {
        let mut desc = Default::default();
        // SAFETY: simple getter.
        unsafe { frame.GetDesc(&mut desc) };
        let fits = self
            .image
            .as_ref()
            .is_some_and(|i| i.width == desc.Width && i.height == desc.Height && i.mips == want_mips);
        if !fits {
            let (tex, srv) = gpu.create_texture(desc.Width, desc.Height, want_mips, None)?;
            self.image = Some(Image { tex, srv, width: desc.Width, height: desc.Height, mips: want_mips });
            self.full_copy = true;
        }
        let rects = if self.full_copy { None } else { self.changed_rects(metadata_size) };
        let image = self.image.as_ref().expect("image was just ensured");
        // SAFETY: both textures live on gpu.device with the same size and format.
        unsafe {
            match rects {
                Some(rects) => {
                    for r in rects {
                        let b = D3D11_BOX {
                            left: r.left.max(0) as u32,
                            top: r.top.max(0) as u32,
                            front: 0,
                            right: (r.right.max(0) as u32).min(desc.Width),
                            bottom: (r.bottom.max(0) as u32).min(desc.Height),
                            back: 1,
                        };
                        if b.right > b.left && b.bottom > b.top {
                            gpu.ctx.CopySubresourceRegion(
                                &image.tex,
                                0,
                                b.left,
                                b.top,
                                0,
                                frame,
                                0,
                                Some(&b),
                            );
                        }
                    }
                }
                None => {
                    if image.mips {
                        gpu.ctx.CopySubresourceRegion(&image.tex, 0, 0, 0, 0, frame, 0, None);
                    } else {
                        gpu.ctx.CopyResource(&image.tex, frame);
                    }
                }
            }
            if image.mips {
                gpu.ctx.GenerateMips(&image.srv);
            }
        }
        self.full_copy = false;
        Ok(())
    }

    /// Rectangles that changed this frame (dirty regions and move destinations), or `None` when a
    /// full copy is simpler or the metadata can't be read.
    fn changed_rects(&mut self, metadata_size: u32) -> Option<Vec<RECT>> {
        if metadata_size == 0 {
            return None;
        }
        let cap = metadata_size as usize;
        self.moves.resize(cap / std::mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>() + 1, Default::default());
        self.dirty.resize(cap / std::mem::size_of::<RECT>() + 1, Default::default());
        let mut move_bytes = 0u32;
        let mut dirty_bytes = 0u32;
        // SAFETY: buffer sizes passed match the vectors' capacities in bytes.
        unsafe {
            self.dup
                .GetFrameMoveRects(
                    (self.moves.len() * std::mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>()) as u32,
                    self.moves.as_mut_ptr(),
                    &mut move_bytes,
                )
                .ok()?;
            self.dup
                .GetFrameDirtyRects(
                    (self.dirty.len() * std::mem::size_of::<RECT>()) as u32,
                    self.dirty.as_mut_ptr(),
                    &mut dirty_bytes,
                )
                .ok()?;
        }
        let moves = move_bytes as usize / std::mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>();
        let dirty = dirty_bytes as usize / std::mem::size_of::<RECT>();
        if moves + dirty > 64 {
            return None;
        }
        let mut out: Vec<RECT> = self.moves[..moves].iter().map(|m| m.DestinationRect).collect();
        out.extend_from_slice(&self.dirty[..dirty]);
        Some(out)
    }

    fn read_shape(&mut self, gpu: &Gpu, size: u32) -> Result<()> {
        self.shape_buf.resize(size as usize, 0);
        let mut required = 0u32;
        let mut info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        // SAFETY: buffer holds `size` bytes as reported by AcquireNextFrame.
        unsafe {
            self.dup.GetFramePointerShape(
                size,
                self.shape_buf.as_mut_ptr() as *mut _,
                &mut required,
                &mut info,
            )?;
        }
        self.pointer.shape =
            match cursor_shape::convert(info.Type, info.Width, info.Height, info.Pitch, &self.shape_buf) {
                Some(img) => Some(upload_cursor(gpu, &img, (info.HotSpot.x, info.HotSpot.y))?),
                None => None,
            };
        Ok(())
    }
}

fn upload_cursor(gpu: &Gpu, img: &CursorImage, hotspot: (i32, i32)) -> Result<CursorTextures> {
    let (_, color) = gpu.create_texture(img.width, img.height, false, Some(&img.color))?;
    let invert = match &img.invert {
        Some(mask) => Some(gpu.create_texture(img.width, img.height, false, Some(mask))?.1),
        None => None,
    };
    Ok(CursorTextures { width: img.width, height: img.height, hotspot, color, invert })
}

/// HRESULT helper for logs.
pub fn hr(e: &Error) -> String {
    let HRESULT(code) = e.code();
    format!("0x{:08X} {}", code as u32, e.message())
}
