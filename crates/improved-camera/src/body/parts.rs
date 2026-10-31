//! `body/BodyParts`: which parts of the third-person body the player sees in first person. The
//! arms, head and sheathed gear stay at full scale for the shadow pass; the parts marked hidden
//! here are shrunk after it for the main view (`hook::fixes`) and restored by the next update.

use std::sync::Mutex;

use skyrim::prelude::*;
use sre::{CameraState, NiAVObject, PlayerCharacter};

use super::snapshot;
use super::state::action::ActionType;
use super::state::hands::{EquipmentType, HandsType, InteractionType};
use super::state::movement::MovementType;
use super::state::race::RaceType;
use crate::camera::snapshot as camera_snapshot;
use crate::camera::state::CameraType;
use crate::settings::{General, Profile};
use crate::util::skyrim_util::{self as util, FAKE_FIRST_PERSON_ZOOM, Idle};

/// Scale of a part hidden by shrinking.
pub const SHRUNK: f32 = 0.001;

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HiddenParts {
    pub left_arm: bool,
    pub right_arm: bool,
    pub head: bool,
    pub weapon_back: bool,
    pub weapon_bow: bool,
    pub quiver: bool,
}

static HIDDEN: Mutex<HiddenParts> = Mutex::new(HiddenParts { left_arm: false, right_arm: false, head: false, weapon_back: false, weapon_bow: false, quiver: false });

