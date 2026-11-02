//! Cheaper shadow views: the frame's CPU cost of culling and building render passes for the shadow
//! maps (the sun's cascades, the focus maps and the local lights' maps) on the game's own D3D11
//! device, with Community Shaders and ENB untouched. `profile` records every step's time per frame
//! to a CSV for the measurement that drives the design (RE `render.md`, request DC); the
//! console command `DrawcallFix` controls both.

use core::ffi::{CStr, c_void};
use std::io::Write as _;
use std::sync::OnceLock;

use framework::Host;
use skyrim::input::{Arg, Command, CommandContext, Param};
use sre::ScriptParamType;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

mod detour;
mod profile;

framework::plugin! {
    name: "drawcall-fix",
    load: load,
    unload: unload,
}

static HOST: OnceLock<&'static Host> = OnceLock::new();

pub(crate) fn log(line: &str) {
    if let Some(host) = HOST.get()
        && let Ok(mut f) = skse::log::append(host.skse().runtime_version, "drawcall-fix")
    {
        let _ = writeln!(f, "{line}");
    }
}

fn load(host: &'static Host) -> bool {
    if skyrim::init(host.skse()).is_err() {
        return false;
    }
    let _ = HOST.set(host);
    match profile::install() {
        Ok(()) => log("profiling hooks in"),
        Err(e) => {
            log(&format!("profiling hooks refused: {e}"));
            return false;
        }
    }
    let _ = host.register_command::<DrawcallFix>();
    host.on_frame(profile::end_frame);
    log(&format!("loaded {}", env!("CARGO_PKG_VERSION")));
    true
}

fn unload() {
    profile::remove();
}

/// `RENDERDOC_GetAPI(10600, &table)` and `TriggerCapture` at index 15 of the 1.6 table, when
/// `renderdoc.dll` sits in the process; a capture of the next frame.
fn renderdoc_capture() -> Result<(), &'static str> {
    type GetApi = unsafe extern "C" fn(u32, *mut *const c_void) -> i32;
    let module = unsafe { GetModuleHandleA(c"renderdoc.dll".as_ptr().cast()) };
    if module.is_null() {
        return Err("renderdoc.dll is not loaded");
    }
    let get_api = unsafe { GetProcAddress(module, c"RENDERDOC_GetAPI".as_ptr().cast()) }.ok_or("RENDERDOC_GetAPI not exported")?;
    let get_api: GetApi = unsafe { core::mem::transmute::<unsafe extern "system" fn() -> isize, GetApi>(get_api) };
    let mut table: *const c_void = core::ptr::null();
    if unsafe { get_api(10600, &raw mut table) } != 1 || table.is_null() {
        return Err("RenderDoc API 1.6 refused");
    }
    let trigger: unsafe extern "C" fn() = unsafe { table.cast::<unsafe extern "C" fn()>().add(15).read() };
    unsafe { trigger() };
    Ok(())
}

/// `Renderer::presentInterval`, the swap chain's sync interval copied from `iVSyncPresentInterval`
/// at startup and read by every `PresentFrame`.
fn present_interval() -> *mut u32 {
    (&raw const sre::g_renderer().presentInterval).cast_mut()
}

fn set_shadow_distance(distance: f32) {
    let mut settings = skyrim::render::cascade_settings();
    settings.shadow_distance = distance;
    skyrim::render::set_cascade_settings(settings);
    skyrim::render::set_shadow_distance(distance);
}

static PARAMS: [Param; 2] = [Param::new(c"action", ScriptParamType::String, true), Param::new(c"value", ScriptParamType::String, true)];

struct DrawcallFix;

impl Command for DrawcallFix {
    const NAME: &'static CStr = c"DrawcallFix";
    const HELP: &'static CStr = c"DrawcallFix [status | profile <label>|off | shadowdistance <units> | vsync <interval> | capture]";
    const PARAMS: &'static [Param] = &PARAMS;

    fn execute(context: &mut CommandContext<'_>) -> bool {
        let text = |arg: Option<&Arg<'_>>| match arg {
            Some(Arg::Str(s)) => s.to_string(),
            Some(Arg::Int(i)) => i.to_string(),
            Some(Arg::Float(f)) => f.to_string(),
            _ => String::new(),
        };
        let (action, value) = (text(context.args().first()).to_ascii_lowercase(), text(context.args().get(1)));
        let runtime = HOST.get().map_or(0, |h| h.skse().runtime_version);
        let reply = match action.as_str() {
            "profile" if value.eq_ignore_ascii_case("off") || value.is_empty() => format!("profile stopped after {} frames", profile::stop()),
            "profile" => match profile::start(runtime, &value) {
                Ok(()) => format!("profile {value} recording"),
                Err(e) => format!("profile not started: {e}"),
            },
            "shadowdistance" => match value.parse::<f32>() {
                Ok(d) if d > 0.0 => {
                    set_shadow_distance(d);
                    format!("shadow distance {d} (live {})", skyrim::render::shadow_distance())
                }
                _ => "shadowdistance needs a positive number".into(),
            },
            "vsync" => match value.parse::<u32>() {
                Ok(interval) if interval <= 4 => {
                    unsafe { present_interval().write_unaligned(interval) };
                    format!("present interval {interval}")
                }
                _ => "vsync needs an interval from 0 to 4".into(),
            },
            "capture" => match renderdoc_capture() {
                Ok(()) => "RenderDoc capture triggered".into(),
                Err(e) => format!("no capture: {e}"),
            },
            _ => format!("drawcall-fix {}: shadow distance {}, present interval {}; {}", env!("CARGO_PKG_VERSION"), skyrim::render::shadow_distance(), unsafe { present_interval().read_unaligned() }, profile::status()),
        };
        log(&reply);
        skyrim::input::print(&reply);
        true
    }
}
