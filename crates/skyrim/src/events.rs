//! Events (subsystem pass 3): `BSTEventSource<E>` with sink registration, the [`EventSink`] trait a
//! mod implements, the typed sources the engine notifies (the TES holder's 56, the UI's two, the
//! input manager's, the player's, the global footstep source and the 46 story-event
//! singletons) and the event structs pass 3 read on the wire.
//!
//! A sink is a `#[repr(C)]` object whose first word points at a two-slot vtable (destructor,
//! `ProcessEvent`); the engine stores the sink's address and calls slot 1 with the event and the
//! source on the sending thread. [`Sink`] builds that object around any [`EventSink`] implementation;
//! it must live at one address for as long as it is registered, so [`BSTEventSource::add_sink`]
//! takes a `&'static Sink` (a `static`, or `Box::leak`) and returns a [`Registration`] that
//! unregisters on drop or is leaked to stay registered for the life of the game. Adding and
//! removing from inside a handler is safe: the engine queues the change until the walk ends.
//!
//! Every registration function and source accessor is an id of `events.json`; `skyrim::init`
//! checks them all. Event fields rated medium say so in their docs; events whose layout rests on
//! static evidence only carry no accessors (their fields are reachable through `raw()`), and
//! events with no layout at all are opaque types a sink can only count.

use core::ffi::c_void;
use core::marker::PhantomData;
use std::fmt;

use layout::v1_7_104::{classes, fns, globals};

use crate::actors::{PlayerCharacter, RefHandle, TESObjectREFR};
use crate::forms::{BGSLocation, FormId};
pub use crate::string::BSFixedString;
pub use layout::v1_7_104::enums::{BSEventNotifyControl, TESHitEvent_flags as HitFlags};

type GetTesHolder = unsafe extern "C" fn() -> *mut classes::EventSourceHolder_TES;
type GetSource = unsafe extern "C" fn() -> *mut classes::BSTEventSource;
type SinkFn = unsafe extern "C" fn(source: *mut classes::BSTEventSource, sink: *const c_void);

pub(crate) struct Fns {
    tes_holder: GetTesHolder,
    ui: *const *const classes::EventSourceHolder_MenuOpenClose,
    input: *const *const classes::EventSourceHolder_Input,
    footstep: *const *const classes::BSTEventSource,
}

unsafe impl Send for Fns {}
unsafe impl Sync for Fns {}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        for id in TES_IDS.iter().chain(OTHER_IDS).chain(story::IDS) {
            reloc.address(*id)?;
        }
        unsafe {
            Ok(Self {
                tes_holder: crate::resolve(reloc, fns::EventSourceHolder_TES__GetSingleton)?,
                ui: reloc.address(globals::g_EventSourceHolder_MenuOpenClose_ptr)? as *const *const classes::EventSourceHolder_MenuOpenClose,
                input: reloc.address(globals::g_EventSourceHolder_Input_ptr)? as *const *const classes::EventSourceHolder_Input,
                footstep: reloc.address(globals::g_FootstepEventSource_ptr)? as *const *const classes::BSTEventSource,
            })
        }
    }
}

fn fns() -> &'static Fns {
    &crate::runtime().events
}

/// An id `Fns::resolve` has checked, as a function pointer.
fn resolved<F: Copy>(id: u64) -> F {
    unsafe { crate::resolve(crate::reloc(), id) }.expect("checked by skyrim::init")
}

/// An event type with the engine source that sends it.
pub trait Event: Sized + 'static {
    /// Address Library id of this instantiation's `BSTEventSource<E>::AddEventSink`.
    const ADD_SINK: u64;
    /// Address Library id of this instantiation's `BSTEventSource<E>::RemoveEventSink`.
    const REMOVE_SINK: u64;
    /// The source the engine notifies with this event, `None` while its holder does not exist yet.
    fn source() -> Option<&'static BSTEventSource<Self>>;
}

/// What a mod implements to receive events of type `E`; called on the sending thread with the
/// source lock held, so the handler must not block on anything that registers or notifies.
pub trait EventSink<E: Event>: Sync + 'static {
    fn process(&self, event: &E, source: &BSTEventSource<E>) -> BSEventNotifyControl;
}

#[repr(C)]
struct SinkVtable<E: Event, S: EventSink<E>> {
    dtor: unsafe extern "C" fn(this: *mut Sink<E, S>, flags: u32) -> *mut Sink<E, S>,
    process: unsafe extern "C" fn(this: *const Sink<E, S>, event: *const E, source: *mut classes::BSTEventSource) -> BSEventNotifyControl,
}

/// The sink object the engine sees: the vtable pointer, then the handler.
#[repr(C)]
pub struct Sink<E: Event, S: EventSink<E>> {
    vtable: &'static SinkVtable<E, S>,
    handler: S,
}

impl<E: Event, S: EventSink<E>> Sink<E, S> {
    const VTABLE: &'static SinkVtable<E, S> = &SinkVtable { dtor: Self::dtor, process: Self::process };

    #[must_use]
    pub const fn new(handler: S) -> Self {
        Self { vtable: Self::VTABLE, handler }
    }

    #[must_use]
    pub fn handler(&self) -> &S {
        &self.handler
    }

    fn as_ptr(&self) -> *const c_void {
        core::ptr::from_ref(self).cast()
    }

    // The engine never deletes a sink it did not allocate; the slot exists because slot 1 is ProcessEvent.
    unsafe extern "C" fn dtor(this: *mut Self, _flags: u32) -> *mut Self {
        this
    }

    unsafe extern "C" fn process(this: *const Self, event: *const E, source: *mut classes::BSTEventSource) -> BSEventNotifyControl {
        let (Some(this), Some(event), Some(source)) = (unsafe { this.as_ref() }, unsafe { event.as_ref() }, unsafe { source.cast::<BSTEventSource<E>>().as_ref() }) else {
            return BSEventNotifyControl::Continue;
        };
        this.handler.process(event, source)
    }
}

/// A sink's registration with a source; dropping it unregisters, [`leak`](Self::leak) keeps it.
#[must_use = "dropping a Registration unregisters the sink"]
pub struct Registration<E: Event> {
    source: &'static BSTEventSource<E>,
    sink: *const c_void,
}

unsafe impl<E: Event> Send for Registration<E> {}
unsafe impl<E: Event> Sync for Registration<E> {}

impl<E: Event> Registration<E> {
    /// Leaves the sink registered for the life of the game.
    pub fn leak(self) {
        core::mem::forget(self);
    }

    #[must_use]
    pub fn source(&self) -> &'static BSTEventSource<E> {
        self.source
    }
}

impl<E: Event> Drop for Registration<E> {
    fn drop(&mut self) {
        unsafe { resolved::<SinkFn>(E::REMOVE_SINK)(self.source.as_mut_ptr(), self.sink) };
    }
}

/// A `BSTEventSource<E>`: the registered sinks, the deferred changes while a notification runs,
/// its spin lock and the notifying flag.
#[repr(transparent)]
pub struct BSTEventSource<E: Event>(classes::BSTEventSource, PhantomData<E>);