#[must_use]
pub fn hidden_parts() -> HiddenParts {
    *HIDDEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn set_hidden(update: impl FnOnce(&mut HiddenParts)) {
    update(&mut HIDDEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheathedGroup {
    WeaponBack,
    WeaponBow,
    Quiver,
}

impl SheathedGroup {
    #[must_use]
    pub const fn hidden(self, parts: HiddenParts) -> bool {
        match self {
            Self::WeaponBack => parts.weapon_back,
            Self::WeaponBow => parts.weapon_bow,
            Self::Quiver => parts.quiver,
        }
    }
}

/// The third-person sheathed-gear nodes hidden from the player's own view: vanilla, XPMSE and IED
/// placements. The shrink after the shadow pass and the restore before the next one both walk
/// this table, so a shrunk node always comes back.
pub const SHEATHED_NODES: [(&str, SheathedGroup); 9] = [
    ("WeaponBack", SheathedGroup::WeaponBack),
    ("WeaponBackLeft", SheathedGroup::WeaponBack),
    ("WeaponBackAxeMace", SheathedGroup::WeaponBack),
    ("MOV WeaponBackDefault", SheathedGroup::WeaponBack),
    ("WeaponBow", SheathedGroup::WeaponBow),
    ("WeaponCrossBow", SheathedGroup::WeaponBow),
    ("MOV WeaponBowDefault", SheathedGroup::WeaponBow),
    ("QUIVER", SheathedGroup::Quiver),
    ("MOV QUIVERDefault", SheathedGroup::Quiver),
];

/// The full scales of the parts, kept while they are shrunk.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scales {
    pub fade_amount: f32,
    pub head: f32,
    pub left_first: f32,
    pub right_first: f32,
    pub left_third: f32,
    pub right_third: f32,
    pub sheathed: [f32; SHEATHED_NODES.len()],
    pub quiver: f32,
}

impl Default for Scales {
    fn default() -> Self {
        Self { fade_amount: 1.0, head: 1.0, left_first: 1.0, right_first: 1.0, left_third: 1.0, right_third: 1.0, sheathed: [1.0; SHEATHED_NODES.len()], quiver: 1.0 }
    }
}

/// The nodes of one update (`GetData`), looked up again every frame.
struct Nodes<'a> {
    first: Option<&'a NiAVObject>,
    third: &'a NiAVObject,
    head: Option<&'a NiAVObject>,
    eye: Option<&'a NiAVObject>,
    left_first: Option<&'a NiAVObject>,
    right_first: Option<&'a NiAVObject>,
    left_third: Option<&'a NiAVObject>,
    right_third: Option<&'a NiAVObject>,
    sheathed: [Option<&'a NiAVObject>; SHEATHED_NODES.len()],
    quiver_first: Option<&'a NiAVObject>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Parts {
    pub scales: Scales,
    /// The first-person model faded out for the dialogue's speaker focus.
    first_person_faded: bool,
}

/// Remembers a part's scale unless it is shrunk now.
fn keep_scale(node: Option<&NiAVObject>, scale: &mut f32) {
    if let Some(node) = node
        && node.local.scale > SHRUNK
    {
        *scale = node.local.scale;
    }
}

#[allow(clippy::float_cmp)]
fn restore_scale(node: Option<&NiAVObject>, scale: f32) {
    if let Some(node) = node
        && node.local.scale != scale
    {
        node.set_local_scale(scale);
    }
}

impl Parts {
    pub fn update(&mut self, player: &PlayerCharacter, profile: &Profile) {
        let Some(nodes) = self.get_data(player) else { return };
        let camera_data = camera_snapshot::get();
        let camera = camera_data.current;
        if !profile.General.bEnableBody || matches!(camera, CameraType::RaceMenu | CameraType::Free | CameraType::SexTFC) {
            self.restore_scales(&nodes);
            return;
        }
        if nodes.eye.is_some() && util::is_first_person() && camera_data.transition == CameraType::Unknown {
            self.process_body(&nodes, profile);
            if let (Some(lf), Some(rf), Some(lt), Some(rt)) = (nodes.left_first, nodes.right_first, nodes.left_third, nodes.right_third) {
                self.process_arms([lf, rf, lt, rt], &profile.General);
            }
            if let Some(head) = nodes.head {
                let engine_state = util::camera().and_then(PlayerCameraExt::state);
                let focus = camera_snapshot::get().dialogue_focus;
                set_hidden(|h| h.head = !(focus || draw_head(&profile.General, snapshot::get().hands, camera, engine_state, util::zoom_of)));
                head.set_local_scale(self.scales.head);
            }
            self.process_weapons(&nodes, profile);
        } else {
            self.restore_scales(&nodes);
        }
    }

    /// Every part back at its full scale, as when the body is off.
    pub fn restore(&mut self, player: &PlayerCharacter) {
        if let Some(nodes) = self.get_data(player) {
            self.restore_scales(&nodes);
        }
    }

    fn get_data<'a>(&mut self, player: &'a PlayerCharacter) -> Option<Nodes<'a>> {
        let third = player.third_person_root()?;
        util::object_by_name(third, "NPC Root [Root]")?;
        let first = player.first_person_root().map(|n| -> &NiAVObject { n });
        let s = &mut self.scales;
        if third.fadeAmount > 0.0 {
            s.fade_amount = third.fadeAmount;
        }
        let head_part = util::actor_head(player);
        let head = head_part.and_then(|h| unsafe { h.parent.cast::<NiAVObject>().as_ref() });
        keep_scale(head, &mut s.head);
        let race = snapshot::get().race;
        let eye = head.and_then(|_| util::actor_eye(player)).or_else(|| match race {
            RaceType::Vampirelord => util::object_by_name(third, "NPC EyeBrow"),
            RaceType::Lich => util::object_by_name(third, "DragPriestNPC Head [Brow]"),
            _ => None,
        });
        let in_first = |name| first.and_then(|f| util::object_by_name(f, name));
        let nodes = Nodes {
            first,
            third,
            head,
            eye,
            left_first: in_first("NPC L UpperArm [LUar]"),
            right_first: in_first("NPC R UpperArm [RUar]"),
            left_third: util::object_by_name(third, "NPC L UpperArm [LUar]"),
            right_third: util::object_by_name(third, "NPC R UpperArm [RUar]"),
            sheathed: SHEATHED_NODES.map(|(name, _)| util::object_by_name(third, name)),
            quiver_first: in_first("QUIVER"),
        };
        keep_scale(nodes.left_first, &mut s.left_first);
        keep_scale(nodes.right_first, &mut s.right_first);
        keep_scale(nodes.left_third, &mut s.left_third);
        keep_scale(nodes.right_third, &mut s.right_third);
        for (i, (node, (_, group))) in nodes.sheathed.iter().zip(SHEATHED_NODES).enumerate() {
            keep_scale(*node, &mut s.sheathed[i]);
            if node.is_some() && group == SheathedGroup::Quiver {
                s.quiver = s.sheathed[i];
            }
        }
        Some(nodes)
    }

    fn process_arms(&self, [left_first, right_first, left_third, right_third]: [&NiAVObject; 4], general: &General) {
        let body = snapshot::get();
        let engine_state = util::camera().and_then(PlayerCameraExt::state);
        let arms = if camera_snapshot::get().dialogue_focus { 1 } else { draw_arms(general, &body, engine_state) };
        let (hide_left, hide_right) = match arms {
            -1 => (true, true),
            0 => (!draw_left_arm(general, &body), !draw_right_arm(general, &body)),
            _ => (false, false),
        };
        left_first.set_local_scale(if hide_left { self.scales.left_first } else { SHRUNK });
        right_first.set_local_scale(if hide_right { self.scales.right_first } else { SHRUNK });
        left_third.set_local_scale(self.scales.left_third);
        right_third.set_local_scale(self.scales.right_third);
        set_hidden(|h| {
            h.left_arm = hide_left;
            h.right_arm = hide_right;
        });
    }

    fn process_body(&mut self, nodes: &Nodes<'_>, profile: &Profile) {
        #[allow(clippy::float_cmp)]
        if let Some(first) = nodes.first
            && first.local.scale != nodes.third.local.scale
        {
            first.set_local_scale(nodes.third.local.scale);
        }
        // The first-person model rides the camera; scaling it would break the dialogue head tracking that reads its nodes.
        if let Some(first) = nodes.first {
            if camera_snapshot::get().dialogue_focus {
                skyrim::store(&first.fadeAmount, 0.0);
                self.first_person_faded = true;
            } else if core::mem::take(&mut self.first_person_faded) {
                skyrim::store(&first.fadeAmount, 1.0);
            }
        }
        skyrim::store(&nodes.third.fadeAmount, body_fade(profile, &snapshot::get(), self.scales.fade_amount));
    }

    fn process_weapons(&self, nodes: &Nodes<'_>, profile: &Profile) {
        let hide = &profile.Hide;
        for (node, scale) in nodes.sheathed.iter().zip(self.scales.sheathed) {
            if let Some(node) = node {
                node.set_local_scale(scale);
            }
        }
        if let Some(quiver) = nodes.quiver_first {
            quiver.set_local_scale(if hide.bQuiver { SHRUNK } else { self.scales.quiver });
        }
        set_hidden(|h| {
            h.weapon_back = hide.bTwoHanded;
            h.weapon_bow = hide.bBow;
            h.quiver = hide.bQuiver;
        });
    }

    #[allow(clippy::float_cmp)]
    fn restore_scales(&mut self, nodes: &Nodes<'_>) {
        set_hidden(|h| *h = HiddenParts::default());
        let third_to_first = util::is_first_person() && camera_snapshot::get().transition == CameraType::ThirdPerson;
        if core::mem::take(&mut self.first_person_faded)
            && let Some(first) = nodes.first
        {
            skyrim::store(&first.fadeAmount, 1.0);
        }
        let s = &self.scales;
        if nodes.third.fadeAmount != s.fade_amount {
            skyrim::store(&nodes.third.fadeAmount, s.fade_amount);
        }
        restore_scale(nodes.head, s.head);
        restore_scale(nodes.left_first, if third_to_first { SHRUNK } else { s.left_first });
        restore_scale(nodes.right_first, if third_to_first { SHRUNK } else { s.right_first });
        restore_scale(nodes.left_third, s.left_third);
        restore_scale(nodes.right_third, s.right_third);
        for (node, scale) in nodes.sheathed.iter().zip(s.sheathed) {
            restore_scale(*node, scale);
        }
        restore_scale(nodes.quiver_first, s.quiver);
    }
}

