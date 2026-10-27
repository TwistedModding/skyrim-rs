//! The back buffer read back as it will be presented, overlay included: the game's own screenshot
//! is taken inside `RenderFrame`, before the overlay draws, so it never shows the overlay.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sre::raw::globals;
use windows::Win32::Graphics::Direct3D11::{D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D};
use windows::core::Interface;

type Done = fn(Result<PathBuf, String>);

static SAVE: Mutex<Option<(PathBuf, Done)>> = Mutex::new(None);

/// One frame's pixels, RGBA rows without padding.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Frame {
    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> Option<[u8; 3]> {
        let at = (y * self.width + x) * 4;
        self.rgba.get(at..at + 3).map(|p| [p[0], p[1], p[2]])
    }

    /// Writes a 32-bit top-down bitmap.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    pub fn write_bmp(&self, path: &Path) -> std::io::Result<()> {
        let size = self.width * self.height * 4;
        let mut out = Vec::with_capacity(54 + size);
        out.extend_from_slice(b"BM");
        for v in [(54 + size) as u32, 0, 54, 40] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(self.width as i32).to_le_bytes());
        out.extend_from_slice(&(-(self.height as i32)).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        for v in [0u32, size as u32, 2835, 2835, 0, 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for px in self.rgba.as_chunks::<4>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        std::fs::File::create(path)?.write_all(&out)
    }
}

/// Copies the back buffer through a staging texture. Call on the thread that presents, after the
/// overlay drew (from a detour on `PresentFrame`'s entry, or through [`save_next_frame`]).
pub fn read_back() -> Result<Frame, String> {
    let e = |e: windows::core::Error| e.to_string();
    let rtv = sre::g_current_render_window().map(|w| w.backBufferRTV).filter(|p| !p.is_null()).ok_or("no back buffer")?;
    let reloc = skyrim::reloc();
    let global = |id| reloc.address(id).map(|a| unsafe { *(a as *const *mut core::ffi::c_void) }).map_err(|e| e.to_string());
    let (device, context) = (global(globals::g_D3D11Device)?, global(globals::g_D3D11Context)?);
    let device = unsafe { ID3D11Device::from_raw_borrowed(&device) }.ok_or("no device")?;
    let context = unsafe { ID3D11DeviceContext::from_raw_borrowed(&context) }.ok_or("no context")?;
    let rtv = unsafe { ID3D11RenderTargetView::from_raw_borrowed(&rtv) }.ok_or("no view")?;
    let back = unsafe { rtv.GetResource() }.map_err(e)?.cast::<ID3D11Texture2D>().map_err(e)?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { back.GetDesc(&raw mut desc) };
    let staging_desc = D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0.cast_unsigned(), MiscFlags: 0, ..desc };
    let mut staging = None;
    unsafe { device.CreateTexture2D(&raw const staging_desc, None, Some(&raw mut staging)) }.map_err(e)?;
    let staging = staging.ok_or("no staging texture")?;
    unsafe { context.CopyResource(&staging, &back) };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)) }.map_err(e)?;
    let (width, height, pitch) = (desc.Width as usize, desc.Height as usize, mapped.RowPitch as usize);
    let rows = unsafe { core::slice::from_raw_parts(mapped.pData.cast::<u8>(), pitch * height) };
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        rgba.extend_from_slice(&rows[y * pitch..y * pitch + width * 4]);
    }
    unsafe { context.Unmap(&staging, 0) };
    Ok(Frame { width, height, rgba })
}

/// Writes the next frame the overlay draws to `path` as a bitmap and then calls `done` with the outcome.
pub fn save_next_frame(path: PathBuf, done: Done) {
    *SAVE.lock().unwrap() = Some((path, done));
}

/// After the overlay drew: writes the frame when one was asked for.
pub(crate) fn after_draw() {
    let Some((path, done)) = SAVE.lock().unwrap().take() else { return };
    done(read_back().and_then(|f| f.write_bmp(&path).map_err(|e| e.to_string())).map(|()| path));
}