impl<E: Event> BSTEventSource<E> {
    /// # Safety
    /// `p` must point to a live `BSTEventSource<E>` that is never freed.
    #[must_use]
    pub unsafe fn from_ptr(p: *const classes::BSTEventSource) -> Option<&'static Self> {
        unsafe { p.cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::BSTEventSource {
        &self.0
    }

    fn as_mut_ptr(&self) -> *mut classes::BSTEventSource {
        (&raw const self.0).cast_mut()
    }

    /// The registered sink objects, read without the lock.
    #[must_use]
    pub fn sinks(&self) -> &[*const c_void] {
        if self.0.sinks.is_null() {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(self.0.sinks.cast::<*const c_void>(), self.0.sinkCount as usize) }
    }

    #[must_use]
    pub fn sink_count(&self) -> u32 {
        self.0.sinkCount
    }

    /// Set while a notification walks the sinks.
    #[must_use]
    pub fn is_notifying(&self) -> bool {
        self.0.notifying != 0
    }

    /// Registers `sink` through `BSTEventSource<E>::AddEventSink`; a sink already registered is
    /// not added again. The engine keeps the pointer, so the sink is `'static` and must never move.
    pub fn add_sink<S: EventSink<E>>(&'static self, sink: &'static Sink<E, S>) -> Registration<E> {
        unsafe { resolved::<SinkFn>(E::ADD_SINK)(self.as_mut_ptr(), sink.as_ptr()) };
        Registration { source: self, sink: sink.as_ptr() }
    }

    /// Registers `sink` ahead of every sink already registered. A notification walks the sinks in
    /// order and ends at the first that returns [`BSEventNotifyControl::Stop`], so this sink sees each
    /// event first and can hide it from all the others (pass 9 on the input source). Takes the
    /// source's lock, so it waits for a running notification and must not be called from a sink
    /// of this source.
    pub fn add_sink_first<S: EventSink<E>>(&'static self, sink: &'static Sink<E, S>) -> Registration<E> {
        let _lock = unsafe { crate::SpinGuard::lock(&raw const self.0.lock) };
        let registration = self.add_sink(sink);
        if !self.0.sinks.is_null() {
            let sinks = unsafe { core::slice::from_raw_parts_mut(self.0.sinks.cast::<*const c_void>(), self.0.sinkCount as usize) };
            if let Some(i) = sinks.iter().position(|s| *s == sink.as_ptr()) {
                sinks[..=i].rotate_right(1);
            }
        }
        registration
    }

    #[must_use]
    pub fn contains<S: EventSink<E>>(&self, sink: &Sink<E, S>) -> bool {
        self.sinks().contains(&sink.as_ptr())
    }
}

/// # Safety
/// `p` must point to a `BSTEventSource<E>` inside an object that lives for the rest of the game.
unsafe fn source_at<E: Event>(p: *const classes::BSTEventSource) -> Option<&'static BSTEventSource<E>> {
    unsafe { BSTEventSource::from_ptr(p) }
}

/// The holder of the 56 script event sources, the static object
/// `g_EventSourceHolder_TES` (id 400420).
#[repr(transparent)]
pub struct EventSourceHolderTes(classes::EventSourceHolder_TES);

impl EventSourceHolderTes {
    /// Through `EventSourceHolder_TES::GetSingleton` (id 14298), which constructs the holder on first use.
    #[must_use]
    pub fn get() -> &'static Self {
        unsafe { &*(fns().tes_holder)().cast::<Self>() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::EventSourceHolder_TES {
        &self.0
    }

    /// Source `index` of the 56, at stride 0x58.
    #[must_use]
    pub fn source_at(&self, index: usize) -> Option<&classes::BSTEventSource> {
        (index < 56).then(|| unsafe { &*(&raw const self.0).cast::<classes::BSTEventSource>().add(index) })
    }
}

/// The UI singleton behind `g_EventSourceHolder_MenuOpenClose_ptr` (id 400327); `skyrim::ui` adds the menu table, stack and queue.
#[repr(transparent)]
pub struct EventSourceHolderMenuOpenClose(classes::EventSourceHolder_MenuOpenClose);

impl EventSourceHolderMenuOpenClose {
    /// `None` until the UI is constructed, which is before the main menu.
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().ui).cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::EventSourceHolder_MenuOpenClose {
        &self.0
    }

    /// Number of open menus that pause the game (medium): its 0/non-0 transitions send `MenuModeChangeEvent`.
    #[must_use]
    pub fn num_pauses_game(&self) -> u32 {
        self.0.numPausesGame
    }
}

/// The input device manager behind `g_EventSourceHolder_Input_ptr` (id 402776).
#[repr(transparent)]
pub struct EventSourceHolderInput(classes::EventSourceHolder_Input);

impl EventSourceHolderInput {
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().input).cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::EventSourceHolder_Input {
        &self.0
    }
}