/// `ProcessBody`'s fade of the third-person body: 0 hides it in the states the Hide options name.
#[must_use]
pub fn body_fade(profile: &Profile, body: &snapshot::Data, fade_amount: f32) -> f32 {
    let hide = &profile.Hide;
    let pick = |hidden: bool| if hidden { 0.0 } else { 1.0 };
    if hide.bBody {
        0.0
    } else if body.action == ActionType::Swim {
        pick(hide.bSwimming)
    } else if body.action == ActionType::Sneak && body.movement == MovementType::Sprint {
        pick(hide.bSneakRoll)
    } else if body.hands > HandsType::None && body.action == ActionType::Fall {
        pick(hide.bJumpCombat)
    } else if !hide.bWeaponDrawn && body.action == ActionType::Fall {
        pick(hide.bJumping)
    } else if !hide.bWeaponDrawn && body.hands == HandsType::Aim {
        pick(hide.bAiming)
    } else if !hide.bWeaponDrawn && body.hands == HandsType::PowerAttack {
        pick(hide.bPowerAttack)
    } else if body.hands > HandsType::None {
        pick(hide.bWeaponDrawn)
    } else {
        fade_amount
    }
}

/// `DrawArms`: -1 first-person arms, 0 mixed (per arm), 1 third-person arms.
#[must_use]
pub fn draw_arms(general: &General, body: &snapshot::Data, engine_state: Option<CameraState>) -> i32 {
    let Some(state) = engine_state else { return -1 };
    if state as u32 > CameraState::FirstPerson as u32 {
        return 1;
    }
    let sprinting = body.movement == MovementType::Sprint;
    let falling = body.action == ActionType::Fall;
    let swimming = body.action == ActionType::Swim;
    let arms_drawn = body.hands > HandsType::None;
    let aiming = body.hands == HandsType::Aim;
    if body.interaction > InteractionType::None || body.action == ActionType::Parkour {
        return -1;
    }
    if util::is_animation_playing(Idle::ReadElderScroll) || util::is_animation_playing(Idle::DrinkPotion) {
        return -1;
    }
    if !general.bEnableFirstPersonArms {
        return 1;
    }
    if !arms_drawn && (sprinting || falling || swimming) && general.bEnableFirstPersonArmsMovement {
        return 0;
    }
    if arms_drawn && body.left_hand == EquipmentType::Shield {
        return 0;
    }
    if body.left_hand == EquipmentType::Torch {
        return i32::from(body.action == ActionType::Sit);
    }
    let third = |option: bool| if option { 1 } else { -1 };
    match body.right_hand {
        EquipmentType::Bow if aiming => return third(general.bEnableThirdPersonBowAim),
        EquipmentType::Bow if arms_drawn => return third(general.bEnableThirdPersonBow),
        EquipmentType::Crossbow if aiming => return third(general.bEnableThirdPersonCrossbowAim),
        EquipmentType::Crossbow if arms_drawn => return third(general.bEnableThirdPersonCrossbow),
        _ => {}
    }
    if arms_drawn && general.bEnableFirstPersonArms {
        return 0;
    }
    1
}

