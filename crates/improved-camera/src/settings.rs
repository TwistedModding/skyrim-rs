//! IC's profile (`settings.h`): the options ported so far, with IC's names and defaults, kept in
//! the plugin's settings under `<Section>.<option>` keys (`General.bEnableBody`,
//! `Body.Race.HalfDragon.fForward`, `FOV.fFirstPersonHands`, `Effects.Zoom.fFOV`, `Camera.Dragon.forward`,
//! `Stabilization.Horse.fSmoothingSpeed`, `Advanced.fNearDistanceMin`, `RestrictAngles.fPitchUp`).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock, RwLockReadGuard};

use framework::settings::{Setting, Value};

/// Where profiles are read from and written to: the host's settings, or memory (the importer's
/// staging and the tests).
pub trait Store {
    fn get(&self, key: &str) -> Option<Setting>;
    fn set(&self, key: &str, value: Setting);
    /// Every key starting with `prefix`, in order.
    fn keys(&self, prefix: &str) -> Vec<String>;
    /// Removes every key starting with `prefix`.
    fn remove(&self, prefix: &str) -> usize;

    fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).as_ref().and_then(bool::from_setting)
    }

    fn get_f32(&self, key: &str) -> Option<f32> {
        self.get(key).as_ref().and_then(f32::from_setting)
    }

    fn get_i32(&self, key: &str) -> Option<i32> {
        self.get(key).as_ref().and_then(i32::from_setting)
    }

    fn set_bool(&self, key: &str, value: bool) {
        self.set(key, value.into());
    }

    fn set_f32(&self, key: &str, value: f32) {
        self.set(key, value.into());
    }

    fn set_i32(&self, key: &str, value: i32) {
        self.set(key, value.into());
    }
}

impl Store for framework::settings::Settings {
    fn get(&self, key: &str) -> Option<Setting> {
        self.get::<bool>(key).map(Setting::Bool).or_else(|| self.get::<f64>(key).map(Setting::Number)).or_else(|| self.get::<String>(key).map(Setting::Text))
    }

    fn set(&self, key: &str, value: Setting) {
        Self::set(self, key, value);
    }

    fn keys(&self, prefix: &str) -> Vec<String> {
        Self::keys(self, prefix)
    }

    fn remove(&self, prefix: &str) -> usize {
        Self::remove(self, prefix)
    }
}

/// Settings held in memory.
#[derive(Default)]
pub struct Memory(RefCell<BTreeMap<String, Setting>>);

impl Store for Memory {
    fn get(&self, key: &str) -> Option<Setting> {
        self.0.borrow().get(key).cloned()
    }

    fn set(&self, key: &str, value: Setting) {
        self.0.borrow_mut().insert(key.to_owned(), value);
    }

    fn keys(&self, prefix: &str) -> Vec<String> {
        self.0.borrow().keys().filter(|k| k.starts_with(prefix)).cloned().collect()
    }

    fn remove(&self, prefix: &str) -> usize {
        let mut map = self.0.borrow_mut();
        let before = map.len();
        map.retain(|k, _| !k.starts_with(prefix));
        before - map.len()
    }
}

/// A profile value stored under one key, or under several keys below it.
pub trait Field: Sized {
    /// The value under `key`, `default` where it is not set.
    fn read(store: &impl Store, key: &str, default: Self) -> Self;
    fn write(&self, store: &impl Store, key: &str);
}

impl Field for bool {
    fn read(store: &impl Store, key: &str, default: Self) -> Self {
        store.get_bool(key).unwrap_or(default)
    }

    fn write(&self, store: &impl Store, key: &str) {
        store.set_bool(key, *self);
    }
}

impl Field for f32 {
    fn read(store: &impl Store, key: &str, default: Self) -> Self {
        store.get_f32(key).unwrap_or(default)
    }

    fn write(&self, store: &impl Store, key: &str) {
        store.set_f32(key, *self);
    }
}