macro_rules! event_structs {
    ($($(#[$doc:meta] $name:ident;) => {
        $(
            $(#[$doc]
            #[repr(transparent)]
            pub struct $name(classes::$name);

            impl $name {
                #[must_use]
                pub fn raw(&self) -> &classes::$name {
                    &self.0
                }
            }
        
    };
}

macro_rules! opaque_events {
    ($($(#[$doc:meta] $name:ident;) => {
        $(
            $(#[$doc]
            #[repr(C)]
            pub struct $name {
                _payload: [u8; 0],
            }
        
    };
}

/// `impl Event` rows: the type, how to reach its source, and its `AddEventSink`/`RemoveEventSink` ids.
macro_rules! sources {
    ($ids:ident; $($event:ty => $source:expr, $add:ident, $remove:ident;) => {
        $(
            impl Event for $event {
                const ADD_SINK: u64 = fns::$add;
                const REMOVE_SINK: u64 = fns::$remove;
                fn source() -> Option<&'static BSTEventSource<Self>> {
                    unsafe { source_at($source) }
                }
            }
        
        const $ids: &[u64] = &[$(fns::$add, fns::$remove),*];
    };
}

event_structs! {
    /// A menu was shown or hidden; sent by the UI's `ProcessMessages`.
    MenuOpenCloseEvent;
    /// The game entered or left menu mode: the first pausing menu opened or the last one closed.
    MenuModeChangeEvent;
    /// An actor equipped or unequipped an item.
    TESEquipEvent;
    /// An item moved between containers.
    TESContainerChangedEvent;
    /// A reference was activated.
    TESActivateEvent;
    /// A reference was hit by a weapon, spell or explosion.
    TESHitEvent;
    /// An actor's combat state changed.
    TESCombatEvent;
    /// A reference was attached to or detached from a loaded cell.
    TESCellAttachDetachEvent;
    /// A form's 3D was loaded or unloaded.
    TESObjectLoadedEvent;
    /// A save game finished loading; the event carries nothing.
    TESLoadGameEvent;
    /// A reference's scripts initialised.
    TESInitScriptEvent;
    /// An actor sat down on or got up from furniture.
    TESFurnitureEvent;
    /// An actor's AI package started, ended or changed.
    TESPackageEvent;
    /// An actor changed location.
    TESActorLocationChangeEvent;
    /// A lock's state changed.
    TESLockChangedEvent;
    /// A behaviour graph raised an animation event. Its sources are the per-graph ones
    /// (`BShkbAnimationGraph` + 0x68): `source()` is `None`, register on
    /// [`crate::animation::graph_event_source`] or [`crate::animation::AnimationGraph::event_source`].
    BSAnimationGraphEvent;
    /// A footstep: an actor's graph event whose tag is in the footstep source's filter list,
    /// re-sent by `Actor::ProcessAnimationGraphEvent` through the global source behind
    /// `g_FootstepEventSource_ptr` (`BGSImpactManager` is the engine's sink).
    BGSFootstepEvent;
    /// The player entered or left a cell (medium: read live twice during a load).
    BGSActorCellEvent;
    /// Static evidence only (`Actor::vfunc_270`, which stores `dead = 0`): `actorDying`, `actorKiller`, `dead`.
    TESDeathEvent;
    /// Static evidence only (`MagicTarget::vfunc_1`): `target`, `caster`, `magicEffect`.
    TESMagicEffectApplyEvent;
    /// Static evidence only (`ActorMagicCaster::vfunc_9`): `object`, `spell`.
    TESSpellCastEvent;
    /// Static evidence only: `sleepStartTime`, `desiredSleepEndTime`.
    TESSleepStartEvent;
    /// Static evidence only: `interrupted`.
    TESSleepStopEvent;
    /// Static evidence only (`PlayerCharacter::vfunc_186`): `waitStartTime`.
    TESWaitStartEvent;
    /// Static evidence only: `interrupted`.
    TESWaitStopEvent;
    /// Static evidence only: `fastTravelEndHours`.
    TESFastTravelEndEvent;
    /// Static evidence only (no sender found): `target`, `caster`, `activeEffectUniqueID`, `isApplied`.
    TESActiveEffectApplyRemoveEvent;
    /// Static evidence only: `reader`, `baseObject`, `uniqueID`.
    TESBookReadEvent;
    /// Static evidence only; its `Notify` never fired in the sessions.
    TESCellFullyLoadedEvent;
    /// Static evidence only: `target`, `oldStage`, `newStage`.
    TESDestructionStageChangedEvent;
    /// Static evidence only: one reference.
    TESEnterBleedoutEvent;
    /// One capture; static: `TESObjectREFR::dtor` stores `formID`.
    TESFormDeleteEvent;
    /// Static evidence only: `reference`, flag.
    TESGrabReleaseEvent;
    /// Static evidence only: `target`, `caster`, `spell`, `status`.
    TESMagicWardHitEvent;
    /// Static evidence only: `reference`, flag.
    TESMoveAttachDetachEvent;
    /// One capture each (`ref`, `type` 0 failed, 1 complete, 2 almost complete).
    TESObjectREFRTranslationEvent;
    /// Static evidence only, no sender found: `activeRef`, `ref`, `opened`.
    TESOpenCloseEvent;
    /// Partial, static, low.
    TESPerkEntryRunEvent;
    /// Static evidence only: `weapon`, `ammo`, `power`, `isSunGazing`.
    TESPlayerBowShotEvent;
    /// Static evidence only: `finishedCallback`, `formID`, `stage`, `flags`.
    TESQuestStageEvent;
    /// Partial, static, low.
    TESQuestStageItemDoneEvent;
    /// One capture; static: `formID`, `started`.
    TESQuestStartStopEvent;
    /// Static evidence only: one reference.
    TESResetEvent;
    /// Static evidence only: `formID`.
    TESResolveNPCTemplatesEvent;
    /// Partial, static, low.
    TESSceneEvent;
    /// Static evidence only: `seller`, `baseObject`, `uniqueID`.
    TESSellEvent;
    /// Static evidence only: one reference.
    TESSwitchRaceCompleteEvent;
    /// Partial, static, low.
    TESTopicInfoEvent;
    /// Static evidence only; no `Notify` instance found for its source.
    TESTrackedStatsEvent;
    /// Partial, static, low.
    TESTrapHitEvent;
    /// Static evidence only (`TESTrapListener`): `target`, `caster`.
    TESTriggerEvent;
    /// Static evidence only: `oldBaseID`, `newBaseID`, `objectID`, `oldUniqueID`, `newUniqueID`.
    TESUniqueIDChangeEvent;
    /// One capture; the VM reacts to `type` 4 (medium).
    PositionPlayerEvent;
}

opaque_events! {
    /// No layout in pass 3; a sink can only count these.
    BGSEventProcessedEvent;
    /// No layout in pass 3; a sink can only count these.
    TESCellReadyToApplyDecalsEvent;
    /// No layout in pass 3; a sink can only count these.
    TESQuestInitEvent;
    /// No layout in pass 3; a sink can only count these.
    TESSceneActionEvent;
    /// No layout in pass 3; a sink can only count these.
    TESScenePhaseEvent;
    /// No layout in pass 3; a sink can only count these.
    TESTriggerEnterEvent;
    /// No layout in pass 3; a sink can only count these.
    TESTriggerLeaveEvent;
    /// No layout in pass 3; a sink can only count these.
    TESAmiiboTouchEvent;
    /// No layout in pass 3; a sink can only count these.
    TESAmiiboForcedStopDetectionEvent;
}

macro_rules! tes {
    ($field:ident) => {
        &raw const EventSourceHolderTes::get().0.$field
    };
}

sources! { TES_IDS;
    BGSEventProcessedEvent => tes!(source_BGSEventProcessedEvent), BSTEventSource_BGSEventProcessedEvent__AddEventSink, BSTEventSource_BGSEventProcessedEvent__RemoveEventSink;
    TESActivateEvent => tes!(source_TESActivateEvent), BSTEventSource_TESActivateEvent__AddEventSink, BSTEventSource_TESActivateEvent__RemoveEventSink;
    TESActiveEffectApplyRemoveEvent => tes!(source_TESActiveEffectApplyRemoveEvent), BSTEventSource_TESActiveEffectApplyRemoveEvent__AddEventSink, BSTEventSource_TESActiveEffectApplyRemoveEvent__RemoveEventSink;
    TESActorLocationChangeEvent => tes!(source_TESActorLocationChangeEvent), BSTEventSource_TESActorLocationChangeEvent__AddEventSink, BSTEventSource_TESActorLocationChangeEvent__RemoveEventSink;
    TESBookReadEvent => tes!(source_TESBookReadEvent), BSTEventSource_TESBookReadEvent__AddEventSink, BSTEventSource_TESBookReadEvent__RemoveEventSink;
    TESCellAttachDetachEvent => tes!(source_TESCellAttachDetachEvent), BSTEventSource_TESCellAttachDetachEvent__AddEventSink, BSTEventSource_TESCellAttachDetachEvent__RemoveEventSink;
    TESCellFullyLoadedEvent => tes!(source_TESCellFullyLoadedEvent), BSTEventSource_TESCellFullyLoadedEvent__AddEventSink, BSTEventSource_TESCellFullyLoadedEvent__RemoveEventSink;
    TESCellReadyToApplyDecalsEvent => tes!(source_TESCellReadyToApplyDecalsEvent), BSTEventSource_TESCellReadyToApplyDecalsEvent__AddEventSink, BSTEventSource_TESCellReadyToApplyDecalsEvent__RemoveEventSink;
    TESCombatEvent => tes!(source_TESCombatEvent), BSTEventSource_TESCombatEvent__AddEventSink, BSTEventSource_TESCombatEvent__RemoveEventSink;
    TESContainerChangedEvent => tes!(source_TESContainerChangedEvent), BSTEventSource_TESContainerChangedEvent__AddEventSink, BSTEventSource_TESContainerChangedEvent__RemoveEventSink;
    TESDeathEvent => tes!(source_TESDeathEvent), BSTEventSource_TESDeathEvent__AddEventSink, BSTEventSource_TESDeathEvent__RemoveEventSink;
    TESDestructionStageChangedEvent => tes!(source_TESDestructionStageChangedEvent), BSTEventSource_TESDestructionStageChangedEvent__AddEventSink, BSTEventSource_TESDestructionStageChangedEvent__RemoveEventSink;
    TESEnterBleedoutEvent => tes!(source_TESEnterBleedoutEvent), BSTEventSource_TESEnterBleedoutEvent__AddEventSink, BSTEventSource_TESEnterBleedoutEvent__RemoveEventSink;
    TESEquipEvent => tes!(source_TESEquipEvent), BSTEventSource_TESEquipEvent__AddEventSink, BSTEventSource_TESEquipEvent__RemoveEventSink;
    TESFormDeleteEvent => tes!(source_TESFormDeleteEvent), BSTEventSource_TESFormDeleteEvent__AddEventSink, BSTEventSource_TESFormDeleteEvent__RemoveEventSink;
    TESFurnitureEvent => tes!(source_TESFurnitureEvent), BSTEventSource_TESFurnitureEvent__AddEventSink, BSTEventSource_TESFurnitureEvent__RemoveEventSink;
    TESGrabReleaseEvent => tes!(source_TESGrabReleaseEvent), BSTEventSource_TESGrabReleaseEvent__AddEventSink, BSTEventSource_TESGrabReleaseEvent__RemoveEventSink;
    TESHitEvent => tes!(source_TESHitEvent), BSTEventSource_TESHitEvent__AddEventSink, BSTEventSource_TESHitEvent__RemoveEventSink;
    TESInitScriptEvent => tes!(source_TESInitScriptEvent), BSTEventSource_TESInitScriptEvent__AddEventSink, BSTEventSource_TESInitScriptEvent__RemoveEventSink;
    TESLoadGameEvent => tes!(source_TESLoadGameEvent), BSTEventSource_TESLoadGameEvent__AddEventSink, BSTEventSource_TESLoadGameEvent__RemoveEventSink;
    TESLockChangedEvent => tes!(source_TESLockChangedEvent), BSTEventSource_TESLockChangedEvent__AddEventSink, BSTEventSource_TESLockChangedEvent__RemoveEventSink;
    TESMagicEffectApplyEvent => tes!(source_TESMagicEffectApplyEvent), BSTEventSource_TESMagicEffectApplyEvent__AddEventSink, BSTEventSource_TESMagicEffectApplyEvent__RemoveEventSink;
    TESMagicWardHitEvent => tes!(source_TESMagicWardHitEvent), BSTEventSource_TESMagicWardHitEvent__AddEventSink, BSTEventSource_TESMagicWardHitEvent__RemoveEventSink;
    TESMoveAttachDetachEvent => tes!(source_TESMoveAttachDetachEvent), BSTEventSource_TESMoveAttachDetachEvent__AddEventSink, BSTEventSource_TESMoveAttachDetachEvent__RemoveEventSink;
    TESObjectLoadedEvent => tes!(source_TESObjectLoadedEvent), BSTEventSource_TESObjectLoadedEvent__AddEventSink, BSTEventSource_TESObjectLoadedEvent__RemoveEventSink;
    TESObjectREFRTranslationEvent => tes!(source_TESObjectREFRTranslationEvent), BSTEventSource_TESObjectREFRTranslationEvent__AddEventSink, BSTEventSource_TESObjectREFRTranslationEvent__RemoveEventSink;
    TESOpenCloseEvent => tes!(source_TESOpenCloseEvent), BSTEventSource_TESOpenCloseEvent__AddEventSink, BSTEventSource_TESOpenCloseEvent__RemoveEventSink;
    TESPackageEvent => tes!(source_TESPackageEvent), BSTEventSource_TESPackageEvent__AddEventSink, BSTEventSource_TESPackageEvent__RemoveEventSink;
    TESPerkEntryRunEvent => tes!(source_TESPerkEntryRunEvent), BSTEventSource_TESPerkEntryRunEvent__AddEventSink, BSTEventSource_TESPerkEntryRunEvent__RemoveEventSink;
    TESQuestInitEvent => tes!(source_TESQuestInitEvent), BSTEventSource_TESQuestInitEvent__AddEventSink, BSTEventSource_TESQuestInitEvent__RemoveEventSink;
    TESQuestStageEvent => tes!(source_TESQuestStageEvent), BSTEventSource_TESQuestStageEvent__AddEventSink, BSTEventSource_TESQuestStageEvent__RemoveEventSink;
    TESQuestStageItemDoneEvent => tes!(source_TESQuestStageItemDoneEvent), BSTEventSource_TESQuestStageItemDoneEvent__AddEventSink, BSTEventSource_TESQuestStageItemDoneEvent__RemoveEventSink;
    TESQuestStartStopEvent => tes!(source_TESQuestStartStopEvent), BSTEventSource_TESQuestStartStopEvent__AddEventSink, BSTEventSource_TESQuestStartStopEvent__RemoveEventSink;
    TESResetEvent => tes!(source_TESResetEvent), BSTEventSource_TESResetEvent__AddEventSink, BSTEventSource_TESResetEvent__RemoveEventSink;
    TESResolveNPCTemplatesEvent => tes!(source_TESResolveNPCTemplatesEvent), BSTEventSource_TESResolveNPCTemplatesEvent__AddEventSink, BSTEventSource_TESResolveNPCTemplatesEvent__RemoveEventSink;
    TESSceneEvent => tes!(source_TESSceneEvent), BSTEventSource_TESSceneEvent__AddEventSink, BSTEventSource_TESSceneEvent__RemoveEventSink;
    TESSceneActionEvent => tes!(source_TESSceneActionEvent), BSTEventSource_TESSceneActionEvent__AddEventSink, BSTEventSource_TESSceneActionEvent__RemoveEventSink;
    TESScenePhaseEvent => tes!(source_TESScenePhaseEvent), BSTEventSource_TESScenePhaseEvent__AddEventSink, BSTEventSource_TESScenePhaseEvent__RemoveEventSink;
    TESSellEvent => tes!(source_TESSellEvent), BSTEventSource_TESSellEvent__AddEventSink, BSTEventSource_TESSellEvent__RemoveEventSink;
    TESSleepStartEvent => tes!(source_TESSleepStartEvent), BSTEventSource_TESSleepStartEvent__AddEventSink, BSTEventSource_TESSleepStartEvent__RemoveEventSink;
    TESSleepStopEvent => tes!(source_TESSleepStopEvent), BSTEventSource_TESSleepStopEvent__AddEventSink, BSTEventSource_TESSleepStopEvent__RemoveEventSink;
    TESSpellCastEvent => tes!(source_TESSpellCastEvent), BSTEventSource_TESSpellCastEvent__AddEventSink, BSTEventSource_TESSpellCastEvent__RemoveEventSink;
    TESPlayerBowShotEvent => tes!(source_TESPlayerBowShotEvent), BSTEventSource_TESPlayerBowShotEvent__AddEventSink, BSTEventSource_TESPlayerBowShotEvent__RemoveEventSink;
    TESTopicInfoEvent => tes!(source_TESTopicInfoEvent), BSTEventSource_TESTopicInfoEvent__AddEventSink, BSTEventSource_TESTopicInfoEvent__RemoveEventSink;
    TESTrackedStatsEvent => tes!(source_TESTrackedStatsEvent), BSTEventSource_TESTrackedStatsEvent__AddEventSink, BSTEventSource_TESTrackedStatsEvent__RemoveEventSink;
    TESTrapHitEvent => tes!(source_TESTrapHitEvent), BSTEventSource_TESTrapHitEvent__AddEventSink, BSTEventSource_TESTrapHitEvent__RemoveEventSink;
    TESTriggerEvent => tes!(source_TESTriggerEvent), BSTEventSource_TESTriggerEvent__AddEventSink, BSTEventSource_TESTriggerEvent__RemoveEventSink;
    TESTriggerEnterEvent => tes!(source_TESTriggerEnterEvent), BSTEventSource_TESTriggerEnterEvent__AddEventSink, BSTEventSource_TESTriggerEnterEvent__RemoveEventSink;
    TESTriggerLeaveEvent => tes!(source_TESTriggerLeaveEvent), BSTEventSource_TESTriggerLeaveEvent__AddEventSink, BSTEventSource_TESTriggerLeaveEvent__RemoveEventSink;
    TESUniqueIDChangeEvent => tes!(source_TESUniqueIDChangeEvent), BSTEventSource_TESUniqueIDChangeEvent__AddEventSink, BSTEventSource_TESUniqueIDChangeEvent__RemoveEventSink;
    // The four sources below have no named RemoveEventSink instance; they use the template body
    // events.md documents (0x3cd9e0), which every instance shares instruction for instruction.
    TESWaitStartEvent => tes!(source_TESWaitStartEvent), BSTEventSource_TESWaitStartEvent__AddEventSink, BSTEventSource_MenuOpenCloseEvent__RemoveEventSink;
    TESWaitStopEvent => tes!(source_TESWaitStopEvent), BSTEventSource_TESWaitStopEvent__AddEventSink, BSTEventSource_MenuOpenCloseEvent__RemoveEventSink;
    TESSwitchRaceCompleteEvent => tes!(source_TESSwitchRaceCompleteEvent), BSTEventSource_TESSwitchRaceCompleteEvent__AddEventSink, BSTEventSource_TESSwitchRaceCompleteEvent__RemoveEventSink;
    TESFastTravelEndEvent => tes!(source_TESFastTravelEndEvent), BSTEventSource_TESFastTravelEndEvent__AddEventSink, BSTEventSource_TESFastTravelEndEvent__RemoveEventSink;
    TESAmiiboTouchEvent => tes!(source_TESAmiiboTouchEvent), BSTEventSource_TESAmiiboTouchEvent__AddEventSink, BSTEventSource_MenuOpenCloseEvent__RemoveEventSink;
    TESAmiiboForcedStopDetectionEvent => tes!(source_TESAmiiboForcedStopDetectionEvent), BSTEventSource_TESAmiiboForcedStopDetectionEvent__AddEventSink, BSTEventSource_MenuOpenCloseEvent__RemoveEventSink;
}

/// The input source sends a pointer to the head of the frame's `InputEvent` list (pass 9
/// describes the events; `device`, `eventType` and `next` are medium here).
#[repr(transparent)]
pub struct InputEventList(*mut classes::InputEvent);

impl InputEventList {
    pub fn iter(&self) -> impl Iterator<Item = &InputEvent> {
        let mut next = self.first();
        core::iter::from_fn(move || {
            let e = next?;
            next = e.next();
            Some(e)
        })
    }
}

impl InputEventList {
    /// The first event of the frame, `None` for an empty list.
    #[must_use]
    pub fn first(&self) -> Option<&InputEvent> {
        unsafe { self.0.cast::<InputEvent>().as_ref() }
    }
}

impl fmt::Debug for InputEventList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InputEventList {{ head: {:p}, first: {:?} }}", self.0, self.first().map(|e| (e.device(), e.event_type())))
    }
}

/// One event of the frame's input list; the typed events (`ButtonEvent`, ...) are pass 9's.
#[repr(transparent)]
pub struct InputEvent(classes::InputEvent);

impl InputEvent {
    #[must_use]
    pub fn raw(&self) -> &classes::InputEvent {
        &self.0
    }

    /// The `INPUT_DEVICE` (medium).
    #[must_use]
    pub fn device(&self) -> u32 {
        self.0.device
    }

    /// The `INPUT_EVENT_TYPE` (medium).
    #[must_use]
    pub fn event_type(&self) -> u32 {
        self.0.eventType
    }

    #[must_use]
    pub fn next(&self) -> Option<&Self> {
        unsafe { self.0.next.cast::<Self>().as_ref() }
    }
}

fn ui_source(offset: fn(&classes::EventSourceHolder_MenuOpenClose) -> *const classes::BSTEventSource) -> *const classes::BSTEventSource {
    EventSourceHolderMenuOpenClose::get().map_or(core::ptr::null(), |ui| offset(&ui.0))
}

fn player_source(offset: fn(*const classes::PlayerCharacter) -> *const classes::BSTEventSource) -> *const classes::BSTEventSource {
    PlayerCharacter::get().map_or(core::ptr::null(), |p| offset(p.as_ptr()))
}

sources! { OTHER_IDS;
    MenuOpenCloseEvent => ui_source(|ui| &raw const ui.source_MenuOpenCloseEvent), BSTEventSource_MenuOpenCloseEvent__AddEventSink, BSTEventSource_MenuOpenCloseEvent__RemoveEventSink;
    MenuModeChangeEvent => ui_source(|ui| &raw const ui.source_MenuModeChangeEvent), BSTEventSource_MenuModeChangeEvent__AddEventSink, BSTEventSource_MenuModeChangeEvent__RemoveEventSink;
    InputEventList => EventSourceHolderInput::get().map_or(core::ptr::null(), |i| &raw const i.0.source_InputEvent), BSTEventSource_InputEvent____ptr64__AddEventSink, BSTEventSource_InputEvent____ptr64__RemoveEventSink;
    BGSFootstepEvent => *fns().footstep, BSTEventSource_BGSFootstepEvent__AddEventSink, BSTEventSource_BGSFootstepEvent__RemoveEventSink;
    BSAnimationGraphEvent => core::ptr::null(), BSTEventSource_BSAnimationGraphEvent__AddEventSink, BSTEventSource_BSAnimationGraphEvent__RemoveEventSink;
    BGSActorCellEvent => player_source(|p| &raw const (*p).source_BGSActorCellEvent), BSTEventSource_BGSActorCellEvent__AddEventSink, BSTEventSource_BGSActorCellEvent__RemoveEventSink;
    PositionPlayerEvent => player_source(|p| &raw const (*p).source_PositionPlayerEvent), BSTEventSource_PositionPlayerEvent__AddEventSink, BSTEventSource_PositionPlayerEvent__RemoveEventSink;
}

/// The 46 story-event singletons (`ActorKill`, `Bounty`, `SkillIncrease`, ...), each one source
/// reached through `<Name>::GetEventSource`, a guarded function-local static. Rated medium, from
/// the registrations of their `BSTEventSink` classes; pass 3 did not read their payloads, so the
/// event types are opaque.
pub mod story {
    use super::{BSTEventSource, Event, GetSource, resolved, source_at};
    use layout::v1_7_104::fns;

    macro_rules! story_events {
        ($($name:ident: $get:ident, $add:ident, $remove:ident;) => {
            $(
                /// Payload not described by pass 3.
                #[repr(C)]
                pub struct $name {
                    _payload: [u8; 0],
                }

                impl Event for $name {
                    const ADD_SINK: u64 = fns::$add;
                    const REMOVE_SINK: u64 = fns::$remove;
                    fn source() -> Option<&'static BSTEventSource<Self>> {
                        unsafe { source_at(resolved::<GetSource>(fns::$get)()) }
                    }
                }
            
            pub(super) const IDS: &[u64] = &[$(fns::$get, fns::$add, fns::$remove),*];
        };
    }

    story_events! {
        ActorItemEquipped: ActorItemEquipped__GetEventSource, BSTEventSource_ActorItemEquipped__Event__AddEventSink, BSTEventSource_ActorItemEquipped__Event__RemoveEventSink;
        ActorKill: ActorKill__GetEventSource, BSTEventSource_ActorKill__Event__AddEventSink, BSTEventSource_ActorKill__Event__RemoveEventSink;
        AssaultCrime: AssaultCrime__GetEventSource, BSTEventSource_AssaultCrime__Event__AddEventSink, BSTEventSource_AssaultCrime__Event__RemoveEventSink;
        BooksRead: BooksRead__GetEventSource, BSTEventSource_BooksRead__Event__AddEventSink, BSTEventSource_BooksRead__Event__RemoveEventSink;
        Bounty: Bounty__GetEventSource, BSTEventSource_Bounty__Event__AddEventSink, BSTEventSource_Bounty__Event__RemoveEventSink;
        ChestsLooted: ChestsLooted__GetEventSource, BSTEventSource_ChestsLooted__Event__AddEventSink, BSTEventSource_ChestsLooted__Event__RemoveEventSink;
        ContractedDisease: ContractedDisease__GetEventSource, BSTEventSource_ContractedDisease__Event__AddEventSink, BSTEventSource_ContractedDisease__Event__RemoveEventSink;
        CriticalHitEvent: CriticalHitEvent__GetEventSource, BSTEventSource_CriticalHitEvent__Event__AddEventSink, BSTEventSource_CriticalHitEvent__Event__RemoveEventSink;
        DaysJailed: DaysJailed__GetEventSource, BSTEventSource_DaysJailed__Event__AddEventSink, BSTEventSource_DaysJailed__Event__RemoveEventSink;
        DaysPassed: DaysPassed__GetEventSource, BSTEventSource_DaysPassed__Event__AddEventSink, BSTEventSource_DaysPassed__Event__RemoveEventSink;
        DefaultObjectsReadyEvent: DefaultObjectsReadyEvent__GetEventSource, BSTEventSource_DefaultObjectsReadyEvent__Event__AddEventSink, BSTEventSource_DefaultObjectsReadyEvent__Event__RemoveEventSink;
        DisarmedEvent: DisarmedEvent__GetEventSource, BSTEventSource_DisarmedEvent__Event__AddEventSink, BSTEventSource_DisarmedEvent__Event__RemoveEventSink;
        DragonSoulGained: DragonSoulGained__GetEventSource, BSTEventSource_DragonSoulGained__Event__AddEventSink, BSTEventSource_DragonSoulGained__Event__RemoveEventSink;
        FinePaid: FinePaid__GetEventSource, BSTEventSource_FinePaid__Event__AddEventSink, BSTEventSource_FinePaid__Event__RemoveEventSink;
        GrandTheftHorse: GrandTheftHorse__GetEventSource, BSTEventSource_GrandTheftHorse__Event__AddEventSink, BSTEventSource_GrandTheftHorse__Event__RemoveEventSink;
        HoursPassed: HoursPassed__GetEventSource, BSTEventSource_HoursPassed__Event__AddEventSink, BSTEventSource_HoursPassed__Event__RemoveEventSink;
        Inventory: Inventory__GetEventSource, BSTEventSource_Inventory__Event__AddEventSink, BSTEventSource_Inventory__Event__RemoveEventSink;
        ItemCrafted: ItemCrafted__GetEventSource, BSTEventSource_ItemCrafted__Event__AddEventSink, BSTEventSource_ItemCrafted__Event__RemoveEventSink;
        ItemSteal: ItemSteal__GetEventSource, BSTEventSource_ItemSteal__Event__AddEventSink, BSTEventSource_ItemSteal__Event__RemoveEventSink;
        ItemsPickpocketed: ItemsPickpocketed__GetEventSource, BSTEventSource_ItemsPickpocketed__Event__AddEventSink, BSTEventSource_ItemsPickpocketed__Event__RemoveEventSink;
        JailEscape: JailEscape__GetEventSource, BSTEventSource_JailEscape__Event__AddEventSink, BSTEventSource_JailEscape__Event__RemoveEventSink;
        Jailing: Jailing__GetEventSource, BSTEventSource_Jailing__Event__AddEventSink, BSTEventSource_Jailing__Event__RemoveEventSink;
        LevelIncrease: LevelIncrease__GetEventSource, BSTEventSource_LevelIncrease__Event__AddEventSink, BSTEventSource_LevelIncrease__Event__RemoveEventSink;
        LocationCleared: LocationCleared__GetEventSource, BSTEventSource_LocationCleared__Event__AddEventSink, BSTEventSource_LocationCleared__Event__RemoveEventSink;
        LocationDiscovery: LocationDiscovery__GetEventSource, BSTEventSource_LocationDiscovery__Event__AddEventSink, BSTEventSource_LocationDiscovery__Event__RemoveEventSink;
        LocksPicked: LocksPicked__GetEventSource, BSTEventSource_LocksPicked__Event__AddEventSink, BSTEventSource_LocksPicked__Event__RemoveEventSink;
        MurderCrime: MurderCrime__GetEventSource, BSTEventSource_MurderCrime__Event__AddEventSink, BSTEventSource_MurderCrime__Event__RemoveEventSink;
        ObjectiveState: ObjectiveState__GetEventSource, BSTEventSource_ObjectiveState__Event__AddEventSink, BSTEventSource_ObjectiveState__Event__RemoveEventSink;
        Pickpocket: Pickpocket__GetEventSource, BSTEventSource_Pickpocket__Event__AddEventSink, BSTEventSource_Pickpocket__Event__RemoveEventSink;
        PoisonedWeapon: PoisonedWeapon__GetEventSource, BSTEventSource_PoisonedWeapon__Event__AddEventSink, BSTEventSource_PoisonedWeapon__Event__RemoveEventSink;
        QuestStatus: QuestStatus__GetEventSource, BSTEventSource_QuestStatus__Event__AddEventSink, BSTEventSource_QuestStatus__Event__RemoveEventSink;
        RaceSexMenuNameChanged: RaceSexMenuEvent__GetEventSource, BSTEventSource_RaceSexMenuEvent__NameChangedEvent__AddEventSink, BSTEventSource_RaceSexMenuEvent__NameChangedEvent__RemoveEventSink;
        ShoutAttack: ShoutAttack__GetEventSource, BSTEventSource_ShoutAttack__Event__AddEventSink, BSTEventSource_ShoutAttack__Event__RemoveEventSink;
        ShoutMastered: ShoutMastered__GetEventSource, BSTEventSource_ShoutMastered__Event__AddEventSink, BSTEventSource_ShoutMastered__Event__RemoveEventSink;
        SkillIncrease: SkillIncrease__GetEventSource, BSTEventSource_SkillIncrease__Event__AddEventSink, BSTEventSource_SkillIncrease__Event__RemoveEventSink;
        SoulGemsUsed: SoulGemsUsed__GetEventSource, BSTEventSource_SoulGemsUsed__Event__AddEventSink, BSTEventSource_SoulGemsUsed__Event__RemoveEventSink;
        SoulsTrapped: SoulsTrapped__GetEventSource, BSTEventSource_SoulsTrapped__Event__AddEventSink, BSTEventSource_SoulsTrapped__Event__RemoveEventSink;
        SpellAttack: SpellAttack__GetEventSource, BSTEventSource_SpellAttack__Event__AddEventSink, BSTEventSource_SpellAttack__Event__RemoveEventSink;
        SpellsLearned: SpellsLearned__GetEventSource, BSTEventSource_SpellsLearned__Event__AddEventSink, BSTEventSource_SpellsLearned__Event__RemoveEventSink;
        TESHarvestItemHarvested: TESHarvestEvent__GetEventSource, BSTEventSource_TESHarvestEvent__ItemHarvested__AddEventSink, BSTEventSource_TESHarvestEvent__ItemHarvested__RemoveEventSink;
        TimesBartered: TimesBartered__GetEventSource, BSTEventSource_TimesBartered__Event__AddEventSink, BSTEventSource_TimesBartered__Event__RemoveEventSink;
        TimesTrained: TimesTrained__GetEventSource, BSTEventSource_TimesTrained__Event__AddEventSink, BSTEventSource_TimesTrained__Event__RemoveEventSink;
        Trespass: Trespass__GetEventSource, BSTEventSource_Trespass__Event__AddEventSink, BSTEventSource_Trespass__Event__RemoveEventSink;
        WeaponAttack: WeaponAttack__GetEventSource, BSTEventSource_WeaponAttack__Event__AddEventSink, BSTEventSource_WeaponAttack__Event__RemoveEventSink;
        WordLearned: WordLearned__GetEventSource, BSTEventSource_WordLearned__Event__AddEventSink, BSTEventSource_WordLearned__Event__RemoveEventSink;
        WordUnlocked: WordUnlocked__GetEventSource, BSTEventSource_WordUnlocked__Event__AddEventSink, BSTEventSource_WordUnlocked__Event__RemoveEventSink;
    }
}

/// # Safety
/// `p` is a reference pointer of an event the engine built, null or live for the event's lifetime.
unsafe fn refr<'a>(p: *mut classes::TESObjectREFR) -> Option<&'a TESObjectREFR> {
    unsafe { p.cast::<TESObjectREFR>().as_ref() }
}

fn fmt_refr(r: Option<&TESObjectREFR>) -> String {
    r.map_or_else(|| "null".into(), |r| format!("{r:?}"))
}

impl MenuOpenCloseEvent {
    /// The menu's name; interned, so [`BSFixedString::ptr_eq`] identifies the menu.
    #[must_use]
    pub fn menu_name(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr(&raw const self.0.menuName) }
    }

    #[must_use]
    pub fn opening(&self) -> bool {
        self.0.opening != 0
    }
}

impl fmt::Debug for MenuOpenCloseEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MenuOpenCloseEvent {{ menuName: {:?}, opening: {} }}", self.menu_name(), self.opening())
    }
}

impl MenuModeChangeEvent {
    /// The menu whose show or hide changed the pause state.
    #[must_use]
    pub fn menu_name(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr(&raw const self.0.menuName) }
    }

    #[must_use]
    pub fn in_menu_mode(&self) -> bool {
        self.0.inMenuMode != 0
    }
}

impl fmt::Debug for MenuModeChangeEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MenuModeChangeEvent {{ menuName: {:?}, inMenuMode: {} }}", self.menu_name(), self.in_menu_mode())
    }
}

impl TESEquipEvent {
    /// The actor equipping or unequipping.
    #[must_use]
    pub fn actor(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.actor) }
    }

    /// Form id of the item.
    #[must_use]
    pub fn base_object(&self) -> FormId {
        FormId(self.0.baseObject)
    }

    /// Form id of the item's own reference when it had one, else 0.
    #[must_use]
    pub fn original_refr(&self) -> FormId {
        FormId(self.0.originalRefr)
    }

    /// The inventory entry's unique id, 0 for stackable items.
    #[must_use]
    pub fn unique_id(&self) -> u16 {
        self.0.uniqueID
    }

    #[must_use]
    pub fn equipped(&self) -> bool {
        self.0.equipped != 0
    }
}

