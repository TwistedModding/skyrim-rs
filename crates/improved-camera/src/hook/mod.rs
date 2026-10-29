//! `hook/MainHook`: installs every interception, in IC's order.

pub mod cameras;
pub mod fixes;
pub mod graphics;
pub mod input;
pub mod misc;

use framework::Host;
use framework::hooks::Error;

/// The player's `IsThirdPerson` (`PlayerCharacter` slot 113) as the engine's virtual calls reach it.
type IsThirdPerson = unsafe extern "C" fn(usize) -> usize;

pub fn install(host: &'static Host) -> Result<(), Error> {
    crate::camera::manager::init_near();
    graphics::install(host)?;
    cameras::install(host)?;
    fixes::install(host)?;
    input::install(host)?;
    misc::install(host)
}