impl Field for i32 {
    fn read(store: &impl Store, key: &str, default: Self) -> Self {
        store.get_i32(key).unwrap_or(default)
    }

    fn write(&self, store: &impl Store, key: &str) {
        store.set_i32(key, *self);
    }
}

/// IC's `BodyOffset`: `fForward` is y, `fUp` z, `fRight` x.
#[allow(non_snake_case)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Offset {
    pub fForward: f32,
    pub fUp: f32,
    pub fRight: f32,
}

impl Offset {
    const fn new(forward: f32, up: f32, right: f32) -> Self {
        Self { fForward: forward, fUp: up, fRight: right }
    }

    /// As a point: x right, y forward, z up.
    #[must_use]
    pub const fn point(self) -> [f32; 3] {
        [self.fRight, self.fForward, self.fUp]
    }
}

impl Field for Offset {
    fn read(store: &impl Store, key: &str, default: Self) -> Self {
        let get = |name: &str, default: f32| store.get_f32(&format!("{key}.{name}")).unwrap_or(default);
        Self { fForward: get("fForward", default.fForward), fUp: get("fUp", default.fUp), fRight: get("fRight", default.fRight) }
    }

    fn write(&self, store: &impl Store, key: &str) {
        store.set_f32(&format!("{key}.fForward"), self.fForward);
        store.set_f32(&format!("{key}.fUp"), self.fUp);
        store.set_f32(&format!("{key}.fRight"), self.fRight);
    }
}

/// IC's camera offsets (`ProfileCamera`), lower-case keys: `forward` is y, `up` z, `right` x.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CamOffset {
    pub forward: f32,
    pub up: f32,
    pub right: f32,
}

impl CamOffset {
    const fn new(forward: f32, up: f32, right: f32) -> Self {
        Self { forward, up, right }
    }

    /// As a point: x right, y forward, z up.
    #[must_use]
    pub const fn point(self) -> [f32; 3] {
        [self.right, self.forward, self.up]
    }
}

impl Field for CamOffset {
    fn read(store: &impl Store, key: &str, default: Self) -> Self {
        let get = |name: &str, default: f32| store.get_f32(&format!("{key}.{name}")).unwrap_or(default);
        Self { forward: get("forward", default.forward), up: get("up", default.up), right: get("right", default.right) }
    }

    fn write(&self, store: &impl Store, key: &str) {
        store.set_f32(&format!("{key}.forward"), self.forward);
        store.set_f32(&format!("{key}.up"), self.up);
        store.set_f32(&format!("{key}.right"), self.right);
    }
}

/// `ProfileStabilization::Camera`: how fake first person rides the animated head in one camera.
#[allow(non_snake_case)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stab {
    pub bEnable: bool,
    pub fHeadFollowStrength: f32,
    pub fSmoothingSpeed: f32,
    pub fSmoothingMaxOffset: f32,
}

impl Default for Stab {
    fn default() -> Self {
        Self { bEnable: true, fHeadFollowStrength: 0.35, fSmoothingSpeed: 14.0, fSmoothingMaxOffset: 6.0 }
    }
}

impl Field for Stab {
    fn read(store: &impl Store, key: &str, default: Self) -> Self {
        let get = |name: &str, default: f32| store.get_f32(&format!("{key}.{name}")).unwrap_or(default);
        Self {
            bEnable: store.get_bool(&format!("{key}.bEnable")).unwrap_or(default.bEnable),
            fHeadFollowStrength: get("fHeadFollowStrength", default.fHeadFollowStrength),
            fSmoothingSpeed: get("fSmoothingSpeed", default.fSmoothingSpeed),
            fSmoothingMaxOffset: get("fSmoothingMaxOffset", default.fSmoothingMaxOffset),
        }
    }

    fn write(&self, store: &impl Store, key: &str) {
        store.set_bool(&format!("{key}.bEnable"), self.bEnable);
        store.set_f32(&format!("{key}.fHeadFollowStrength"), self.fHeadFollowStrength);
        store.set_f32(&format!("{key}.fSmoothingSpeed"), self.fSmoothingSpeed);
        store.set_f32(&format!("{key}.fSmoothingMaxOffset"), self.fSmoothingMaxOffset);
    }
}