impl fmt::Debug for TESEquipEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TESEquipEvent {{ actor: {}, baseObject: {}, originalRefr: {}, uniqueID: {}, equipped: {} }}",
            fmt_refr(self.actor()),
            self.base_object(),
            self.original_refr(),
            self.unique_id(),
            self.equipped()
        )
    }
}

impl TESContainerChangedEvent {
    /// Form id of the container the item left, 0 when created.
    #[must_use]
    pub fn old_container(&self) -> FormId {
        FormId(self.0.oldContainer)
    }

    /// Form id of the receiving container, 0 when dropped or destroyed.
    #[must_use]
    pub fn new_container(&self) -> FormId {
        FormId(self.0.newContainer)
    }

    /// Form id of the item.
    #[must_use]
    pub fn base_object(&self) -> FormId {
        FormId(self.0.baseObject)
    }

    #[must_use]
    pub fn item_count(&self) -> u32 {
        self.0.itemCount
    }

    /// Form id of the item's reference when it has one.
    #[must_use]
    pub fn reference(&self) -> FormId {
        FormId(self.0.reference)
    }

    #[must_use]
    pub fn unique_id(&self) -> u16 {
        self.0.uniqueID
    }
}

impl fmt::Debug for TESContainerChangedEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TESContainerChangedEvent {{ oldContainer: {}, newContainer: {}, baseObject: {}, itemCount: {}, reference: {}, uniqueID: {} }}",
            self.old_container(),
            self.new_container(),
            self.base_object(),
            self.item_count(),
            self.reference(),
            self.unique_id()
        )
    }
}

