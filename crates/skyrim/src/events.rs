//! Events (subsystem pass 3): `BSTEventSource<E>` with sink registration, the [`EventSink`] trait a
//! mod implements, and the [`Event`] impls that tie each event to the source the engine notifies
//! (the TES holder's 56, the UI's two, the input manager's, the player's, the global footstep
//! source and the 46 story-event singletons). The events pass 3 laid out are the generated
//! `sre::TESEquipEvent`, `sre::MenuOpenCloseEvent` and the rest, read through their fields and
//! readers; the events without a layout are opaque types here a sink can only count.
//!
//! A sink is a `#[repr(C)]` object whose first word points at a two-slot vtable (destructor,
//! `ProcessEvent`); the engine stores the sink's address and calls slot 1 with the event and the
//! source on the sending thread. [`Sink`] builds that object around any [`EventSink`] implementation;
//! it must live at one address for as long as it is registered, so [`BSTEventSource::add_sink`]
//! takes a `&'static Sink` (a `static`, or `Box::leak`) and returns a [`Registration`] that
//! unregisters on drop or is leaked to stay registered for the life of the game. Adding and
//! removing from inside a handler is safe: the engine queues the change until the walk ends.

use core::ffi::c_void;
use core::marker::PhantomData;

use sre::raw::fns;
use sre::BSEventNotifyControl;

type SinkFn = unsafe extern "C" fn(source: *mut sre::BSTEventSource, sink: *const c_void);

fn sink_fn(id: u64) -> SinkFn {
    // Every instantiation of the engine's sink has these two slots; the sink objects here copy that shape.
    sre::fixed_slots!("BSTEventSink<MenuOpenCloseEvent>", 0x0, 0, 1);
    unsafe { crate::engine_fn(id) }
}

/// An event type with the engine source that sends it.
pub trait Event: Sized + 'static {
    /// Address Library id of this instantiation's `BSTEventSource<E>::AddEventSink`.
    fn add_sink() -> u64;
    /// Address Library id of this instantiation's `BSTEventSource<E>::RemoveEventSink`.
    fn remove_sink() -> u64;
    /// The source the engine notifies with this event, `None` while its holder does not exist yet.
    fn source() -> Option<&'static BSTEventSource<Self>>;
}

/// What a mod implements to receive events of type `E`; called on the sending thread with the
/// source lock held, so the handler must not block on anything that registers or notifies.
pub trait EventSink<E: Event>: Sync + 'static {
    fn process(&self, event: &E, source: &BSTEventSource<E>) -> BSEventNotifyControl;
}

/// The two slots the engine calls on a sink object `T` for events of type `E`.
#[repr(C)]
struct SinkVtable<T, E> {
    dtor: unsafe extern "C" fn(this: *mut T, flags: u32) -> *mut T,
    process: unsafe extern "C" fn(this: *const T, event: *const E, source: *mut sre::BSTEventSource) -> BSEventNotifyControl,
}

/// The sink object the engine sees: the vtable pointer, then the handler.
#[repr(C)]
pub struct Sink<E: Event, S: EventSink<E>> {
    vtable: &'static SinkVtable<Self, E>,
    handler: S,
}

impl<E: Event, S: EventSink<E>> Sink<E, S> {
    const VTABLE: &'static SinkVtable<Self, E> = &SinkVtable { dtor: Self::dtor, process: Self::process };

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

    unsafe extern "C" fn process(this: *const Self, event: *const E, source: *mut sre::BSTEventSource) -> BSEventNotifyControl {
        let (Some(this), Some(event), Some(source)) = (unsafe { this.as_ref() }, unsafe { event.as_ref() }, unsafe { source.cast::<BSTEventSource<E>>().as_ref() }) else {
            return BSEventNotifyControl::Continue;
        };
        this.handler.process(event, source)
    }
}

/// What an [`ErasedSink`] runs: the event and the source as the engine passes them.
pub type ErasedHandler = Box<dyn Fn(*const c_void, *const sre::BSTEventSource) -> BSEventNotifyControl + Send + Sync>;

/// A sink whose event type is chosen at run time, for a host that registers on behalf of code it
/// may unload: the engine keeps the host's object while the handler decides what runs behind it.
#[repr(C)]
pub struct ErasedSink {
    vtable: &'static SinkVtable<Self, c_void>,
    handler: ErasedHandler,
}

impl ErasedSink {
    const VTABLE: &'static SinkVtable<Self, c_void> = &SinkVtable { dtor: Self::dtor, process: Self::process };

