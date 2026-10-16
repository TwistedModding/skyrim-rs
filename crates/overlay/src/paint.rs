//! Paints tessellated `egui` meshes into the back buffer on the game's immediate context. The frame
//! starts with `ClearState` (frame pass), so the state set here is not restored.

use core::ffi::c_void;
use std::collections::HashMap;

use egui::epaint::{ImageDelta, Primitive, Vertex, textures::TexturesDelta};
use egui::{ClippedPrimitive, ImageData, TextureId};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_FLAG, D3D11_BIND_INDEX_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VERTEX_BUFFER, D3D11_BLEND_DESC, D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE, D3D11_BLEND_OP_ADD,
    D3D11_BOX, D3D11_BUFFER_DESC, D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_COMPARISON_ALWAYS, D3D11_CPU_ACCESS_WRITE, D3D11_CULL_NONE, D3D11_DEPTH_STENCIL_DESC, D3D11_FILL_SOLID, D3D11_FILTER_MIN_MAG_MIP_LINEAR,
    D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA, D3D11_MAP_WRITE_DISCARD, D3D11_MAPPED_SUBRESOURCE, D3D11_RASTERIZER_DESC, D3D11_RENDER_TARGET_BLEND_DESC, D3D11_SAMPLER_DESC, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_DYNAMIC, D3D11_VIEWPORT, ID3D11BlendState, ID3D11Buffer, ID3D11DepthStencilState, ID3D11Device, ID3D11DeviceContext,
    ID3D11InputLayout, ID3D11PixelShader, ID3D11RasterizerState, ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R32_UINT, DXGI_FORMAT_R32G32_FLOAT, DXGI_SAMPLE_DESC};
use windows::core::{Interface, PCSTR, Result, s};

const SHADER: &str = r"
cbuffer Screen : register(b0) { float2 screen; };
struct VsIn { float2 pos : POSITION; float2 uv : TEXCOORD; float4 color : COLOR; };
struct PsIn { float4 pos : SV_POSITION; float2 uv : TEXCOORD; float4 color : COLOR; };
PsIn vs(VsIn v) {
    PsIn o;
    o.pos = float4(2.0 * v.pos.x / screen.x - 1.0, 1.0 - 2.0 * v.pos.y / screen.y, 0.0, 1.0);
    o.uv = v.uv;
    o.color = v.color;
    return o;
}
Texture2D tex : register(t0);
SamplerState smp : register(s0);
float4 ps(PsIn p) : SV_TARGET { return p.color * tex.Sample(smp, p.uv); }
";

struct Texture {
    texture: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
}

/// A dynamic buffer that grows to the largest frame it has held.
struct Dynamic {
    buffer: Option<ID3D11Buffer>,
    capacity: usize,
    bind: D3D11_BIND_FLAG,
}

impl Dynamic {
    const fn new(bind: D3D11_BIND_FLAG) -> Self {
        Self { buffer: None, capacity: 0, bind }
    }

    fn write<T: Copy>(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, data: &[T]) -> Result<&ID3D11Buffer> {
        if self.buffer.is_none() || self.capacity < data.len() {
            self.capacity = data.len().next_power_of_two().max(1024);
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: u32::try_from(self.capacity * size_of::<T>()).unwrap_or(u32::MAX),
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: self.bind.0.cast_unsigned(),
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            };
            self.buffer = None;
            unsafe { device.CreateBuffer(&raw const desc, None, Some(&raw mut self.buffer)) }?;
        }
        let buffer = self.buffer.as_ref().expect("created above");
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            context.Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&raw mut mapped))?;
            core::ptr::copy_nonoverlapping(data.as_ptr(), mapped.pData.cast::<T>(), data.len());
            context.Unmap(buffer, 0);
        }
        Ok(buffer)
    }
}

pub(crate) struct Painter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    layout: ID3D11InputLayout,
    blend: ID3D11BlendState,
    raster: ID3D11RasterizerState,
    depth: ID3D11DepthStencilState,
    sampler: ID3D11SamplerState,
    screen: ID3D11Buffer,
    vertices: Dynamic,
    indices: Dynamic,
    textures: HashMap<TextureId, Texture>,
}

fn compile(entry: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    let mut code = None;
    let mut errors = None;
    let compiled = unsafe { D3DCompile(SHADER.as_ptr().cast(), SHADER.len(), s!("overlay"), None, None, entry, target, 0, 0, &raw mut code, Some(&raw mut errors)) };
    if let Err(e) = compiled {
        let detail = errors.map(|b| unsafe { String::from_utf8_lossy(core::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize())).into_owned() });
        return Err(windows::core::Error::new(e.code(), detail.unwrap_or_default()));
    }
    Ok(code.expect("D3DCompile succeeded"))
}

