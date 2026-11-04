//! Paints tessellated `egui` meshes into the frame target without changing the immediate context's state.

use core::ffi::c_void;
use std::collections::HashMap;

use egui::epaint::{ImageDelta, Primitive, Vertex, textures::TexturesDelta};
use egui::{ClippedPrimitive, ImageData, TextureId};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_FLAG, D3D11_BIND_INDEX_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VERTEX_BUFFER, D3D11_BLEND_DESC, D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE, D3D11_BLEND_OP_ADD,
    D3D11_BOX, D3D11_BUFFER_DESC, D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_COMPARISON_ALWAYS, D3D11_CPU_ACCESS_WRITE, D3D11_CULL_NONE, D3D11_DEPTH_STENCIL_DESC, D3D11_FILL_SOLID, D3D11_FILTER_MIN_MAG_MIP_LINEAR,
    D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA, D3D11_MAP_WRITE_DISCARD, D3D11_MAPPED_SUBRESOURCE, D3D11_RASTERIZER_DESC, D3D11_RENDER_TARGET_BLEND_DESC, D3D11_SAMPLER_DESC, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_DYNAMIC, D3D11_VIEWPORT, ID3D11BlendState, ID3D11Buffer, ID3D11DepthStencilState, ID3D11Device, ID3D11DeviceContext,
    ID3D11DepthStencilView, ID3D11DomainShader, ID3D11GeometryShader, ID3D11HullShader, ID3D11InputLayout, ID3D11PixelShader, ID3D11RasterizerState, ID3D11RenderTargetView, ID3D11SamplerState,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
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

/// The state [`Painter::draw`] changes on the immediate context.
#[derive(Default)]
struct SavedState {
    layout: Option<ID3D11InputLayout>,
    topology: D3D_PRIMITIVE_TOPOLOGY,
    vertex_buffer: Option<ID3D11Buffer>,
    stride: u32,
    vertex_offset: u32,
    index_buffer: Option<ID3D11Buffer>,
    index_format: DXGI_FORMAT,
    index_offset: u32,
    vertex_shader: Option<ID3D11VertexShader>,
    vertex_constants: [Option<ID3D11Buffer>; 1],
    hull_shader: Option<ID3D11HullShader>,
    domain_shader: Option<ID3D11DomainShader>,
    geometry_shader: Option<ID3D11GeometryShader>,
    pixel_shader: Option<ID3D11PixelShader>,
    sampler: [Option<ID3D11SamplerState>; 1],
    view: [Option<ID3D11ShaderResourceView>; 1],
    raster: Option<ID3D11RasterizerState>,
    viewports: [D3D11_VIEWPORT; 16],
    viewport_count: u32,
    scissors: [RECT; 16],
    scissor_count: u32,
    blend: Option<ID3D11BlendState>,
    blend_factor: [f32; 4],
    sample_mask: u32,
    depth: Option<ID3D11DepthStencilState>,
    stencil_ref: u32,
    targets: [Option<ID3D11RenderTargetView>; 8],
    depth_target: Option<ID3D11DepthStencilView>,
}

