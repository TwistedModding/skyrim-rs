//! `hook/MainHook`: installs every interception, in IC's order.

pub mod cameras;
pub mod fixes;
pub mod graphics;
pub mod misc;

use framework::Host;
use framework::hooks::Error;

pub fn install(host: &'static Host) -> Result<(), Error> {
    graphics::install(host)?;
    cameras::install(host)?;
    fixes::install(host)?;
    misc::install(host)
}