/// Whether the right arm keeps the first-person arm hidden (`DrawRightarm`); the left adds the
/// shield and torch options (`DrawLeftarm`).
fn draw_arm(general: &General, body: &snapshot::Data) -> bool {
    let moving = body.movement == MovementType::Sprint || matches!(body.action, ActionType::Fall | ActionType::Swim);
    let arms_drawn = body.hands > HandsType::None;
    !(general.bEnableFirstPersonArms && (arms_drawn || (moving && general.bEnableFirstPersonArmsMovement)))
}

#[must_use]
pub fn draw_left_arm(general: &General, body: &snapshot::Data) -> bool {
    let blocking = body.hands == HandsType::Block;
    if body.hands > HandsType::None && body.left_hand == EquipmentType::Shield {
        return if blocking { general.bEnableThirdPersonShieldBlock } else { general.bEnableThirdPersonShield };
    }
    if body.left_hand == EquipmentType::Torch {
        return if blocking { general.bEnableThirdPersonTorchBlock } else { general.bEnableThirdPersonTorch };
    }
    draw_arm(general, body)
}

#[must_use]
pub fn draw_right_arm(general: &General, body: &snapshot::Data) -> bool {
    draw_arm(general, body)
}

/// `DrawHead`: whether the head shows in this camera; `engine_state` is the player camera's
/// current state, `zoom` reads a state's target zoom.
#[allow(clippy::float_cmp)]
#[must_use]
pub fn draw_head(general: &General, hands: HandsType, camera: CameraType, engine_state: Option<CameraState>, zoom: impl Fn(CameraState) -> Option<f32>) -> bool {
    let Some(engine_state) = engine_state else { return true };
    let arms_drawn = hands > HandsType::None;
    let fake_first = |state| zoom(state) == Some(FAKE_FIRST_PERSON_ZOOM);
    match camera {
        CameraType::FirstPerson => {
            if (arms_drawn && general.bEnableHeadCombat) || (!arms_drawn && general.bEnableHead) {
                return true;
            }
        }
        CameraType::ThirdPerson => {
            if fake_first(CameraState::ThirdPerson) {
                return general.bEnableHead;
            }
            if general.bEnableFakeFirstPerson && ((arms_drawn && !general.bEnableHeadCombat) || (!arms_drawn && !general.bEnableHead)) {
                return false;
            }
        }
        CameraType::Animation | CameraType::Crafting | CameraType::Gathering | CameraType::VATS => return !fake_first(CameraState::ThirdPerson),
        CameraType::Bleedout => return !fake_first(CameraState::Bleedout),
        CameraType::Dragon => return !fake_first(CameraState::Dragon) || general.bEnableHeadDragon,
        CameraType::Horse => return !fake_first(CameraState::Horse) || general.bEnableHeadHorse,
        CameraType::Sex => return !fake_first(CameraState::ThirdPerson) || general.bEnableHeadSex,
        CameraType::VampirelordTransition | CameraType::Vampirelord => return general.bEnableHeadVampireLord,
        CameraType::WerewolfTransition | CameraType::Werewolf => return general.bEnableHeadWerewolf,
        _ => {}
    }
    engine_state as u32 > CameraState::FirstPerson as u32
}