impl SavedState {
    fn capture(context: &ID3D11DeviceContext) -> Self {
        let mut s = Self { viewport_count: 16, scissor_count: 16, ..Self::default() };
        unsafe {
            s.layout = context.IAGetInputLayout().ok();
            s.topology = context.IAGetPrimitiveTopology();
            context.IAGetVertexBuffers(0, 1, Some(&raw mut s.vertex_buffer), Some(&raw mut s.stride), Some(&raw mut s.vertex_offset));
            context.IAGetIndexBuffer(Some(&raw mut s.index_buffer), Some(&raw mut s.index_format), Some(&raw mut s.index_offset));
            context.VSGetShader(&raw mut s.vertex_shader, None, None);
            context.VSGetConstantBuffers(0, Some(&mut s.vertex_constants));
            context.HSGetShader(&raw mut s.hull_shader, None, None);
            context.DSGetShader(&raw mut s.domain_shader, None, None);
            context.GSGetShader(&raw mut s.geometry_shader, None, None);
            context.PSGetShader(&raw mut s.pixel_shader, None, None);
            context.PSGetSamplers(0, Some(&mut s.sampler));
            context.PSGetShaderResources(0, Some(&mut s.view));
            s.raster = context.RSGetState().ok();
            context.RSGetViewports(&raw mut s.viewport_count, Some(s.viewports.as_mut_ptr()));
            context.RSGetScissorRects(&raw mut s.scissor_count, Some(s.scissors.as_mut_ptr()));
            context.OMGetBlendState(Some(&raw mut s.blend), Some(&mut s.blend_factor), Some(&raw mut s.sample_mask));
            context.OMGetDepthStencilState(Some(&raw mut s.depth), Some(&raw mut s.stencil_ref));
            context.OMGetRenderTargets(Some(&mut s.targets), Some(&raw mut s.depth_target));
        }
        s
    }

