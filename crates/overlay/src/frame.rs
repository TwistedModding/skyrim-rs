//! The back buffer read back as it will be presented: before the overlay draws (a frame callback,
//! for a mod's screenshot) or after it (overlay included; the game's own screenshot is taken inside
//! `RenderFrame`, before the overlay draws, so it never shows the overlay), and written as a bitmap
//! or, through WIC, a PNG.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sre::raw::globals;
use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Direct3D11::{D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_RENDER_TARGET_VIEW_DESC, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN};
use windows::Win32::Graphics::Imaging::{CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat24bppBGR, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapEncoderNoCache, WICBitmapPaletteTypeCustom};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::core::{HSTRING, Interface};

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

    /// The `width` x `height` rectangle at `left`, `top`, clipped to the frame.
    #[must_use]
    pub fn crop(&self, left: usize, top: usize, width: usize, height: usize) -> Self {
        let (left, top) = (left.min(self.width), top.min(self.height));
        let (width, height) = (width.min(self.width - left), height.min(self.height - top));
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in top..top + height {
            let row = (y * self.width + left) * 4;
            rgba.extend_from_slice(&self.rgba[row..row + width * 4]);
        }
        Self { width, height, rgba }
    }

    /// Writes a 24-bit PNG (the alpha left out) through the Windows Imaging Component.
    #[allow(clippy::cast_possible_truncation)]
    pub fn write_png(&self, path: &Path) -> windows::core::Result<()> {
        let factory = wic()?;
        let (width, height) = (self.width as u32, self.height as u32);
        unsafe {
            let bitmap = factory.CreateBitmapFromMemory(width, height, &GUID_WICPixelFormat32bppRGBA, width * 4, &self.rgba)?;
            let converter = factory.CreateFormatConverter()?;
            converter.Initialize(&bitmap, &GUID_WICPixelFormat24bppBGR, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
            let stream = factory.CreateStream()?;
            stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
            let encoder = factory.CreateEncoder(&GUID_ContainerFormatPng, core::ptr::null())?;
            encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
            let (mut frame, mut options) = (None, None);
            encoder.CreateNewFrame(&raw mut frame, &raw mut options)?;
            let frame = frame.ok_or_else(windows::core::Error::empty)?;
            frame.Initialize(options.as_ref())?;
            frame.SetSize(width, height)?;
            let mut format = GUID_WICPixelFormat24bppBGR;
            frame.SetPixelFormat(&raw mut format)?;
            frame.WriteSource(&converter, core::ptr::null())?;
            frame.Commit()?;
            encoder.Commit()
        }
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
/// overlay drew (from a detour on `PresentFrame`'s entry, or through [`save_next_frame`]). The
/// game creates its swap chain as `R8G8B8A8_UNORM` and keeps it on a resize (RE `camera.md` IC8 N),
/// so any other format means something replaced the swap chain and is refused.
pub fn read_back() -> Result<Frame, String> {
    let rtv = sre::g_current_render_window().map(|w| w.backBufferRTV).filter(|p| !p.is_null()).ok_or("no back buffer")?;
    read_target(rtv)
}

fn read_target(rtv: *mut core::ffi::c_void) -> Result<Frame, String> {
    let e = |e: windows::core::Error| e.to_string();
    let reloc = skyrim::reloc();
    let global = |id| reloc.address(id).map(|a| unsafe { *(a as *const *mut core::ffi::c_void) }).map_err(|e| e.to_string());
    let (device, context) = (global(sre::id!(globals::g_D3D11Device))?, global(sre::id!(globals::g_D3D11Context))?);
    let device = unsafe { ID3D11Device::from_raw_borrowed(&device) }.ok_or("no device")?;
    let context = unsafe { ID3D11DeviceContext::from_raw_borrowed(&context) }.ok_or("no context")?;
    let rtv = unsafe { ID3D11RenderTargetView::from_raw_borrowed(&rtv) }.ok_or("no view")?;
    let back = unsafe { rtv.GetResource() }.map_err(e)?.cast::<ID3D11Texture2D>().map_err(e)?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { back.GetDesc(&raw mut desc) };
    let mut view = D3D11_RENDER_TARGET_VIEW_DESC::default();
    unsafe { rtv.GetDesc(&raw mut view) };
    let format = if view.Format == DXGI_FORMAT_UNKNOWN { desc.Format } else { view.Format };
    if format != DXGI_FORMAT_R8G8B8A8_UNORM {
        return Err(format!("the back buffer's format {} is not R8G8B8A8_UNORM", format.0));
    }
    if desc.SampleDesc.Count > 1 {
        return Err(format!("the back buffer is multisampled ({}x)", desc.SampleDesc.Count));
    }
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

/// The WIC factory; a thread without COM gets an apartment first.
fn wic() -> windows::core::Result<IWICImagingFactory> {
    let create = || unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) };
    create().or_else(|_| {
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        create()
    })
}

/// Writes the next frame the overlay draws to `path` as a bitmap and then calls `done` with the outcome.
pub fn save_next_frame(path: PathBuf, done: Done) {
    *SAVE.lock().unwrap() = Some((path, done));
}

/// After the overlay drew: writes the frame when one was asked for.
pub(crate) fn after_draw(rtv: *mut core::ffi::c_void) {
    let Some((path, done)) = SAVE.lock().unwrap().take() else { return };
    done(read_target(rtv).and_then(|f| f.write_bmp(&path).map_err(|e| e.to_string())).map(|()| path));
}

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand};
    use windows::core::HSTRING;

    use super::{Frame, wic};

    fn frame() -> Frame {
        let rgba = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [10, 20, 30, 0], [1, 2, 3, 255], [200, 100, 50, 255]].concat();
        Frame { width: 3, height: 2, rgba }
    }

    #[test]
    fn a_crop_keeps_its_rows_and_clips_to_the_frame() {
        let f = frame().crop(1, 0, 5, 1);
        assert_eq!((f.width, f.height), (2, 1));
        assert_eq!(f.rgba, [0, 255, 0, 255, 0, 0, 255, 255]);
        assert_eq!(frame().crop(2, 1, 1, 1).pixel(0, 0), Some([200, 100, 50]));
        assert_eq!(frame().crop(9, 9, 1, 1).rgba.len(), 0);
    }

    #[test]
    fn a_png_written_reads_back_with_the_same_pixels() {
        let path = std::env::temp_dir().join(format!("overlay-frame-{}.png", std::process::id()));
        frame().write_png(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        let factory = wic().unwrap();
        let mut rgba = vec![0u8; 3 * 2 * 4];
        let (mut width, mut height) = (0, 0);
        unsafe {
            let file = factory.CreateDecoderFromFilename(&HSTRING::from(path.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).unwrap();
            let decoded = file.GetFrame(0).unwrap();
            decoded.GetSize(&raw mut width, &raw mut height).unwrap();
            let converter = factory.CreateFormatConverter().unwrap();
            converter.Initialize(&decoded, &GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).unwrap();
            converter.CopyPixels(core::ptr::null(), 12, &mut rgba).unwrap();
        }
        let _ = std::fs::remove_file(&path);
        assert_eq!((width, height), (3, 2));
        let expected: Vec<u8> = frame().rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        assert_eq!(rgba, expected, "the alpha is left out");
    }
}