#[cfg(test)]
mod tests {
    use super::{body_fade, draw_arms, draw_head, draw_left_arm};
    use crate::body::snapshot::Data;
    use crate::body::state::action::ActionType;
    use crate::body::state::hands::{EquipmentType, HandsType};
    use crate::camera::state::CameraType;
    use crate::settings::{General, Profile};
    use sre::CameraState;

    fn body(hands: HandsType, left: EquipmentType, right: EquipmentType) -> Data {
        Data { hands, left_hand: left, right_hand: right, ..Data::NONE }
    }

    #[test]
    fn arms_follow_ic_rules() {
        let g = General::default();
        let first = Some(CameraState::FirstPerson);
        assert_eq!(draw_arms(&g, &body(HandsType::None, EquipmentType::Fist, EquipmentType::Fist), first), 1);
        assert_eq!(draw_arms(&g, &body(HandsType::Show, EquipmentType::Fist, EquipmentType::Sword), first), 0);
        assert_eq!(draw_arms(&g, &body(HandsType::Show, EquipmentType::Fist, EquipmentType::Bow), first), -1);
        assert_eq!(draw_arms(&g, &body(HandsType::Show, EquipmentType::Shield, EquipmentType::Sword), first), 0);
        assert_eq!(draw_arms(&g, &body(HandsType::Show, EquipmentType::Fist, EquipmentType::Sword), Some(CameraState::ThirdPerson)), 1);
        let blocking = body(HandsType::Block, EquipmentType::Shield, EquipmentType::Sword);
        assert!(!draw_left_arm(&g, &blocking));
        assert!(draw_left_arm(&General { bEnableThirdPersonShieldBlock: true, ..g }, &blocking));
    }

    #[test]
    fn head_hides_in_first_person_unless_enabled() {
        let g = General::default();
        let none = |_| None;
        let first = Some(CameraState::FirstPerson);
        let horse = Some(CameraState::Horse);
        assert!(!draw_head(&g, HandsType::None, CameraType::FirstPerson, first, none));
        assert!(draw_head(&General { bEnableHead: true, ..g }, HandsType::None, CameraType::FirstPerson, first, none));
        assert!(!draw_head(&g, HandsType::None, CameraType::Horse, horse, |_| Some(-0.275)));
        assert!(draw_head(&g, HandsType::None, CameraType::Horse, horse, |_| Some(-0.2)));
        assert!(!draw_head(&g, HandsType::None, CameraType::Werewolf, first, none));
        assert!(draw_head(&g, HandsType::None, CameraType::Unknown, Some(CameraState::ThirdPerson), none));
    }

    #[test]
    fn body_fade_hides_the_states_the_profile_names() {
        let mut p = Profile::default();
        let swimming = Data { action: ActionType::Swim, ..Data::NONE };
        assert!((body_fade(&p, &swimming, 0.7) - 1.0).abs() < f32::EPSILON);
        p.Hide.bSwimming = true;
        assert!(body_fade(&p, &swimming, 0.7).abs() < f32::EPSILON);
        assert!((body_fade(&p, &Data::NONE, 0.7) - 0.7).abs() < f32::EPSILON);
    }
}