macro_rules! sections {
    ($($ty:ident = $prefix:literal { $($field:ident: $t:ty = $default:expr, }) => {
        $(
            #[allow(non_snake_case)]
            #[derive(Clone, Copy, Debug, PartialEq)]
            pub struct $ty { $(pub $field: $t, }

            impl Default for $ty {
                fn default() -> Self {
                    Self { $($field: $default, }
                }
            }

            impl $ty {
                fn read(store: &impl Store) -> Self {
                    let d = Self::default();
                    Self { $($field: <$t>::read(store, concat!($prefix, ".", stringify!($field)), d.$field), }
                }

                fn write(&self, store: &impl Store) {
                    $(self.$field.write(store, concat!($prefix, ".", stringify!($field)));
                }
            }
        
    };
}

sections! {
    General = "General" {
        bEnableBody: bool = true,
        bEnableBodyConsole: bool = false,
        bEnableFakeFirstPerson: bool = false,
        bEnableHead: bool = false,
        bEnableHeadCombat: bool = false,
        bEnableHeadHorse: bool = false,
        bEnableHeadDragon: bool = false,
        bEnableHeadVampireLord: bool = false,
        bEnableHeadWerewolf: bool = false,
        bEnableHeadSex: bool = false,
        bEnableHeadSpectator: bool = false,
        bEnableFirstPersonArms: bool = true,
        bEnableFirstPersonArmsMovement: bool = false,
        bEnableThirdPersonTorch: bool = false,
        bEnableThirdPersonTorchBlock: bool = false,
        bEnableThirdPersonShield: bool = false,
        bEnableThirdPersonShieldBlock: bool = false,
        bEnableThirdPersonBow: bool = false,
        bEnableThirdPersonBowAim: bool = false,
        bEnableThirdPersonCrossbow: bool = false,
        bEnableThirdPersonCrossbowAim: bool = false,
    }
    Hide = "Hide" {
        bBow: bool = true,
        bQuiver: bool = true,
        bTwoHanded: bool = true,
        bBody: bool = false,
        bWeaponDrawn: bool = false,
        bAiming: bool = false,
        bPowerAttack: bool = false,
        bJumping: bool = false,
        bJumpCombat: bool = false,
        bSneakRoll: bool = false,
        bSwimming: bool = false,
    }
    Body = "Body" {
        fForward: f32 = 0.0,
        fUp: f32 = 0.0,
        fRight: f32 = 0.0,
        fDirectionalIntensity: f32 = 1.0,
        fDirectionalSpeed: f32 = 8.0,
        fAnchorForward: f32 = 14.0,
        fAnchorUp: f32 = 6.0,
        bYawSmoothing: bool = true,
        fYawSmoothingSpeed: f32 = 10.0,
        fYawSmoothingMaxAngle: f32 = 90.0,
        bFreeLook: bool = true,
        bFreeLookSpine: bool = false,
        fFreeLookMaxAngle: f32 = 80.0,
        fFreeLookSettleAngle: f32 = 35.0,
        fFreeLookCatchupSpeed: f32 = 15.0,
        fFreeLookTwistStrength: f32 = 1.0,
        bDisableTurnAnimation: bool = true,
        Combat: Offset = Offset::default(),
    }
    BodyDirectional = "Body.Directional" {
        Forward: Offset = Offset::default(),
        ForwardRight: Offset = Offset::default(),
        Right: Offset = Offset::default(),
        BackRight: Offset = Offset::default(),
        Back: Offset = Offset::default(),
        BackLeft: Offset = Offset::default(),
        Left: Offset = Offset::default(),
        ForwardLeft: Offset = Offset::default(),
    }
    BodyMovement = "Body.Movement" {
        Idle: Offset = Offset::default(),
        WalkBase: Offset = Offset::default(),
        RunBase: Offset = Offset::default(),
        SprintBase: Offset = Offset::default(),
        DirForward: Offset = Offset::default(),
        DirForwardRight: Offset = Offset::default(),
        DirForwardLeft: Offset = Offset::default(),
        DirBack: Offset = Offset::default(),
        DirBackRight: Offset = Offset::default(),
        DirBackLeft: Offset = Offset::default(),
        DirRight: Offset = Offset::default(),
        DirLeft: Offset = Offset::default(),
        DirDefault: Offset = Offset::default(),
        SprintForward: Offset = Offset::default(),
        SprintForwardRight: Offset = Offset::default(),
        SprintForwardLeft: Offset = Offset::default(),
        SprintBack: Offset = Offset::default(),
        SprintBackRight: Offset = Offset::default(),
        SprintBackLeft: Offset = Offset::default(),
        SprintRight: Offset = Offset::default(),
        SprintLeft: Offset = Offset::default(),
        SprintDefault: Offset = Offset::default(),
        fIdleLerp: f32 = 8.0,
        fWalkLerp: f32 = 8.0,
        fRunLerp: f32 = 12.0,
        fSprintLerp: f32 = 15.0,
        fWalkIntensity: f32 = 0.6,
        fRunIntensity: f32 = 1.2,
        fSprintIntensity: f32 = 1.2,
    }
    BodyHands = "Body.Hands" {
        Sheath: Offset = Offset::default(),
        Show: Offset = Offset::default(),
        Aim: Offset = Offset::default(),
        Attack: Offset = Offset::default(),
        Block: Offset = Offset::default(),
        Cast: Offset = Offset::default(),
        PowerAttack: Offset = Offset::new(-12.6, 0.0, -6.4),
        fLerp: f32 = 12.0,
    }
    BodyAction = "Body.Action" {
        AI: Offset = Offset::default(),
        Animation: Offset = Offset::default(),
        Bathing: Offset = Offset::default(),
        BoatRide: Offset = Offset::default(),
        CartRide: Offset = Offset::default(),
        Craft: Offset = Offset::default(),
        Dialogue: Offset = Offset::default(),
        Fall: Offset = Offset::default(),
        Fish: Offset = Offset::default(),
        Gather: Offset = Offset::default(),
        Giantess: Offset = Offset::default(),
        LichTransition: Offset = Offset::default(),
        Parkour: Offset = Offset::default(),
        Ride: Offset = Offset::default(),
        Sex: Offset = Offset::default(),
        Sit: Offset = Offset::default(),
        Sleep: Offset = Offset::default(),
        Sneak: Offset = Offset::default(),
        Stagger: Offset = Offset::default(),
        Swim: Offset = Offset::default(),
        VampirelordTransition: Offset = Offset::default(),
        View: Offset = Offset::default(),
        WerewolfTransition: Offset = Offset::default(),
        fLerp: f32 = 12.0,
    }
    BodyRace = "Body.Race" {
        Argonian: Offset = Offset::default(),
        ArgonianVampire: Offset = Offset::default(),
        Breton: Offset = Offset::default(),
        BretonVampire: Offset = Offset::default(),
        Darkelf: Offset = Offset::default(),
        DarkelfVampire: Offset = Offset::default(),
        Highelf: Offset = Offset::default(),
        HighelfVampire: Offset = Offset::default(),
        Imperial: Offset = Offset::default(),
        ImperialVampire: Offset = Offset::default(),
        Khajiit: Offset = Offset::default(),
        KhajiitVampire: Offset = Offset::default(),
        Nord: Offset = Offset::default(),
        NordVampire: Offset = Offset::default(),
        Orc: Offset = Offset::default(),
        OrcVampire: Offset = Offset::default(),
        Redguard: Offset = Offset::default(),
        RedguardVampire: Offset = Offset::default(),
        Woodelf: Offset = Offset::default(),
        WoodelfVampire: Offset = Offset::default(),
        Werewolf: Offset = Offset::default(),
        Vampirelord: Offset = Offset::default(),
        Werebear: Offset = Offset::default(),
        Lich: Offset = Offset::default(),
        HalfDragon: Offset = Offset::default(),
        HalfDragonVampire: Offset = Offset::default(),
    }
    Fov = "FOV" {
        fFirstPerson: f32 = 80.0,
        fFirstPersonHands: f32 = 80.0,
        fFirstPersonCombat: f32 = 80.0,
        fFurniture: f32 = 80.0,
        fCrafting: f32 = 80.0,
        fKillmove: f32 = 80.0,
        fRagdoll: f32 = 80.0,
        fDeath: f32 = 80.0,
        fCannibal: f32 = 80.0,
        fHorse: f32 = 80.0,
        fHorseCombat: f32 = 80.0,
        fHorseTransition: f32 = 80.0,
        fDragon: f32 = 80.0,
        fDragonCombat: f32 = 80.0,
        fDragonTransition: f32 = 80.0,
        fVampireLord: f32 = 80.0,
        fWerewolf: f32 = 80.0,
        fLich: f32 = 80.0,
        fSex: f32 = 80.0,
        fThirdPerson: f32 = 80.0,
    }
    EffectsZoom = "Effects.Zoom" {
        fFOV: f32 = -30.0,
    }
    Advanced = "Advanced" {
        bHeadbob: bool = false,
        bHeadbobCombat: bool = false,
        bCameraTransition: bool = true,
        fPlayerDeathReloadTime: f32 = 5.0,
        fForwardBumperOffset: f32 = 45.0,
        fSideBumperOffset: f32 = 15.0,
        fBackBumperOffset: f32 = 15.0,
        fWerewolfBumperOffset: f32 = 15.0,
        fNearDistanceDefault: f32 = 15.0,
        fNearDistanceMax: f32 = 15.0,
        fNearDistanceMin: f32 = 4.0,
        fNearDistanceStartAngle: f32 = -15.0,
        fNearDistanceEndAngle: f32 = -50.0,
        fNearDistanceCombatMax: f32 = 15.0,
        fNearDistanceCombatMin: f32 = 4.0,
        fNearDistanceCombatStartAngle: f32 = -15.0,
        fNearDistanceCombatEndAngle: f32 = -50.0,
    }
    CameraFirstperson = "Camera.Firstperson" {
        forward: f32 = 0.0,
        up: f32 = 0.0,
        right: f32 = 0.0,
        Combat: CamOffset = CamOffset::default(),
    }
    CameraHorse = "Camera.Horse" {
        forward: f32 = 0.0,
        up: f32 = 0.0,
        right: f32 = 0.0,
        Combat: CamOffset = CamOffset::default(),
    }
    Camera = "Camera" {
        Dragon: CamOffset = CamOffset::new(10.0, 0.0, 0.0),
        VampireLord: CamOffset = CamOffset::default(),
        Werewolf: CamOffset = CamOffset::new(-9.0, 0.0, 0.0),
        Lich: CamOffset = CamOffset::new(9.0, 0.0, 0.0),
        Sex: CamOffset = CamOffset::new(-12.0, -5.0, 0.0),
        Spectator: CamOffset = CamOffset::new(5.0, 5.0, 0.0),
    }
    Stabilization = "Stabilization" {
        Horse: Stab = Stab::default(),
        Dragon: Stab = Stab::default(),
        VampireLord: Stab = Stab::default(),
        Werewolf: Stab = Stab::default(),
        Lich: Stab = Stab::default(),
        Sex: Stab = Stab::default(),
        Furniture: Stab = Stab::default(),
        Crafting: Stab = Stab::default(),
        Gathering: Stab = Stab::default(),
        Bleedout: Stab = Stab::default(),
        Transition: Stab = Stab::default(),
    }
    FreeCamera = "FreeCamera" {
        bEnable: bool = true,
    }
    RestrictAngles = "RestrictAngles" {
        fPitchUp: f32 = 89.0,
        fPitchDown: f32 = -89.0,
        fHeadingLeft: f32 = -89.0,
        fHeadingRight: f32 = 89.0,
        fInteractionPitchUp: f32 = 75.0,
        fInteractionPitchDown: f32 = -75.0,
        fInteractionHeadingLeft: f32 = -70.0,
        fInteractionHeadingRight: f32 = 70.0,
    }
    Dialogue = "Dialogue" {
        bEnable: bool = false,
        bEnableFirstPerson: bool = true,
        bEnableThirdPerson: bool = true,
        bForceFirstPerson: bool = true,
        bForceThirdPerson: bool = false,
        bSmoothTransition: bool = true,
        bSmoothCamAPI: bool = true,
        bFaceSpeaker: bool = true,
        bHeadTracking: bool = true,
        bHideDialogueMenu: bool = true,
        bRepositionHUDMessages: bool = true,
        bSwitchTarget: bool = false,
        bLockOn: bool = true,
        bFadeToBlack: bool = false,
        bLetterbox: bool = true,
        fLetterboxRatio: f32 = 0.12,
        fLetterboxTransitionSpeed: f32 = 2.5,
        fPlayerShoulderPosX: f32 = 60.0,
        fPlayerShoulderPosY: f32 = 0.0,
        fPlayerShoulderPosZ: f32 = 0.0,
        fNpcShoulderPosX: f32 = -60.0,
        fNpcShoulderPosY: f32 = 0.0,
        fNpcShoulderPosZ: f32 = 0.0,
        fHumanCamOffsetX: f32 = 0.0,
        fHumanCamOffsetY: f32 = 0.0,
        fHumanCamOffsetZ: f32 = 5.0,
        fCreatureCamOffsetX: f32 = 0.0,
        fCreatureCamOffsetY: f32 = 0.0,
        fCreatureCamOffsetZ: f32 = 15.0,
        fMessagePosX: f32 = -601.0,
        fMessagePosY: f32 = -400.0,
        fCameraSpeed: f32 = 500.0,
        fFadeSpeed: f32 = 4.0,
        fFirstPersonZoom: f32 = 50.0,
        fThirdPersonZoom: f32 = 75.0,
        fDragonZoom: f32 = 125.0,
        fDefaultWorldFOV: f32 = 55.0,
    }
    Vanity = "Vanity" {
        bEnable: bool = false,
        bToggleMenus: bool = true,
        bLetterbox: bool = true,
        bNPCFocus: bool = true,
        bCollisionSmoothing: bool = true,
        fLetterboxRatio: f32 = 0.12,
        fLetterboxTransitionSpeed: f32 = 2.5,
        fIdleDelay: f32 = 120.0,
        fOrbitSpeed: f32 = 10.0,
        fDistance: f32 = 200.0,
        fHeight: f32 = 35.0,
        fFOV: f32 = 65.0,
        iCollisionPullInMethod: i32 = 2,
        fCollisionPullInDuration: f32 = 0.2,
        iCollisionRecoveryMethod: i32 = 15,
        fCollisionRecoveryDuration: f32 = 0.6,
        fFocusRadius: f32 = 1500.0,
        fFocusChance: f32 = 75.0,
        fFocusOffset: f32 = 100.0,
        fFocusDuration: f32 = 6.0,
        fFocusCooldown: f32 = 8.0,
        fFocusBlendTime: f32 = 1.5,
        fFocusFOVOffset: f32 = -10.0,
    }
    EffectsLetterbox = "Effects.Letterbox" {
        bEnable: bool = false,
        bOverlay: bool = false,
    }
    Studio = "Studio" {
        bRevertOnExit: bool = true,
        fNearbyRadius: f32 = 2048.0,
        fUIScale: f32 = 1.35,
        bHideHUD: bool = false,
        bCaptureCropToGuide: bool = false,
        fBracketDelay: f32 = 0.35,
        iGuideGrid: i32 = 0,
        iGuideAspect: i32 = 0,
        bGuideCenter: bool = false,
        bGuideSafeArea: bool = false,
        bGuideHorizon: bool = false,
    }
}

impl CameraFirstperson {
    /// The first-person offset, the combat one with the hands drawn.
    #[must_use]
    pub const fn offset(&self, hands_drawn: bool) -> CamOffset {
        if hands_drawn { self.Combat } else { CamOffset::new(self.forward, self.up, self.right) }
    }
}

impl CameraHorse {
    #[must_use]
    pub const fn offset(&self) -> CamOffset {
        CamOffset::new(self.forward, self.up, self.right)
    }
}

/// The active profile, IC's `Plugin::Profile()`.
#[allow(non_snake_case)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Profile {
    pub General: General,
    pub Hide: Hide,
    pub Body: Body,
    pub Directional: BodyDirectional,
    pub Movement: BodyMovement,
    pub Hands: BodyHands,
    pub Action: BodyAction,
    pub Race: BodyRace,
    pub FOV: Fov,
    pub Zoom: EffectsZoom,
    pub Letterbox: EffectsLetterbox,
    pub Advanced: Advanced,
    pub Firstperson: CameraFirstperson,
    pub Horse: CameraHorse,
    pub Camera: Camera,
    pub Stabilization: Stabilization,
    pub FreeCamera: FreeCamera,
    pub Dialogue: Dialogue,
    pub Vanity: Vanity,
    pub RestrictAngles: RestrictAngles,
    pub Studio: Studio,
}

impl Profile {
    #[must_use]
    pub fn read(store: &impl Store) -> Self {
        Self {
            Firstperson: CameraFirstperson::read(store),
            Horse: CameraHorse::read(store),
            Camera: Camera::read(store),
            Stabilization: Stabilization::read(store),
            FreeCamera: FreeCamera::read(store),
            Dialogue: Dialogue::read(store),
            Vanity: Vanity::read(store),
            RestrictAngles: RestrictAngles::read(store),
            Studio: Studio::read(store),
            General: General::read(store),
            Hide: Hide::read(store),
            Body: Body::read(store),
            Directional: BodyDirectional::read(store),
            Movement: BodyMovement::read(store),
            Hands: BodyHands::read(store),
            Action: BodyAction::read(store),
            Race: BodyRace::read(store),
            FOV: Fov::read(store),
            Zoom: EffectsZoom::read(store),
            Letterbox: EffectsLetterbox::read(store),
            Advanced: Advanced::read(store),
        }
    }

    pub fn write(&self, store: &impl Store) {
        self.General.write(store);
        self.Hide.write(store);
        self.Body.write(store);
        self.Directional.write(store);
        self.Movement.write(store);
        self.Hands.write(store);
        self.Action.write(store);
        self.Race.write(store);
        self.FOV.write(store);
        self.Zoom.write(store);
        self.Letterbox.write(store);
        self.Advanced.write(store);
        self.Firstperson.write(store);
        self.Horse.write(store);
        self.Camera.write(store);
        self.Stabilization.write(store);
        self.FreeCamera.write(store);
        self.Dialogue.write(store);
        self.Vanity.write(store);
        self.RestrictAngles.write(store);
        self.Studio.write(store);
    }
}

static PROFILE: LazyLock<RwLock<Profile>> = LazyLock::new(RwLock::default);

/// The active profile.
pub fn profile() -> RwLockReadGuard<'static, Profile> {
    PROFILE.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Replaces the active profile, as read at load or edited in the tab.
pub fn set_profile(profile: &Profile) {
    *PROFILE.write().unwrap_or_else(std::sync::PoisonError::into_inner) = *profile;
}

#[cfg(test)]
mod tests {
    use super::{Memory as Map, Offset, Profile, Store};

    #[test]
    fn an_empty_store_reads_ic_defaults() {
        let p = Profile::read(&Map::default());
        assert!(p.General.bEnableBody && p.General.bEnableFirstPersonArms && !p.General.bEnableHead);
        assert!(p.Hide.bBow && p.Hide.bQuiver && p.Hide.bTwoHanded && !p.Hide.bBody);
        assert_eq!(p.Hands.PowerAttack, Offset { fForward: -12.6, fUp: 0.0, fRight: -6.4 });
        assert!((p.Movement.fSprintLerp - 15.0).abs() < f32::EPSILON);
        assert!((p.FOV.fFirstPersonHands - 80.0).abs() < f32::EPSILON && (p.FOV.fThirdPerson - 80.0).abs() < f32::EPSILON);
        assert!((p.Zoom.fFOV + 30.0).abs() < f32::EPSILON);
        assert!((p.Advanced.fNearDistanceDefault - 15.0).abs() < f32::EPSILON && (p.Advanced.fNearDistanceCombatMin - 4.0).abs() < f32::EPSILON);
        assert!((p.Advanced.fNearDistanceEndAngle + 50.0).abs() < f32::EPSILON);
    }

    #[test]
    fn keys_follow_ic_names_and_round_trip() {
        let store = Map::default();
        let mut p = Profile::default();
        p.General.bEnableHead = true;
        p.Race.HalfDragon = Offset { fForward: 2.0, fUp: -3.0, fRight: 0.5 };
        p.write(&store);
        assert_eq!(store.get_bool("General.bEnableHead"), Some(true));
        assert_eq!(store.get_f32("Body.Race.HalfDragon.fUp"), Some(-3.0));
        assert_eq!(store.get_f32("Body.Hands.fLerp"), Some(12.0));
        assert_eq!(store.get_f32("FOV.fFirstPersonHands"), Some(80.0));
        assert_eq!(store.get_f32("Effects.Zoom.fFOV"), Some(-30.0));
        assert_eq!(store.get_f32("Advanced.fNearDistanceStartAngle"), Some(-15.0));
        assert_eq!(store.get_bool("Advanced.bCameraTransition"), Some(true));
        assert_eq!(store.get_f32("Camera.Dragon.forward"), Some(10.0));
        assert_eq!(store.get_f32("Camera.Sex.up"), Some(-5.0));
        assert_eq!(store.get_f32("Camera.Firstperson.Combat.right"), Some(0.0));
        assert_eq!(store.get_f32("Camera.Horse.forward"), Some(0.0));
        assert_eq!(store.get_bool("Stabilization.Bleedout.bEnable"), Some(true));
        assert_eq!(store.get_f32("Stabilization.Horse.fHeadFollowStrength"), Some(0.35));
        assert_eq!(store.get_f32("Body.fAnchorForward"), Some(14.0));
        assert_eq!(store.get_bool("FreeCamera.bEnable"), Some(true));
        assert_eq!(store.get_f32("Dialogue.fDefaultWorldFOV"), Some(55.0));
        assert_eq!(store.get_f32("Vanity.fIdleDelay"), Some(120.0));
        assert_eq!(store.get_i32("Vanity.iCollisionRecoveryMethod"), Some(15));
        assert_eq!(store.get_f32("Dialogue.fNpcShoulderPosX"), Some(-60.0));
        assert_eq!(store.get_bool("Body.bFreeLook"), Some(true));
        assert_eq!(store.get_f32("RestrictAngles.fInteractionHeadingLeft"), Some(-70.0));
        assert_eq!(store.get_bool("Studio.bRevertOnExit"), Some(true));
        assert_eq!(store.get_f32("Studio.fBracketDelay"), Some(0.35));
        assert_eq!(store.get_i32("Studio.iGuideAspect"), Some(0));
        assert_eq!(Profile::read(&store), p);
    }

    #[test]
    fn offsets_map_forward_to_y_up_to_z_right_to_x() {
        assert_eq!(Offset { fForward: 1.0, fUp: 2.0, fRight: 3.0 }.point(), [3.0, 1.0, 2.0]);
    }
}
