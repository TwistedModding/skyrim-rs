//! `ui/effect`: IC's screen effects, painted into the game every frame through the host's HUD
//! painter (`IEffect`'s `RegisterEffect::Update`, in IC's order: the letterbox, then the fade).

pub mod fade;
pub mod letterbox;

use framework::{View, egui};

pub fn paint(painter: &egui::Painter, view: &View) {
    letterbox::paint(painter, view.screen());
    fade::paint(painter, view.screen());
}