fn bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe { core::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()) }
}

fn element(name: PCSTR, format: DXGI_FORMAT, offset: u32) -> D3D11_INPUT_ELEMENT_DESC {
    D3D11_INPUT_ELEMENT_DESC { SemanticName: name, SemanticIndex: 0, Format: format, InputSlot: 0, AlignedByteOffset: offset, InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA, InstanceDataStepRate: 0 }
}

/// Fills an out parameter the way every `ID3D11Device::Create*` call does.
fn create<T>(f: impl FnOnce(&mut Option<T>) -> Result<()>) -> Result<T> {
    let mut out = None;
    f(&mut out)?;
    Ok(out.expect("Create* succeeded"))
}

impl Painter {
    /// # Safety
    /// `device` and `context` must be the game's live `ID3D11Device` and immediate context.
    pub(crate) unsafe fn new(device: *mut c_void, context: *mut c_void) -> Result<Self> {
        let none = || windows::core::Error::from(windows::Win32::Foundation::E_POINTER);
        let device = unsafe { ID3D11Device::from_raw_borrowed(&device) }.ok_or_else(none)?.clone();
        let context = unsafe { ID3D11DeviceContext::from_raw_borrowed(&context) }.ok_or_else(none)?.clone();
        let vs = compile(s!("vs"), s!("vs_4_0"))?;
        let ps = compile(s!("ps"), s!("ps_4_0"))?;
        let elements = [element(s!("POSITION"), DXGI_FORMAT_R32G32_FLOAT, 0), element(s!("TEXCOORD"), DXGI_FORMAT_R32G32_FLOAT, 8), element(s!("COLOR"), DXGI_FORMAT_R8G8B8A8_UNORM, 16)];
        let mut blend = D3D11_BLEND_DESC::default();
        blend.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: true.into(),
            SrcBlend: D3D11_BLEND_ONE,
            DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D11_BLEND_OP_ADD,
            SrcBlendAlpha: D3D11_BLEND_ONE,
            DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOpAlpha: D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0.to_le_bytes()[0],
        };
        let raster = D3D11_RASTERIZER_DESC { FillMode: D3D11_FILL_SOLID, CullMode: D3D11_CULL_NONE, ScissorEnable: true.into(), ..Default::default() };
        let sampler = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            ComparisonFunc: D3D11_COMPARISON_ALWAYS,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let screen = D3D11_BUFFER_DESC { ByteWidth: 16, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() };
        unsafe {
            Ok(Self {
                vertex_shader: create(|o| device.CreateVertexShader(bytes(&vs), None, Some(o)))?,
                pixel_shader: create(|o| device.CreatePixelShader(bytes(&ps), None, Some(o)))?,
                layout: create(|o| device.CreateInputLayout(&elements, bytes(&vs), Some(o)))?,
                blend: create(|o| device.CreateBlendState(&raw const blend, Some(o)))?,
                raster: create(|o| device.CreateRasterizerState(&raw const raster, Some(o)))?,
                depth: create(|o| device.CreateDepthStencilState(&D3D11_DEPTH_STENCIL_DESC::default(), Some(o)))?,
                sampler: create(|o| device.CreateSamplerState(&raw const sampler, Some(o)))?,
                screen: create(|o| device.CreateBuffer(&raw const screen, None, Some(o)))?,
                vertices: Dynamic::new(D3D11_BIND_VERTEX_BUFFER),
                indices: Dynamic::new(D3D11_BIND_INDEX_BUFFER),
                textures: HashMap::new(),
                device,
                context,
            })
        }
    }

    /// The size in pixels of the texture behind a render target view.
    ///
    /// # Safety
    /// `rtv` must be a live `ID3D11RenderTargetView`.
    pub(crate) unsafe fn target_size(rtv: *mut c_void) -> Option<[u32; 2]> {
        let rtv = unsafe { ID3D11RenderTargetView::from_raw_borrowed(&rtv) }?;
        let texture = unsafe { rtv.GetResource() }.ok()?.cast::<ID3D11Texture2D>().ok()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&raw mut desc) };
        Some([desc.Width, desc.Height])
    }

    /// Applies the texture changes and draws `primitives` (in points) into `rtv`.
    ///
    /// # Safety
    /// `rtv` must be a live render target view of the device, `size` its pixel size.
    pub(crate) unsafe fn paint(&mut self, rtv: *mut c_void, size: [u32; 2], pixels_per_point: f32, primitives: &[ClippedPrimitive], delta: &TexturesDelta) -> Result<()> {
        for (id, images) in &delta.set {
            for image in images {
                self.set_texture(*id, image)?;
            }
        }
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut draws = Vec::new();
        for ClippedPrimitive { clip_rect, primitive } in primitives {
            let Primitive::Mesh(mesh) = primitive else {
                continue;
            };
            if mesh.indices.is_empty() || !self.textures.contains_key(&mesh.texture_id) {
                continue;
            }
            draws.push((*clip_rect * pixels_per_point, mesh.texture_id, indices.len(), mesh.indices.len(), vertices.len()));
            vertices.extend_from_slice(&mesh.vertices);
            indices.extend_from_slice(&mesh.indices);
        }
        if !draws.is_empty() {
            let rtv = unsafe { ID3D11RenderTargetView::from_raw_borrowed(&rtv) }.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;
            self.draw(rtv, size, pixels_per_point, &vertices, &indices, &draws)?;
        }
        for id in &delta.free {
            self.textures.remove(id);
        }
        Ok(())
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_precision_loss)]
    fn draw(&mut self, rtv: &ID3D11RenderTargetView, size: [u32; 2], pixels_per_point: f32, vertices: &[Vertex], indices: &[u32], draws: &[(egui::Rect, TextureId, usize, usize, usize)]) -> Result<()> {
        let (device, context) = (&self.device, &self.context);
        let vb = self.vertices.write(device, context, vertices)?.clone();
        let ib = self.indices.write(device, context, indices)?.clone();
        let screen = [size[0] as f32 / pixels_per_point, size[1] as f32 / pixels_per_point, 0.0, 0.0];
        let stride = size_of::<Vertex>() as u32;
        unsafe {
            context.UpdateSubresource(&self.screen, 0, None, screen.as_ptr().cast(), 0, 0);
            context.IASetInputLayout(&self.layout);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.IASetVertexBuffers(0, 1, Some(&Some(vb)), Some(&raw const stride), Some(&0));
            context.IASetIndexBuffer(&ib, DXGI_FORMAT_R32_UINT, 0);
            context.VSSetShader(&self.vertex_shader, None);
            context.VSSetConstantBuffers(0, Some(&[Some(self.screen.clone())]));
            context.HSSetShader(None, None);
            context.DSSetShader(None, None);
            context.GSSetShader(None, None);
            context.PSSetShader(&self.pixel_shader, None);
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.RSSetState(&self.raster);
            context.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: size[0] as f32, Height: size[1] as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
            context.OMSetBlendState(&self.blend, Some(&[0.0; 4]), u32::MAX);
            context.OMSetDepthStencilState(&self.depth, 0);
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
        }
        for &(clip, texture, first, count, base) in draws {
            let rect =
                RECT { left: clip.min.x.max(0.0).round() as i32, top: clip.min.y.max(0.0).round() as i32, right: clip.max.x.min(size[0] as f32).round() as i32, bottom: clip.max.y.min(size[1] as f32).round() as i32 };
            if rect.right <= rect.left || rect.bottom <= rect.top {
                continue;
            }
            unsafe {
                context.RSSetScissorRects(Some(&[rect]));
                context.PSSetShaderResources(0, Some(&[Some(self.textures[&texture].view.clone())]));
                context.DrawIndexed(count as u32, first as u32, base as i32);
            }
        }
        Ok(())
    }

    #[allow(clippy::cast_possible_truncation)]
    fn set_texture(&mut self, id: TextureId, delta: &ImageDelta) -> Result<()> {
        let ImageData::Color(image) = &delta.image;
        let [w, h] = image.size.map(|n| n as u32);
        let pixels = image.pixels.as_ptr().cast::<c_void>();
        if let Some([x, y]) = delta.pos {
            let Some(existing) = self.textures.get(&id) else {
                return Ok(());
            };
            let area = D3D11_BOX { left: x as u32, top: y as u32, front: 0, right: x as u32 + w, bottom: y as u32 + h, back: 1 };
            unsafe { self.context.UpdateSubresource(&existing.texture, 0, Some(&raw const area), pixels, w * 4, 0) };
            return Ok(());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let data = D3D11_SUBRESOURCE_DATA { pSysMem: pixels, SysMemPitch: w * 4, SysMemSlicePitch: 0 };
        let texture = create(|o| unsafe { self.device.CreateTexture2D(&raw const desc, Some(&raw const data), Some(o)) })?;
        let view = create(|o| unsafe { self.device.CreateShaderResourceView(&texture, None, Some(o)) })?;
        self.textures.insert(id, Texture { texture, view });
        Ok(())
    }
}
