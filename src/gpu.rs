//! One Direct3D 11 device per graphics adapter, with the tiny pipeline used to draw the
//! mirrored desktop and the cursor: a single textured quad, no vertex buffers.

use crate::geometry::Rotation;
use std::sync::OnceLock;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct2D::{
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_FACTORY_OPTIONS, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1CreateFactory, ID2D1DeviceContext, ID2D1Factory1,
};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
    D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP, ID3DBlob,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory,
};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter1, IDXGIDevice, IDXGIDevice1};
use windows::core::{Interface, PCSTR, Result, s};

const SHADER: &str = r#"
cbuffer Params : register(b0) {
    float4 dest; // left, top, right, bottom in normalized device coordinates
    float4 uvx;  // tex.x = dot(uvx.xyz, float3(u, v, 1))
    float4 uvy;  // tex.y = dot(uvy.xyz, float3(u, v, 1))
};
struct VSOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
VSOut vs_main(uint id : SV_VertexID) {
    float2 t = float2(id & 1, id >> 1);
    VSOut o;
    o.pos = float4(lerp(dest.x, dest.z, t.x), lerp(dest.y, dest.w, t.y), 0, 1);
    o.uv = float2(dot(uvx.xyz, float3(t, 1)), dot(uvy.xyz, float3(t, 1)));
    return o;
}
Texture2D tex : register(t0);
SamplerState smp : register(s0);
float4 ps_main(VSOut i) : SV_Target { return tex.Sample(smp, i.uv); }
"#;

#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    dest: [f32; 4],
    uvx: [f32; 4],
    uvy: [f32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    /// Copy the texture (desktop image).
    Opaque,
    /// Straight alpha blending (cursor color pixels).
    Alpha,
    /// `dest = src * (1 - dest) + dest * (1 - src)`: white source pixels invert the screen.
    Invert,
}

fn compile(entry: PCSTR, target: PCSTR) -> Result<Vec<u8>> {
    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the source pointer and length describe SHADER; out-params are valid.
    let r = unsafe {
        D3DCompile(
            SHADER.as_ptr() as *const _,
            SHADER.len(),
            s!("mirror.hlsl"),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut blob,
            Some(&mut errors),
        )
    };
    if let Err(e) = r {
        if let Some(errors) = errors {
            // SAFETY: the error blob holds a text message of the reported size.
            let msg = unsafe {
                std::slice::from_raw_parts(errors.GetBufferPointer() as *const u8, errors.GetBufferSize())
            };
            crate::error!("shader compile failed: {}", String::from_utf8_lossy(msg));
        }
        return Err(e);
    }
    let blob = blob.expect("D3DCompile succeeded without output");
    // SAFETY: the blob owns GetBufferSize() bytes at GetBufferPointer().
    Ok(unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) }
        .to_vec())
}

/// Compiled once per process and shared by every device.
fn shaders() -> Result<&'static (Vec<u8>, Vec<u8>)> {
    static CACHE: OnceLock<(Vec<u8>, Vec<u8>)> = OnceLock::new();
    if let Some(c) = CACHE.get() {
        return Ok(c);
    }
    let vs = compile(s!("vs_main"), s!("vs_4_0"))?;
    let ps = compile(s!("ps_main"), s!("ps_4_0"))?;
    Ok(CACHE.get_or_init(|| (vs, ps)))
}

pub fn adapter_luid(adapter: &IDXGIAdapter1) -> Result<i64> {
    // SAFETY: simple COM getter.
    let desc = unsafe { adapter.GetDesc1()? };
    Ok((i64::from(desc.AdapterLuid.HighPart) << 32) | i64::from(desc.AdapterLuid.LowPart))
}

pub struct Gpu {
    pub luid: i64,
    pub device: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
    pub d2d: ID2D1DeviceContext,
    pub dwrite: IDWriteFactory,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    linear: ID3D11SamplerState,
    alpha: ID3D11BlendState,
    invert: ID3D11BlendState,
    raster: ID3D11RasterizerState,
    params: ID3D11Buffer,
}