impl TESActivateEvent {
    #[must_use]
    pub fn object_activated(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.objectActivated) }
    }

    #[must_use]
    pub fn action_ref(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.actionRef) }
    }
}

impl fmt::Debug for TESActivateEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TESActivateEvent {{ objectActivated: {}, actionRef: {} }}", fmt_refr(self.object_activated()), fmt_refr(self.action_ref()))
    }
}

impl TESHitEvent {
    /// The reference hit.
    #[must_use]
    pub fn target(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.target) }
    }

    /// The aggressor.
    #[must_use]
    pub fn cause(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.cause) }
    }

    /// Form id of the weapon, spell or explosion.
    #[must_use]
    pub fn source_form(&self) -> FormId {
        FormId(self.0.source)
    }

    /// Form id of the projectile, 0 for melee.
    #[must_use]
    pub fn projectile(&self) -> FormId {
        FormId(self.0.projectile)
    }

    /// Power attack, sneak attack, bash and blocked bits; the high nibble is unused.
    #[must_use]
    pub fn flags(&self) -> HitFlags {
        HitFlags(u32::from(self.0.flags))
    }
}

impl fmt::Debug for TESHitEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TESHitEvent {{ target: {}, cause: {}, source: {}, projectile: {}, flags: {:#x} }}",
            fmt_refr(self.target()),
            fmt_refr(self.cause()),
            self.source_form(),
            self.projectile(),
            self.flags().bits()
        )
    }
}