    /// Leaks the sink: once registered, the engine keeps its address for the life of the game.
    #[must_use]
    pub fn new(handler: ErasedHandler) -> &'static Self {
        Box::leak(Box::new(Self { vtable: Self::VTABLE, handler }))
    }

    unsafe extern "C" fn dtor(this: *mut Self, _flags: u32) -> *mut Self {
        this
    }

    unsafe extern "C" fn process(this: *const Self, event: *const c_void, source: *mut sre::BSTEventSource) -> BSEventNotifyControl {
        match unsafe { this.as_ref() } {
            Some(this) => (this.handler)(event, source),
            None => BSEventNotifyControl::Continue,
        }
    }
}

/// A source known by address with its event type's registration function, for an [`ErasedSink`];
/// laid out as C and frozen, as it crosses the framework's plugin ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErasedSource {
    source: *mut sre::BSTEventSource,
    add: u64,
}

const _: () = assert!(size_of::<ErasedSource>() == 16 && core::mem::offset_of!(ErasedSource, add) == 8);

unsafe impl Send for ErasedSource {}
unsafe impl Sync for ErasedSource {}

impl ErasedSource {
    /// The source of `E`, `None` while its holder does not exist yet.
    #[must_use]
    pub fn of<E: Event>() -> Option<Self> {
        E::source().map(Self::new)
    }

    /// A source of `E` reached some other way, such as a reference's graph source.
    #[must_use]
    pub fn new<E: Event>(source: &BSTEventSource<E>) -> Self {
        Self { source: source.as_mut_ptr(), add: E::add_sink() }
    }

    #[must_use]
    pub fn address(&self) -> usize {
        self.source as usize
    }

    /// Registers `sink` through the event type's `AddEventSink`; a sink already registered is not added again.
    pub fn add_sink(&self, sink: &'static ErasedSink) {
        unsafe { sink_fn(self.add)(self.source, core::ptr::from_ref(sink).cast()) };
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
        unsafe { sink_fn(E::remove_sink())(self.source.as_mut_ptr(), self.sink) };
    }
}

/// A `BSTEventSource<E>`: the generated `sre::BSTEventSource` typed with the event it sends.
#[repr(transparent)]
pub struct BSTEventSource<E: Event>(sre::BSTEventSource, PhantomData<E>);