    fn restore(&self, context: &ID3D11DeviceContext) {
        unsafe {
            // Targets before views: binding a target unbinds any view of the same resource.
            context.OMSetRenderTargets(Some(&self.targets), self.depth_target.as_ref());
            context.OMSetDepthStencilState(self.depth.as_ref(), self.stencil_ref);
            context.OMSetBlendState(self.blend.as_ref(), Some(&self.blend_factor), self.sample_mask);
            context.RSSetScissorRects(Some(&self.scissors[..self.scissor_count as usize]));
            context.RSSetViewports(Some(&self.viewports[..self.viewport_count as usize]));
            context.RSSetState(self.raster.as_ref());
            context.PSSetShaderResources(0, Some(&self.view));
            context.PSSetSamplers(0, Some(&self.sampler));
            context.PSSetShader(self.pixel_shader.as_ref(), None);
            context.GSSetShader(self.geometry_shader.as_ref(), None);
            context.DSSetShader(self.domain_shader.as_ref(), None);
            context.HSSetShader(self.hull_shader.as_ref(), None);
            context.VSSetConstantBuffers(0, Some(&self.vertex_constants));
            context.VSSetShader(self.vertex_shader.as_ref(), None);
            context.IASetIndexBuffer(self.index_buffer.as_ref(), self.index_format, self.index_offset);
            context.IASetVertexBuffers(0, 1, Some(&raw const self.vertex_buffer), Some(&raw const self.stride), Some(&raw const self.vertex_offset));
            context.IASetPrimitiveTopology(self.topology);
            context.IASetInputLayout(self.layout.as_ref());
        }
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
    /// The active full-size frame target, or the window's view when no matching target is bound.
    ///
    /// # Safety
    /// `fallback` must be the window's live `ID3D11RenderTargetView`.
    pub(crate) unsafe fn frame_target(fallback: *mut c_void) -> Result<ID3D11RenderTargetView> {
        let missing = || windows::core::Error::from(windows::Win32::Foundation::E_POINTER);
        let fallback = unsafe { ID3D11RenderTargetView::from_raw_borrowed(&fallback) }.ok_or_else(missing)?;
        let desc = unsafe { Self::target_desc(fallback.as_raw()) }.ok_or_else(missing)?;
        let context = unsafe { fallback.GetDevice()?.GetImmediateContext()? };
        let mut bound = [None];
        unsafe { context.OMGetRenderTargets(Some(&mut bound), None) };
        if let Some(target) = bound[0].take() {
            let active = unsafe { Self::target_desc(target.as_raw()) };
            if active.is_some_and(|a| a.Width == desc.Width && a.Height == desc.Height && a.SampleDesc.Count == desc.SampleDesc.Count) {
                return Ok(target);
            }
        }
        Ok(fallback.clone())
    }

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

    /// The texture behind a render target view.
    ///
    /// # Safety
    /// `rtv` must be a live `ID3D11RenderTargetView`.
    pub(crate) unsafe fn target_desc(rtv: *mut c_void) -> Option<D3D11_TEXTURE2D_DESC> {
        let rtv = unsafe { ID3D11RenderTargetView::from_raw_borrowed(&rtv) }?;
        let texture = unsafe { rtv.GetResource() }.ok()?.cast::<ID3D11Texture2D>().ok()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&raw mut desc) };
        Some(desc)
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
        let saved = SavedState::capture(context);
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
        saved.restore(context);
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Color32, ColorImage, Mesh, Rect, TextureOptions, pos2, vec2};
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D11_PRIMITIVE_TOPOLOGY_LINELIST};
    use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_USAGE_STAGING, D3D11CreateDevice};

    fn pixel(device: &ID3D11Device, context: &ID3D11DeviceContext, texture: &ID3D11Texture2D, x: usize, y: usize) -> [u8; 4] {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&raw mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0.cast_unsigned();
            let staging = create(|out| device.CreateTexture2D(&raw const desc, None, Some(out))).unwrap();
            context.CopyResource(&staging, texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)).unwrap();
            let bytes = mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize + x * 4).cast::<[u8; 4]>().read_unaligned();
            context.Unmap(&staging, 0);
            bytes
        }
    }

    fn warp() -> (ID3D11Device, ID3D11DeviceContext) {
        let (mut device, mut immediate) = (None, None);
        unsafe { D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, windows::Win32::Foundation::HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), None, D3D11_SDK_VERSION, Some(&raw mut device), None, Some(&raw mut immediate)) }.unwrap();
        (device.unwrap(), immediate.unwrap())
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn a_large_partial_update_lands_where_it_was_asked() {
        let (device, immediate) = warp();
        let mut painter = unsafe { Painter::new(device.as_raw(), immediate.as_raw()) }.unwrap();
        let id = TextureId::Managed(0);
        let mut delta = TexturesDelta::default();
        delta.push(id, ImageDelta::full(ColorImage::filled([2048, 64], Color32::BLACK), TextureOptions::LINEAR));
        let [w, h] = [2031, 39];
        let color = |x: usize, y: usize| Color32::from_rgba_premultiplied((x % 251) as u8, (x / 251) as u8, y as u8, 255);
        let patch = (0..h).flat_map(|y| (0..w).map(move |x| color(x, y))).collect();
        delta.push(id, ImageDelta::partial([0, 21], ColorImage::new([w, h], patch), TextureOptions::LINEAR));
        unsafe { painter.paint(core::ptr::null_mut(), [0, 0], 1.0, &[], &delta) }.unwrap();
        delta.clear();
        let texture = &painter.textures[&id].texture;
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1), (1000, 20)] {
            assert_eq!(pixel(&device, &immediate, texture, x, 21 + y), color(x, y).to_array(), "at {x}, {y}");
        }
        assert_eq!(pixel(&device, &immediate, texture, 0, 20), [0, 0, 0, 255], "the rows above are untouched");
    }

    #[test]
    fn paints_the_active_frame_and_preserves_immediate_state() {
        unsafe {
            let (device, immediate) = warp();
            let texture = |width| {
                create(|out| {
                    device.CreateTexture2D(
                        &D3D11_TEXTURE2D_DESC {
                            Width: width,
                            Height: 32,
                            MipLevels: 1,
                            ArraySize: 1,
                            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                            Usage: D3D11_USAGE_DEFAULT,
                            BindFlags: D3D11_BIND_RENDER_TARGET.0.cast_unsigned(),
                            ..Default::default()
                        },
                        None,
                        Some(out),
                    )
                })
                .unwrap()
            };
            let fallback_texture = texture(64);
            let active_texture = texture(64);
            let small_texture = texture(16);
            let view = |texture: &ID3D11Texture2D| create(|out| device.CreateRenderTargetView(texture, None, Some(out))).unwrap();
            let (fallback, active, small) = (view(&fallback_texture), view(&active_texture), view(&small_texture));
            immediate.ClearRenderTargetView(&fallback, &[0.0, 0.0, 0.0, 1.0]);
            immediate.ClearRenderTargetView(&active, &[0.0, 0.0, 0.0, 1.0]);
            immediate.OMSetRenderTargets(Some(&[Some(small)]), None);
            assert_eq!(Painter::frame_target(fallback.as_raw()).unwrap(), fallback, "a smaller pass is not the frame");
            immediate.OMSetRenderTargets(None, None);
            assert_eq!(Painter::frame_target(fallback.as_raw()).unwrap(), fallback);
            immediate.OMSetRenderTargets(Some(&[Some(active.clone())]), None);
            let selected = Painter::frame_target(fallback.as_raw()).unwrap();
            assert_eq!(selected, active, "the active frame can differ from the stored window view");
            immediate.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_LINELIST);
            immediate.RSSetViewports(Some(&[D3D11_VIEWPORT { Width: 7.0, Height: 5.0, MaxDepth: 1.0, ..Default::default() }]));
            let second_target = view(&texture(64));
            immediate.OMSetRenderTargets(Some(&[Some(active.clone()), Some(second_target.clone())]), None);
            let scissor = RECT { left: 1, top: 2, right: 3, bottom: 4 };
            immediate.RSSetScissorRects(Some(&[scissor]));
            immediate.OMSetBlendState(None, Some(&[0.25; 4]), 0xF0);

            let mut painter = Painter::new(device.as_raw(), immediate.as_raw()).unwrap();
            let id = TextureId::Managed(0);
            let mut delta = TexturesDelta::default();
            delta.push(id, ImageDelta::full(ColorImage::filled([2, 2], Color32::BLACK), TextureOptions::LINEAR));
            delta.push(id, ImageDelta::partial([1, 1], ColorImage::filled([1, 1], Color32::WHITE), TextureOptions::NEAREST));
            let mut mesh = Mesh::with_texture(id);
            mesh.add_colored_rect(Rect::from_min_size(pos2(8.0, 8.0), vec2(32.0, 16.0)), Color32::WHITE);
            for vertex in &mut mesh.vertices {
                vertex.uv = pos2(0.75, 0.75);
            }
            let primitive = ClippedPrimitive { clip_rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(64.0, 32.0)), primitive: Primitive::Mesh(mesh) };
            painter.paint(selected.as_raw(), [64, 32], 1.0, &[primitive], &delta).unwrap();
            delta.clear();

            let mut targets = [None, None];
            immediate.OMGetRenderTargets(Some(&mut targets), None);
            assert_eq!(targets, [Some(active.clone()), Some(second_target)]);
            let (mut count, mut rect) = (1, RECT::default());
            immediate.RSGetScissorRects(&raw mut count, Some(&raw mut rect));
            assert_eq!((count, rect), (1, scissor));
            let (mut factor, mut mask) = ([0.0; 4], 0);
            immediate.OMGetBlendState(None, Some(&mut factor), Some(&raw mut mask));
            assert_eq!((factor, mask), ([0.25; 4], 0xF0));
            let mut shader = None;
            immediate.VSGetShader(&raw mut shader, None, None);
            assert!(shader.is_none(), "the overlay's shader does not stay bound");
            assert!(immediate.IAGetInputLayout().is_err(), "nor its input layout");
            assert_eq!(immediate.IAGetPrimitiveTopology(), D3D11_PRIMITIVE_TOPOLOGY_LINELIST);
            let (mut count, mut viewport) = (1, D3D11_VIEWPORT::default());
            immediate.RSGetViewports(&raw mut count, Some(&raw mut viewport));
            assert_eq!((count, viewport.Width, viewport.Height), (1, 7.0, 5.0));
            immediate.OMSetRenderTargets(None, None);
            assert_eq!(pixel(&device, &immediate, &active_texture, 16, 16), [255; 4], "paints the selected frame");
            assert_eq!(pixel(&device, &immediate, &fallback_texture, 16, 16), [0, 0, 0, 255], "the stored output view is untouched");
        }
    }
}
