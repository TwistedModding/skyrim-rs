// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The graphics adapter the game renders with, read through DXGI from the game's own device.

use sre::raw::globals;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Dxgi::{DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter, IDXGIAdapter1, IDXGIAdapter3, IDXGIDevice};
use windows::core::Interface;

/// What DXGI reports for the adapter.
pub struct Adapter {
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    /// The user-mode driver's file version, `None` when DXGI does not report it.
    pub driver: Option<[u16; 4]>,
    pub dedicated_memory: u64,
    pub shared_memory: u64,
}

impl Adapter {
    #[must_use]
    pub fn vendor_name(&self) -> &'static str {
        match self.vendor {
            0x10DE => "NVIDIA",
            0x1002 | 0x1022 => "AMD",
            0x8086 => "Intel",
            0x1414 => "Microsoft",
            _ => "unknown vendor",
        }
    }

    /// The driver version as the vendor's own tools show it: NVIDIA's release number (`560.94` for
    /// file version `32.0.15.6094`), the file version for every other vendor.
    #[must_use]
    pub fn driver_text(&self) -> Option<String> {
        let [a, b, c, d] = self.driver?;
        let file = format!("{a}.{b}.{c}.{d}");
        if self.vendor == 0x10DE {
            let release = u32::from(c % 10) * 10_000 + u32::from(d);
            return Some(format!("{}.{:02} ({file})", release / 100, release % 100));
        }
        Some(file)
    }
}

/// The local video memory the game's process uses and the budget the OS gives it, in bytes.
pub struct VideoMemory {
    pub usage: u64,
    pub budget: u64,
}

fn dxgi_adapter() -> Option<IDXGIAdapter> {
    let global = skyrim::reloc().address(sre::id!(globals::g_D3D11Device)).ok()?;
    let raw = unsafe { *(global as *const *mut core::ffi::c_void) };
    let device = unsafe { ID3D11Device::from_raw_borrowed(&raw) }?;
    unsafe { device.cast::<IDXGIDevice>().ok()?.GetAdapter() }.ok()
}

/// The adapter of the game's device, `None` before the game creates it.
#[must_use]
pub fn adapter() -> Option<Adapter> {
    let adapter = dxgi_adapter()?;
    let desc = unsafe { adapter.cast::<IDXGIAdapter1>().ok()?.GetDesc1() }.ok()?;
    let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let driver = unsafe { adapter.CheckInterfaceSupport(&IDXGIDevice::IID) }.ok().map(|v| [(v >> 48) as u16, (v >> 32) as u16, (v >> 16) as u16, v as u16]);
    Some(Adapter {
        name: String::from_utf16_lossy(&desc.Description[..end]),
        vendor: desc.VendorId,
        device: desc.DeviceId,
        driver,
        dedicated_memory: desc.DedicatedVideoMemory as u64,
        shared_memory: desc.SharedSystemMemory as u64,
    })
}

/// The game's local video memory use, `None` before the device exists or on a DXGI without budgets.
#[must_use]
pub fn video_memory() -> Option<VideoMemory> {
    let adapter = dxgi_adapter()?.cast::<IDXGIAdapter3>().ok()?;
    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    unsafe { adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &raw mut info) }.ok()?;
    Some(VideoMemory { usage: info.CurrentUsage, budget: info.Budget })
}