impl<E: Event> BSTEventSource<E> {
    /// # Safety
    /// `p` must be null or point to a live `BSTEventSource<E>` that is never freed.
    #[must_use]
    pub unsafe fn from_ptr(p: *const sre::BSTEventSource) -> Option<&'static Self> {
        unsafe { p.cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &sre::BSTEventSource {
        &self.0
    }

    fn as_mut_ptr(&self) -> *mut sre::BSTEventSource {
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

    /// Registers `sink` through `BSTEventSource<E>::AddEventSink`; a sink already registered is
    /// not added again. The engine keeps the pointer, so the sink is `'static` and must never move.
    pub fn add_sink<S: EventSink<E>>(&'static self, sink: &'static Sink<E, S>) -> Registration<E> {
        unsafe { sink_fn(E::add_sink())(self.as_mut_ptr(), sink.as_ptr()) };
        Registration { source: self, sink: sink.as_ptr() }
    }

    /// Registers `sink` ahead of every sink already registered. A notification walks the sinks in
    /// order and ends at the first that returns [`BSEventNotifyControl::Stop`], so this sink sees each
    /// event first and can hide it from all the others (pass 9 on the input source). Takes the
    /// source's lock, so it waits for a running notification and must not be called from a sink
    /// of this source.
    pub fn add_sink_first<S: EventSink<E>>(&'static self, sink: &'static Sink<E, S>) -> Registration<E> {
        let _lock = crate::spin::SpinGuard::lock(&self.0.lock);
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
/// `p` must be null or point to a `BSTEventSource<E>` inside an object that lives for the rest of the game.
unsafe fn source_at<E: Event>(p: *const sre::BSTEventSource) -> Option<&'static BSTEventSource<E>> {
    unsafe { BSTEventSource::from_ptr(p) }
}

macro_rules! opaque_events {
    ($($(#[$doc:meta])* $name:ident;)*) => {
        $(
            $(#[$doc])*
            #[repr(C)]
            pub struct $name {
                _payload: [u8; 0],
            }
        )*
    };
}

/// `impl Event` rows: the type, how to reach its source, and its `AddEventSink`/`RemoveEventSink` ids.
macro_rules! sources {
    ($($event:ty => $source:expr, $add:ident, $remove:ident;)*) => {
        $(
            impl Event for $event {
                fn add_sink() -> u64 {
                    sre::id!(fns::$add)
                }
                fn remove_sink() -> u64 {
                    sre::id!(fns::$remove)
                }
                fn source() -> Option<&'static BSTEventSource<Self>> {
                    unsafe { source_at($source) }
                }
            }
        )*
    };
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
    (ptr $accessor:ident) => {
        sre::EventSourceHolder_TES::get_singleton().map_or(core::ptr::null(), |h| h.$accessor().cast_const())
    };
    ($field:ident) => {
        sre::EventSourceHolder_TES::get_singleton().map_or(core::ptr::null(), |h| &raw const h.$field)
    };
}

sources! {
    BGSEventProcessedEvent => tes!(ptr source_BGSEventProcessedEvent_ptr), BSTEventSource_BGSEventProcessedEvent__AddEventSink, BSTEventSource_BGSEventProcessedEvent__RemoveEventSink;
    sre::TESActivateEvent => tes!(source_TESActivateEvent), BSTEventSource_TESActivateEvent__AddEventSink, BSTEventSource_TESActivateEvent__RemoveEventSink;
    sre::TESActiveEffectApplyRemoveEvent => tes!(ptr source_TESActiveEffectApplyRemoveEvent_ptr), BSTEventSource_TESActiveEffectApplyRemoveEvent__AddEventSink, BSTEventSource_TESActiveEffectApplyRemoveEvent__RemoveEventSink;
    sre::TESActorLocationChangeEvent => tes!(source_TESActorLocationChangeEvent), BSTEventSource_TESActorLocationChangeEvent__AddEventSink, BSTEventSource_TESActorLocationChangeEvent__RemoveEventSink;
    sre::TESBookReadEvent => tes!(ptr source_TESBookReadEvent_ptr), BSTEventSource_TESBookReadEvent__AddEventSink, BSTEventSource_TESBookReadEvent__RemoveEventSink;
    sre::TESCellAttachDetachEvent => tes!(source_TESCellAttachDetachEvent), BSTEventSource_TESCellAttachDetachEvent__AddEventSink, BSTEventSource_TESCellAttachDetachEvent__RemoveEventSink;
    sre::TESCellFullyLoadedEvent => tes!(ptr source_TESCellFullyLoadedEvent_ptr), BSTEventSource_TESCellFullyLoadedEvent__AddEventSink, BSTEventSource_TESCellFullyLoadedEvent__RemoveEventSink;
    TESCellReadyToApplyDecalsEvent => tes!(ptr source_TESCellReadyToApplyDecalsEvent_ptr), BSTEventSource_TESCellReadyToApplyDecalsEvent__AddEventSink, BSTEventSource_TESCellReadyToApplyDecalsEvent__RemoveEventSink;
    sre::TESCombatEvent => tes!(source_TESCombatEvent), BSTEventSource_TESCombatEvent__AddEventSink, BSTEventSource_TESCombatEvent__RemoveEventSink;
    sre::TESContainerChangedEvent => tes!(source_TESContainerChangedEvent), BSTEventSource_TESContainerChangedEvent__AddEventSink, BSTEventSource_TESContainerChangedEvent__RemoveEventSink;
    sre::TESDeathEvent => tes!(ptr source_TESDeathEvent_ptr), BSTEventSource_TESDeathEvent__AddEventSink, BSTEventSource_TESDeathEvent__RemoveEventSink;
    sre::TESDestructionStageChangedEvent => tes!(ptr source_TESDestructionStageChangedEvent_ptr), BSTEventSource_TESDestructionStageChangedEvent__AddEventSink, BSTEventSource_TESDestructionStageChangedEvent__RemoveEventSink;
    sre::TESEnterBleedoutEvent => tes!(ptr source_TESEnterBleedoutEvent_ptr), BSTEventSource_TESEnterBleedoutEvent__AddEventSink, BSTEventSource_TESEnterBleedoutEvent__RemoveEventSink;
    sre::TESEquipEvent => tes!(source_TESEquipEvent), BSTEventSource_TESEquipEvent__AddEventSink, BSTEventSource_TESEquipEvent__RemoveEventSink;
    sre::TESFormDeleteEvent => tes!(ptr source_TESFormDeleteEvent_ptr), BSTEventSource_TESFormDeleteEvent__AddEventSink, BSTEventSource_TESFormDeleteEvent__RemoveEventSink;
    sre::TESFurnitureEvent => tes!(source_TESFurnitureEvent), BSTEventSource_TESFurnitureEvent__AddEventSink, BSTEventSource_TESFurnitureEvent__RemoveEventSink;
    sre::TESGrabReleaseEvent => tes!(ptr source_TESGrabReleaseEvent_ptr), BSTEventSource_TESGrabReleaseEvent__AddEventSink, BSTEventSource_TESGrabReleaseEvent__RemoveEventSink;
    sre::TESHitEvent => tes!(source_TESHitEvent), BSTEventSource_TESHitEvent__AddEventSink, BSTEventSource_TESHitEvent__RemoveEventSink;
    sre::TESInitScriptEvent => tes!(source_TESInitScriptEvent), BSTEventSource_TESInitScriptEvent__AddEventSink, BSTEventSource_TESInitScriptEvent__RemoveEventSink;
    sre::TESLoadGameEvent => tes!(source_TESLoadGameEvent), BSTEventSource_TESLoadGameEvent__AddEventSink, BSTEventSource_TESLoadGameEvent__RemoveEventSink;
    sre::TESLockChangedEvent => tes!(source_TESLockChangedEvent), BSTEventSource_TESLockChangedEvent__AddEventSink, BSTEventSource_TESLockChangedEvent__RemoveEventSink;
    sre::TESMagicEffectApplyEvent => tes!(ptr source_TESMagicEffectApplyEvent_ptr), BSTEventSource_TESMagicEffectApplyEvent__AddEventSink, BSTEventSource_TESMagicEffectApplyEvent__RemoveEventSink;
    sre::TESMagicWardHitEvent => tes!(ptr source_TESMagicWardHitEvent_ptr), BSTEventSource_TESMagicWardHitEvent__AddEventSink, BSTEventSource_TESMagicWardHitEvent__RemoveEventSink;
    sre::TESMoveAttachDetachEvent => tes!(ptr source_TESMoveAttachDetachEvent_ptr), BSTEventSource_TESMoveAttachDetachEvent__AddEventSink, BSTEventSource_TESMoveAttachDetachEvent__RemoveEventSink;
    sre::TESObjectLoadedEvent => tes!(source_TESObjectLoadedEvent), BSTEventSource_TESObjectLoadedEvent__AddEventSink, BSTEventSource_TESObjectLoadedEvent__RemoveEventSink;
    sre::TESObjectREFRTranslationEvent => tes!(ptr source_TESObjectREFRTranslationEvent_ptr), BSTEventSource_TESObjectREFRTranslationEvent__AddEventSink, BSTEventSource_TESObjectREFRTranslationEvent__RemoveEventSink;
    sre::TESOpenCloseEvent => tes!(ptr source_TESOpenCloseEvent_ptr), BSTEventSource_TESOpenCloseEvent__AddEventSink, BSTEventSource_TESOpenCloseEvent__RemoveEventSink;
    sre::TESPackageEvent => tes!(source_TESPackageEvent), BSTEventSource_TESPackageEvent__AddEventSink, BSTEventSource_TESPackageEvent__RemoveEventSink;
    sre::TESPerkEntryRunEvent => tes!(ptr source_TESPerkEntryRunEvent_ptr), BSTEventSource_TESPerkEntryRunEvent__AddEventSink, BSTEventSource_TESPerkEntryRunEvent__RemoveEventSink;
    TESQuestInitEvent => tes!(ptr source_TESQuestInitEvent_ptr), BSTEventSource_TESQuestInitEvent__AddEventSink, BSTEventSource_TESQuestInitEvent__RemoveEventSink;
    sre::TESQuestStageEvent => tes!(ptr source_TESQuestStageEvent_ptr), BSTEventSource_TESQuestStageEvent__AddEventSink, BSTEventSource_TESQuestStageEvent__RemoveEventSink;
    sre::TESQuestStageItemDoneEvent => tes!(ptr source_TESQuestStageItemDoneEvent_ptr), BSTEventSource_TESQuestStageItemDoneEvent__AddEventSink, BSTEventSource_TESQuestStageItemDoneEvent__RemoveEventSink;
    sre::TESQuestStartStopEvent => tes!(ptr source_TESQuestStartStopEvent_ptr), BSTEventSource_TESQuestStartStopEvent__AddEventSink, BSTEventSource_TESQuestStartStopEvent__RemoveEventSink;
    sre::TESResetEvent => tes!(ptr source_TESResetEvent_ptr), BSTEventSource_TESResetEvent__AddEventSink, BSTEventSource_TESResetEvent__RemoveEventSink;
    sre::TESResolveNPCTemplatesEvent => tes!(ptr source_TESResolveNPCTemplatesEvent_ptr), BSTEventSource_TESResolveNPCTemplatesEvent__AddEventSink, BSTEventSource_TESResolveNPCTemplatesEvent__RemoveEventSink;
    sre::TESSceneEvent => tes!(ptr source_TESSceneEvent_ptr), BSTEventSource_TESSceneEvent__AddEventSink, BSTEventSource_TESSceneEvent__RemoveEventSink;
    TESSceneActionEvent => tes!(ptr source_TESSceneActionEvent_ptr), BSTEventSource_TESSceneActionEvent__AddEventSink, BSTEventSource_TESSceneActionEvent__RemoveEventSink;
    TESScenePhaseEvent => tes!(ptr source_TESScenePhaseEvent_ptr), BSTEventSource_TESScenePhaseEvent__AddEventSink, BSTEventSource_TESScenePhaseEvent__RemoveEventSink;
    sre::TESSellEvent => tes!(ptr source_TESSellEvent_ptr), BSTEventSource_TESSellEvent__AddEventSink, BSTEventSource_TESSellEvent__RemoveEventSink;
    sre::TESSleepStartEvent => tes!(ptr source_TESSleepStartEvent_ptr), BSTEventSource_TESSleepStartEvent__AddEventSink, BSTEventSource_TESSleepStartEvent__RemoveEventSink;
    sre::TESSleepStopEvent => tes!(ptr source_TESSleepStopEvent_ptr), BSTEventSource_TESSleepStopEvent__AddEventSink, BSTEventSource_TESSleepStopEvent__RemoveEventSink;
    sre::TESSpellCastEvent => tes!(ptr source_TESSpellCastEvent_ptr), BSTEventSource_TESSpellCastEvent__AddEventSink, BSTEventSource_TESSpellCastEvent__RemoveEventSink;
    sre::TESPlayerBowShotEvent => tes!(ptr source_TESPlayerBowShotEvent_ptr), BSTEventSource_TESPlayerBowShotEvent__AddEventSink, BSTEventSource_TESPlayerBowShotEvent__RemoveEventSink;
    sre::TESTopicInfoEvent => tes!(ptr source_TESTopicInfoEvent_ptr), BSTEventSource_TESTopicInfoEvent__AddEventSink, BSTEventSource_TESTopicInfoEvent__RemoveEventSink;
    sre::TESTrackedStatsEvent => tes!(ptr source_TESTrackedStatsEvent_ptr), BSTEventSource_TESTrackedStatsEvent__AddEventSink, BSTEventSource_TESTrackedStatsEvent__RemoveEventSink;
    sre::TESTrapHitEvent => tes!(ptr source_TESTrapHitEvent_ptr), BSTEventSource_TESTrapHitEvent__AddEventSink, BSTEventSource_TESTrapHitEvent__RemoveEventSink;
    sre::TESTriggerEvent => tes!(ptr source_TESTriggerEvent_ptr), BSTEventSource_TESTriggerEvent__AddEventSink, BSTEventSource_TESTriggerEvent__RemoveEventSink;
    TESTriggerEnterEvent => tes!(ptr source_TESTriggerEnterEvent_ptr), BSTEventSource_TESTriggerEnterEvent__AddEventSink, BSTEventSource_TESTriggerEnterEvent__RemoveEventSink;
    TESTriggerLeaveEvent => tes!(ptr source_TESTriggerLeaveEvent_ptr), BSTEventSource_TESTriggerLeaveEvent__AddEventSink, BSTEventSource_TESTriggerLeaveEvent__RemoveEventSink;
    sre::TESUniqueIDChangeEvent => tes!(ptr source_TESUniqueIDChangeEvent_ptr), BSTEventSource_TESUniqueIDChangeEvent__AddEventSink, BSTEventSource_TESUniqueIDChangeEvent__RemoveEventSink;
    // The four sources below have no named RemoveEventSink instance; they use the template body
    // events.md documents (0x3cd9e0), which every instance shares instruction for instruction.
    sre::TESWaitStartEvent => tes!(ptr source_TESWaitStartEvent_ptr), BSTEventSource_TESWaitStartEvent__AddEventSink, BSTEventSource_MenuOpenCloseEvent__RemoveEventSink;
    sre::TESWaitStopEvent => tes!(ptr source_TESWaitStopEvent_ptr), BSTEventSource_TESWaitStopEvent__AddEventSink, BSTEventSource_MenuOpenCloseEvent__RemoveEventSink;
    sre::TESSwitchRaceCompleteEvent => tes!(ptr source_TESSwitchRaceCompleteEvent_ptr), BSTEventSource_TESSwitchRaceCompleteEvent__AddEventSink, BSTEventSource_TESSwitchRaceCompleteEvent__RemoveEventSink;
    sre::TESFastTravelEndEvent => tes!(ptr source_TESFastTravelEndEvent_ptr), BSTEventSource_TESFastTravelEndEvent__AddEventSink, BSTEventSource_TESFastTravelEndEvent__RemoveEventSink;
    TESAmiiboTouchEvent => tes!(ptr source_TESAmiiboTouchEvent_ptr), BSTEventSource_TESAmiiboTouchEvent__AddEventSink, BSTEventSource_MenuOpenCloseEvent__RemoveEventSink;
    TESAmiiboForcedStopDetectionEvent => tes!(ptr source_TESAmiiboForcedStopDetectionEvent_ptr), BSTEventSource_TESAmiiboForcedStopDetectionEvent__AddEventSink, BSTEventSource_MenuOpenCloseEvent__RemoveEventSink;
}

/// What the input source sends: a pointer to the head of the frame's `InputEvent` list (pass 9
/// describes the events; `device`, `eventType` and `next` are medium here).
#[repr(transparent)]
pub struct InputEventList(*mut sre::InputEvent);

impl InputEventList {
    /// The first event of the frame, `None` for an empty list.
    #[must_use]
    pub fn first(&self) -> Option<&sre::InputEvent> {
        unsafe { self.0.as_ref() }
    }

    pub fn iter(&self) -> impl Iterator<Item = &sre::InputEvent> {
        let mut next = self.first();
        core::iter::from_fn(move || {
            let e = next?;
            next = unsafe { e.next.as_ref() };
            Some(e)
        })
    }
}

fn ui_source(offset: fn(&sre::EventSourceHolder_MenuOpenClose) -> *const sre::BSTEventSource) -> *const sre::BSTEventSource {
    sre::EventSourceHolder_MenuOpenClose::singleton().map_or(core::ptr::null(), offset)
}

fn player_source(offset: fn(&sre::PlayerCharacter) -> *const sre::BSTEventSource) -> *const sre::BSTEventSource {
    sre::PlayerCharacter::singleton().map_or(core::ptr::null(), offset)
}

sources! {
    sre::MenuOpenCloseEvent => ui_source(|ui| (&raw const ui.source_MenuOpenCloseEvent)), BSTEventSource_MenuOpenCloseEvent__AddEventSink, BSTEventSource_MenuOpenCloseEvent__RemoveEventSink;
    sre::MenuModeChangeEvent => ui_source(|ui| (&raw const ui.source_MenuModeChangeEvent)), BSTEventSource_MenuModeChangeEvent__AddEventSink, BSTEventSource_MenuModeChangeEvent__RemoveEventSink;
    InputEventList => sre::EventSourceHolder_Input::singleton().map_or(core::ptr::null(), |i| (&raw const i.source_InputEvent)), BSTEventSource_InputEvent____ptr64__AddEventSink, BSTEventSource_InputEvent____ptr64__RemoveEventSink;
    sre::BGSFootstepEvent => sre::g_footstep_event_source_ptr().map_or(core::ptr::null(), core::ptr::from_ref), BSTEventSource_BGSFootstepEvent__AddEventSink, BSTEventSource_BGSFootstepEvent__RemoveEventSink;
    sre::BGSActorCellEvent => player_source(|p| p.source_BGSActorCellEvent_ptr().cast_const()), BSTEventSource_BGSActorCellEvent__AddEventSink, BSTEventSource_BGSActorCellEvent__RemoveEventSink;
    sre::PositionPlayerEvent => player_source(|p| p.source_PositionPlayerEvent_ptr().cast_const()), BSTEventSource_PositionPlayerEvent__AddEventSink, BSTEventSource_PositionPlayerEvent__RemoveEventSink;
}

/// A behaviour graph's events come from the per-graph sources (`BShkbAnimationGraph` + 0x68):
/// `source()` is `None`, register on [`crate::animation::graph_event_source`] or
/// `BShkbAnimationGraphExt::event_source`.
impl Event for sre::BSAnimationGraphEvent {
    fn add_sink() -> u64 {
        sre::id!(fns::BSTEventSource_BSAnimationGraphEvent__AddEventSink)
    }
    fn remove_sink() -> u64 {
        sre::id!(fns::BSTEventSource_BSAnimationGraphEvent__RemoveEventSink)
    }
    fn source() -> Option<&'static BSTEventSource<Self>> {
        None
    }
}

/// The 46 story-event singletons (`ActorKill`, `Bounty`, `SkillIncrease`, ...), each one source
/// reached through `<Name>::GetEventSource`, a guarded function-local static. Rated medium, from
/// the registrations of their `BSTEventSink` classes; pass 3 did not read their payloads, so the
/// event types are opaque.
pub mod story {
    use super::{BSTEventSource, Event, source_at};
    use sre::raw::fns;

    macro_rules! story_events {
        ($($name:ident: $get:ident, $add:ident, $remove:ident;)*) => {
            $(
                /// Payload not described by pass 3.
                #[repr(C)]
                pub struct $name {
                    _payload: [u8; 0],
                }

                impl Event for $name {
                    fn add_sink() -> u64 {
                    sre::id!(fns::$add)
                }
                    fn remove_sink() -> u64 {
                    sre::id!(fns::$remove)
                }
                    fn source() -> Option<&'static BSTEventSource<Self>> {
                        unsafe { source_at(sre::$get().cast()) }
                    }
                }
            )*
        };
    }

    story_events! {
        ActorItemEquipped: actor_item_equipped__get_event_source, BSTEventSource_ActorItemEquipped__Event__AddEventSink, BSTEventSource_ActorItemEquipped__Event__RemoveEventSink;
        ActorKill: actor_kill__get_event_source, BSTEventSource_ActorKill__Event__AddEventSink, BSTEventSource_ActorKill__Event__RemoveEventSink;
        AssaultCrime: assault_crime__get_event_source, BSTEventSource_AssaultCrime__Event__AddEventSink, BSTEventSource_AssaultCrime__Event__RemoveEventSink;
        BooksRead: books_read__get_event_source, BSTEventSource_BooksRead__Event__AddEventSink, BSTEventSource_BooksRead__Event__RemoveEventSink;
        Bounty: bounty__get_event_source, BSTEventSource_Bounty__Event__AddEventSink, BSTEventSource_Bounty__Event__RemoveEventSink;
        ChestsLooted: chests_looted__get_event_source, BSTEventSource_ChestsLooted__Event__AddEventSink, BSTEventSource_ChestsLooted__Event__RemoveEventSink;
        ContractedDisease: contracted_disease__get_event_source, BSTEventSource_ContractedDisease__Event__AddEventSink, BSTEventSource_ContractedDisease__Event__RemoveEventSink;
        CriticalHitEvent: critical_hit_event__get_event_source, BSTEventSource_CriticalHitEvent__Event__AddEventSink, BSTEventSource_CriticalHitEvent__Event__RemoveEventSink;
        DaysJailed: days_jailed__get_event_source, BSTEventSource_DaysJailed__Event__AddEventSink, BSTEventSource_DaysJailed__Event__RemoveEventSink;
        DaysPassed: days_passed__get_event_source, BSTEventSource_DaysPassed__Event__AddEventSink, BSTEventSource_DaysPassed__Event__RemoveEventSink;
        DefaultObjectsReadyEvent: default_objects_ready_event__get_event_source, BSTEventSource_DefaultObjectsReadyEvent__Event__AddEventSink, BSTEventSource_DefaultObjectsReadyEvent__Event__RemoveEventSink;
        DisarmedEvent: disarmed_event__get_event_source, BSTEventSource_DisarmedEvent__Event__AddEventSink, BSTEventSource_DisarmedEvent__Event__RemoveEventSink;
        DragonSoulGained: dragon_soul_gained__get_event_source, BSTEventSource_DragonSoulGained__Event__AddEventSink, BSTEventSource_DragonSoulGained__Event__RemoveEventSink;
        FinePaid: fine_paid__get_event_source, BSTEventSource_FinePaid__Event__AddEventSink, BSTEventSource_FinePaid__Event__RemoveEventSink;
        GrandTheftHorse: grand_theft_horse__get_event_source, BSTEventSource_GrandTheftHorse__Event__AddEventSink, BSTEventSource_GrandTheftHorse__Event__RemoveEventSink;
        HoursPassed: hours_passed__get_event_source, BSTEventSource_HoursPassed__Event__AddEventSink, BSTEventSource_HoursPassed__Event__RemoveEventSink;
        Inventory: inventory__get_event_source, BSTEventSource_Inventory__Event__AddEventSink, BSTEventSource_Inventory__Event__RemoveEventSink;
        ItemCrafted: item_crafted__get_event_source, BSTEventSource_ItemCrafted__Event__AddEventSink, BSTEventSource_ItemCrafted__Event__RemoveEventSink;
        ItemSteal: item_steal__get_event_source, BSTEventSource_ItemSteal__Event__AddEventSink, BSTEventSource_ItemSteal__Event__RemoveEventSink;
        ItemsPickpocketed: items_pickpocketed__get_event_source, BSTEventSource_ItemsPickpocketed__Event__AddEventSink, BSTEventSource_ItemsPickpocketed__Event__RemoveEventSink;
        JailEscape: jail_escape__get_event_source, BSTEventSource_JailEscape__Event__AddEventSink, BSTEventSource_JailEscape__Event__RemoveEventSink;
        Jailing: jailing__get_event_source, BSTEventSource_Jailing__Event__AddEventSink, BSTEventSource_Jailing__Event__RemoveEventSink;
        LevelIncrease: level_increase__get_event_source, BSTEventSource_LevelIncrease__Event__AddEventSink, BSTEventSource_LevelIncrease__Event__RemoveEventSink;
        LocationCleared: location_cleared__get_event_source, BSTEventSource_LocationCleared__Event__AddEventSink, BSTEventSource_LocationCleared__Event__RemoveEventSink;
        LocationDiscovery: location_discovery__get_event_source, BSTEventSource_LocationDiscovery__Event__AddEventSink, BSTEventSource_LocationDiscovery__Event__RemoveEventSink;
        LocksPicked: locks_picked__get_event_source, BSTEventSource_LocksPicked__Event__AddEventSink, BSTEventSource_LocksPicked__Event__RemoveEventSink;
        MurderCrime: murder_crime__get_event_source, BSTEventSource_MurderCrime__Event__AddEventSink, BSTEventSource_MurderCrime__Event__RemoveEventSink;
        ObjectiveState: objective_state__get_event_source, BSTEventSource_ObjectiveState__Event__AddEventSink, BSTEventSource_ObjectiveState__Event__RemoveEventSink;
        Pickpocket: pickpocket__get_event_source, BSTEventSource_Pickpocket__Event__AddEventSink, BSTEventSource_Pickpocket__Event__RemoveEventSink;
        PoisonedWeapon: poisoned_weapon__get_event_source, BSTEventSource_PoisonedWeapon__Event__AddEventSink, BSTEventSource_PoisonedWeapon__Event__RemoveEventSink;
        QuestStatus: quest_status__get_event_source, BSTEventSource_QuestStatus__Event__AddEventSink, BSTEventSource_QuestStatus__Event__RemoveEventSink;
        RaceSexMenuNameChanged: race_sex_menu_event__get_event_source, BSTEventSource_RaceSexMenuEvent__NameChangedEvent__AddEventSink, BSTEventSource_RaceSexMenuEvent__NameChangedEvent__RemoveEventSink;
        ShoutAttack: shout_attack__get_event_source, BSTEventSource_ShoutAttack__Event__AddEventSink, BSTEventSource_ShoutAttack__Event__RemoveEventSink;
        ShoutMastered: shout_mastered__get_event_source, BSTEventSource_ShoutMastered__Event__AddEventSink, BSTEventSource_ShoutMastered__Event__RemoveEventSink;
        SkillIncrease: skill_increase__get_event_source, BSTEventSource_SkillIncrease__Event__AddEventSink, BSTEventSource_SkillIncrease__Event__RemoveEventSink;
        SoulGemsUsed: soul_gems_used__get_event_source, BSTEventSource_SoulGemsUsed__Event__AddEventSink, BSTEventSource_SoulGemsUsed__Event__RemoveEventSink;
        SoulsTrapped: souls_trapped__get_event_source, BSTEventSource_SoulsTrapped__Event__AddEventSink, BSTEventSource_SoulsTrapped__Event__RemoveEventSink;
        SpellAttack: spell_attack__get_event_source, BSTEventSource_SpellAttack__Event__AddEventSink, BSTEventSource_SpellAttack__Event__RemoveEventSink;
        SpellsLearned: spells_learned__get_event_source, BSTEventSource_SpellsLearned__Event__AddEventSink, BSTEventSource_SpellsLearned__Event__RemoveEventSink;
        TESHarvestItemHarvested: tes_harvest_event__get_event_source, BSTEventSource_TESHarvestEvent__ItemHarvested__AddEventSink, BSTEventSource_TESHarvestEvent__ItemHarvested__RemoveEventSink;
        TimesBartered: times_bartered__get_event_source, BSTEventSource_TimesBartered__Event__AddEventSink, BSTEventSource_TimesBartered__Event__RemoveEventSink;
        TimesTrained: times_trained__get_event_source, BSTEventSource_TimesTrained__Event__AddEventSink, BSTEventSource_TimesTrained__Event__RemoveEventSink;
        Trespass: trespass__get_event_source, BSTEventSource_Trespass__Event__AddEventSink, BSTEventSource_Trespass__Event__RemoveEventSink;
        WeaponAttack: weapon_attack__get_event_source, BSTEventSource_WeaponAttack__Event__AddEventSink, BSTEventSource_WeaponAttack__Event__RemoveEventSink;
        WordLearned: word_learned__get_event_source, BSTEventSource_WordLearned__Event__AddEventSink, BSTEventSource_WordLearned__Event__RemoveEventSink;
        WordUnlocked: word_unlocked__get_event_source, BSTEventSource_WordUnlocked__Event__AddEventSink, BSTEventSource_WordUnlocked__Event__RemoveEventSink;
    }
}
