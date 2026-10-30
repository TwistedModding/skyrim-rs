//! `hook/InputHook`: the look handler (`LookHandler` slots 5 `ProcessMouseMove` and 4
//! `ProcessThumbstick`), whose input IC's angle restriction trims before the handler takes it, and
//! the Toggle POV handler (`TogglePOVHandler` slot 6, `ProcessButton`): a release of the key
//! pressed less than 0.3 s before is a POV toggle, read before the handler takes the release. The
//! handler only marks the key held; the engine's own switch is in the camera states' handlers (RE
//! `camera.md` sections 3 and 13).

use framework::Host;
use framework::hooks::ops::Vtable;
use framework::hooks::{Error, Location, Options};
use sre::raw::vtables::{self, LookHandler_vtable};
use sre::{ButtonEvent, MouseMoveEvent, ThumbstickEvent, TogglePOVHandler};

use crate::event::input_event_manager::{look_event_gamepad, look_event_mouse, toggle_pov};

const PROCESS_BUTTON: usize = 6;
/// IC's longest hold that still counts as a tap.
const TAP: f32 = 0.3;

/// `(handler, event, PlayerControlsData*)`, the shape of every handler slot hooked here.
type ProcessEvent = unsafe extern "C" fn(usize, usize, usize) -> usize;

pub fn install(host: &'static Host) -> Result<(), Error> {
    let hooks = host.hooks();
    let look = |slot| Vtable::<ProcessEvent>::slot(Location::id(LookHandler_vtable::ID, 0), slot);
    hooks.before(look(LookHandler_vtable::SLOT_ProcessMouseMove), Options::new(host.name()), |args| {
        if let Some(event) = unsafe { (args.1 as *mut MouseMoveEvent).as_mut() } {
            look_event_mouse(event);
        }
    })?;
    hooks.before(look(LookHandler_vtable::SLOT_ProcessThumbstick), Options::new(host.name()), |args| {
        if let Some(event) = unsafe { (args.1 as *mut ThumbstickEvent).as_mut() } {
            look_event_gamepad(event);
        }
    })?;
    hooks.before(Vtable::<ProcessEvent>::slot(Location::id(vtables::TogglePOVHandler_vtable::ID, 0), PROCESS_BUTTON), Options::new(host.name()), |args| {
        let handler = unsafe { (args.0 as *const TogglePOVHandler).as_ref() };
        let event = unsafe { (args.1 as *const ButtonEvent).as_ref() };
        if let (Some(handler), Some(event)) = (handler, event)
            && handler.pressed != 0
            && event.value == 0.0
            && event.heldDownSecs < TAP
        {
            toggle_pov();
        }
    })?;
    Ok(())
}