/// `TESCombatEvent::new_state` values.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CombatState {
    None,
    Combat,
    Searching,
}

impl TESCombatEvent {
    #[must_use]
    pub fn actor(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.actor) }
    }

    #[must_use]
    pub fn target_actor(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.targetActor) }
    }

    /// `None` for a value other than 0, 1 or 2.
    #[must_use]
    pub fn new_state(&self) -> Option<CombatState> {
        match self.0.newState {
            0 => Some(CombatState::None),
            1 => Some(CombatState::Combat),
            2 => Some(CombatState::Searching),
            _ => None,
        }
    }
}

impl fmt::Debug for TESCombatEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TESCombatEvent {{ actor: {}, targetActor: {}, newState: {} }}", fmt_refr(self.actor()), fmt_refr(self.target_actor()), self.0.newState)
    }
}

impl TESCellAttachDetachEvent {
    #[must_use]
    pub fn reference(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.reference) }
    }

    #[must_use]
    pub fn attached(&self) -> bool {
        self.0.attached != 0
    }
}

impl fmt::Debug for TESCellAttachDetachEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TESCellAttachDetachEvent {{ reference: {}, attached: {} }}", fmt_refr(self.reference()), self.attached())
    }
}

impl TESObjectLoadedEvent {
    #[must_use]
    pub fn form_id(&self) -> FormId {
        FormId(self.0.formID)
    }

