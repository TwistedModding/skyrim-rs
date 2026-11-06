// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! A crafted plugin on the `skse` crate alone: `scripts/loader-probe.ps1` runs it and copies of it
//! with patched version data under each script extender, and it logs what every entry point and
//! message hands it to `skyrim-rs-loader-probe.log`. See `docs/subsystems/skse-loaders.md`.

use core::ffi::{CStr, c_char};
use core::fmt::{Arguments, Write as _};
use std::io::Write as _;
use std::sync::OnceLock;

use skse::{Interface, Message, MessagingInterface};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;

skse::plugin! {
    name: "skyrim-rs-loader-probe",
    author: "skyrim-rs",
    version: 1,
    load: load,
}

const LOG: &str = "skyrim-rs-loader-probe";

static SKSE: OnceLock<usize> = OnceLock::new();

/// The name in this copy's version data, which the script patches per copy.
fn name() -> &'static CStr {
    CStr::from_bytes_until_nul(&SKSEPlugin_Version.name).unwrap_or(c"")
}

fn line(runtime: u32, text: Arguments) {
    if let Ok(mut f) = skse::log::append(runtime, LOG) {
        let thread = unsafe { GetCurrentThreadId() };
        let _ = writeln!(f, "{} thread={thread} {text}", name().to_string_lossy());
    }
}

/// `GetPluginInfo` for this copy's own name, with the dword after the info. 2.0.19 knows a copy by
/// the name `SKSEPlugin_Query` gave, which the script does not patch.
fn own_info(skse: &Interface) -> String {
    let lookup = |name: &CStr| unsafe { (skse.get_plugin_info)(name.as_ptr()).as_ref() };
    match lookup(name()).or_else(|| lookup(c"skyrim-rs-loader-probe")) {
        None => "null".to_owned(),
        Some(info) => {
            let text = if info.name.is_null() { "<null>".into() } else { unsafe { CStr::from_ptr(info.name.cast::<c_char>()) }.to_string_lossy() };
            format!("{{{} {text} {} +0x18={:#x}}}", info.info_version, info.version, unsafe { info.loader_handle() })
        }
    }
}

/// Everything an entry point can read off the interface, the handle included.
fn report(entry: &str, skse: &Interface) {
    let mut tables = String::new();
    for id in 0..=8 {
        let table = unsafe { (skse.query_interface)(id) }.cast::<u32>();
        let version = if table.is_null() { "null".to_owned() } else { format!("v{}", unsafe { table.read() }) };
        let _ = write!(tables, " {id}:{version}");
    }
    line(
        skse.runtime_version,
        format_args!(
            "{entry} loader={:08x} runtime={:08x} reserved={:?} handle={} release={} info={} tables{tables}",
            skse.skse_version,
            skse.runtime_version,
            skse.reserved,
            skse.plugin_handle(),
            unsafe { (skse.get_release_index)() },
            own_info(skse),
        ),
    );
}

/// # Safety
/// Called by the loader with its interface.
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "C" fn SKSEPlugin_Preload(skse: &Interface) -> bool {
    report("Preload", skse);
    true
}

fn load(skse: &Interface) -> bool {
    let _ = SKSE.set(core::ptr::from_ref(skse) as usize);
    report("Load", skse);
    let listening = skse.messaging().is_some_and(|m: &MessagingInterface| m.listen_skse(skse.plugin_handle(), on_message));
    line(skse.runtime_version, format_args!("listening={listening}"));
    true
}

unsafe extern "C" fn on_message(message: *mut Message) {
    let Some(&skse) = SKSE.get() else { return };
    let skse = unsafe { &*(skse as *const Interface) };
    let m = unsafe { &*message };
    let sender = unsafe { CStr::from_ptr(m.sender) }.to_string_lossy();
    line(skse.runtime_version, format_args!("kind={} len={} data={:p} sender={sender} info={}", m.kind, m.data_len, m.data, own_info(skse)));
}