impl Gpu {
    pub fn new(adapter: &IDXGIAdapter1) -> Result<Self> {
        let luid = adapter_luid(adapter)?;
        let (vs_code, ps_code) = shaders()?;
        let levels = [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0];
        let mut device = None;
        let mut ctx = None;
        // SAFETY: all pointers are valid for the call; out-params are Options.
        unsafe {
            D3D11CreateDevice(
                adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )?;
        }
        let device: ID3D11Device = device.expect("D3D11CreateDevice returned no device");
        let ctx = ctx.expect("D3D11CreateDevice returned no context");

        // SAFETY: COM calls with valid descriptors; created objects are owned by Gpu.
        unsafe {
            // Keep the GPU queue short: lower latency and no frames piling up.
            device.cast::<IDXGIDevice1>()?.SetMaximumFrameLatency(1)?;

            let mut vs = None;
            device.CreateVertexShader(vs_code, None, Some(&mut vs))?;
            let mut ps = None;
            device.CreatePixelShader(ps_code, None, Some(&mut ps))?;

            let sampler = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                ..Default::default()
            };
            let mut linear = None;
            device.CreateSamplerState(&sampler, Some(&mut linear))?;

            let mut blend = D3D11_BLEND_DESC::default();
            blend.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_SRC_ALPHA,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_ZERO,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut alpha = None;
            device.CreateBlendState(&blend, Some(&mut alpha))?;
            blend.RenderTarget[0].SrcBlend = D3D11_BLEND_INV_DEST_COLOR;
            blend.RenderTarget[0].DestBlend = D3D11_BLEND_INV_SRC_COLOR;
            let mut invert = None;
            device.CreateBlendState(&blend, Some(&mut invert))?;

            let raster_desc = D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                DepthClipEnable: true.into(),
                ..Default::default()
            };
            let mut raster = None;
            device.CreateRasterizerState(&raster_desc, Some(&mut raster))?;

            let cb_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<Params>() as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            let mut params = None;
            device.CreateBuffer(&cb_desc, None, Some(&mut params))?;

            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&D2D1_FACTORY_OPTIONS::default()))?;
            let d2d_device = d2d_factory.CreateDevice(&device.cast::<IDXGIDevice>()?)?;
            let d2d = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;

            Ok(Self {
                luid,
                device,
                ctx,
                d2d,
                dwrite,
                vs: vs.unwrap(),
                ps: ps.unwrap(),
                linear: linear.unwrap(),
                alpha: alpha.unwrap(),
                invert: invert.unwrap(),
                raster: raster.unwrap(),
                params: params.unwrap(),
            })
        }
    }

    /// Creates a shader-readable texture, optionally with a full mip chain for smooth downscaling.
    pub fn create_texture(
        &self,
        width: u32,
        height: u32,
        mips: bool,
        data: Option<&[u8]>,
    ) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: if mips { 0 } else { 1 },
            ArraySize: 1,
            Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: if mips {
                (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32
            } else {
                D3D11_BIND_SHADER_RESOURCE.0 as u32
            },
            CPUAccessFlags: 0,
            MiscFlags: if mips { D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32 } else { 0 },
        };
        let init = data.map(|d| D3D11_SUBRESOURCE_DATA {
            pSysMem: d.as_ptr() as *const _,
            SysMemPitch: width * 4,
            SysMemSlicePitch: 0,
        });
        let mut tex = None;
        let mut srv = None;
        // SAFETY: descriptors are valid; initial data (if any) holds width*height*4 bytes.
        unsafe {
            self.device.CreateTexture2D(&desc, init.as_ref().map(|i| i as *const _), Some(&mut tex))?;
            let tex: ID3D11Texture2D = tex.unwrap();
            self.device.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
            Ok((tex, srv.unwrap()))
        }
    }

    /// Draws `srv` into the bound render target over `dest_ndc`, sampling it through `rotation`.
    pub fn draw(&self, srv: &ID3D11ShaderResourceView, dest_ndc: [f32; 4], rotation: Rotation, blend: Blend) {
        let (a, b) = rotation.uv_transform();
        let params = Params { dest: dest_ndc, uvx: [a[0], a[1], a[2], 0.0], uvy: [b[0], b[1], b[2], 0.0] };
        let blend_state = match blend {
            Blend::Opaque => None,
            Blend::Alpha => Some(&self.alpha),
            Blend::Invert => Some(&self.invert),
        };
        // SAFETY: all bound objects belong to this device and outlive the draw call.
        unsafe {
            self.ctx.UpdateSubresource(&self.params, 0, None, &params as *const Params as *const _, 0, 0);
            self.ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            self.ctx.IASetInputLayout(None);
            self.ctx.VSSetShader(&self.vs, None);
            self.ctx.VSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
            self.ctx.PSSetShader(&self.ps, None);
            self.ctx.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
            self.ctx.PSSetSamplers(0, Some(&[Some(self.linear.clone())]));
            self.ctx.RSSetState(&self.raster);
            self.ctx.OMSetBlendState(blend_state, None, 0xffff_ffff);
            self.ctx.Draw(4, 0);
            self.ctx.PSSetShaderResources(0, Some(&[None]));
        }
    }
}