    #[must_use]
    pub fn loaded(&self) -> bool {
        self.0.loaded != 0
    }
}

impl fmt::Debug for TESObjectLoadedEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TESObjectLoadedEvent {{ formID: {}, loaded: {} }}", self.form_id(), self.loaded())
    }
}

impl fmt::Debug for TESLoadGameEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TESLoadGameEvent")
    }
}

impl TESInitScriptEvent {
    #[must_use]
    pub fn reference(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.reference) }
    }
}

impl fmt::Debug for TESInitScriptEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TESInitScriptEvent {{ reference: {} }}", fmt_refr(self.reference()))
    }
}

/// `TESFurnitureEvent::kind` values.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FurnitureEvent {
    Sit,
    GetUp,
}

impl TESFurnitureEvent {
    #[must_use]
    pub fn actor(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.actor) }
    }

    #[must_use]
    pub fn target_furniture(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.targetFurniture) }
    }

    #[must_use]
    pub fn kind(&self) -> Option<FurnitureEvent> {
        match self.0.type_ {
            0 => Some(FurnitureEvent::Sit),
            1 => Some(FurnitureEvent::GetUp),
            _ => None,
        }
    }
}

impl fmt::Debug for TESFurnitureEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TESFurnitureEvent {{ actor: {}, targetFurniture: {}, type: {} }}", fmt_refr(self.actor()), fmt_refr(self.target_furniture()), self.0.type_)
    }
}

/// `TESPackageEvent::kind` values.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PackageEvent {
    Start,
    End,
    Change,
}

impl TESPackageEvent {
    #[must_use]
    pub fn actor(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.actor) }
    }

    /// Form id of the package.
    #[must_use]
    pub fn package(&self) -> FormId {
        FormId(self.0.package)
    }

    #[must_use]
    pub fn kind(&self) -> Option<PackageEvent> {
        match self.0.type_ {
            0 => Some(PackageEvent::Start),
            1 => Some(PackageEvent::End),
            2 => Some(PackageEvent::Change),
            _ => None,
        }
    }
}

impl fmt::Debug for TESPackageEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TESPackageEvent {{ actor: {}, package: {}, type: {} }}", fmt_refr(self.actor()), self.package(), self.0.type_)
    }
}

impl TESActorLocationChangeEvent {
    #[must_use]
    pub fn actor(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.actor) }
    }

    #[must_use]
    pub fn old_location(&self) -> Option<&BGSLocation> {
        unsafe { self.0.oldLoc.cast::<BGSLocation>().as_ref() }
    }

    #[must_use]
    pub fn new_location(&self) -> Option<&BGSLocation> {
        unsafe { self.0.newLoc.cast::<BGSLocation>().as_ref() }
    }
}

impl fmt::Debug for TESActorLocationChangeEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TESActorLocationChangeEvent {{ actor: {}, oldLoc: {:?}, newLoc: {:?} }}", fmt_refr(self.actor()), self.old_location(), self.new_location())
    }
}

impl TESLockChangedEvent {
    #[must_use]
    pub fn locked_object(&self) -> Option<&TESObjectREFR> {
        unsafe { refr(self.0.lockedObject) }
    }
}

impl fmt::Debug for TESLockChangedEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TESLockChangedEvent {{ lockedObject: {} }}", fmt_refr(self.locked_object()))
    }
}

impl BSAnimationGraphEvent {
    /// The animation event name.
    #[must_use]
    pub fn tag(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr(&raw const self.0.tag) }
    }

    /// The reference that owns the graph (pass 7: the `PlayerCharacter` itself in every player event).
    #[must_use]
    pub fn owner(&self) -> Option<&TESObjectREFR> {
        unsafe { self.0.owner.cast::<TESObjectREFR>().as_ref() }
    }

    #[must_use]
    pub fn payload(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr(&raw const self.0.payload) }
    }
}

impl fmt::Debug for BSAnimationGraphEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BSAnimationGraphEvent {{ tag: {:?}, owner: {}, payload: {:?} }}", self.tag(), fmt_refr(self.owner()), self.payload())
    }
}

impl BGSFootstepEvent {
    /// The handle of the actor whose animation produced the footstep.
    #[must_use]
    pub fn actor_handle(&self) -> RefHandle {
        RefHandle(self.0.actorHandle)
    }

    /// The animation event name, a copy of the graph event's tag.
    #[must_use]
    pub fn tag(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr(&raw const self.0.tag) }
    }
}

impl fmt::Debug for BGSFootstepEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BGSFootstepEvent {{ actor: {}, tag: {:?} }}", self.actor_handle(), self.tag())
    }
}

impl BGSActorCellEvent {
    /// The actor's handle, the low dword.
    #[must_use]
    pub fn actor_handle(&self) -> RefHandle {
        RefHandle((self.0.actorHandleAndCell & 0xFFFF_FFFF) as u32)
    }

    /// The cell's form id, the high dword.
    #[must_use]
    pub fn cell(&self) -> FormId {
        FormId((self.0.actorHandleAndCell >> 32) as u32)
    }

    #[must_use]
    pub fn attached(&self) -> bool {
        self.0.attached != 0
    }
}

impl fmt::Debug for BGSActorCellEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BGSActorCellEvent {{ actorHandle: {}, cell: {}, attached: {} }}", self.actor_handle(), self.cell(), self.attached())
    }
}
