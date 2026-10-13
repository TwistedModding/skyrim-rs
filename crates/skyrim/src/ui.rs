//! UI and Scaleform (subsystem pass 8): the UI singleton's menu table, stack and counters, the
//! message queue menus are shown and hidden through, [`IMenu`] and menus of a mod's own
//! ([`Menu`]), the Scaleform movie view ([`Movie`]) and value model ([`Value`]), native
//! `ActionScript` callbacks ([`Function`]) and the HUD notification path.
//!
//! Threads: the queue is drained, menus are advanced and drawn by the "UI" job or the main
//! thread. Queue messages from the main thread: the 64 reusable messages are counted without a
//! lock. Reading the stack takes the engine's recursive lock, which the draw holds, so a detour on
//! `RenderMenuStack` may read it freely. Scaleform calls belong on the UI step: inside a menu's
//! own slots or a detour there.

use core::ffi::{CStr, c_char, c_void};
use core::mem::MaybeUninit;
use core::ops::Deref;
use std::ffi::CString;
use std::sync::atomic::{AtomicU32, Ordering};

use layout::v1_7_104::{classes, enums, fns, globals, vtables};
use windows_sys::Win32::System::Threading::{GetCurrentThreadId, Sleep};

pub use crate::events::EventSourceHolderMenuOpenClose as Ui;
use crate::string::BSFixedString;

pub use enums::IMenu_flags as MenuFlags;

pub type RenderMenuStackFn = fns::RenderMenuStack_fn;
pub type AddMessageFn = fns::UIMessageQueue__AddMessage_fn;
pub type ShowHudMessageFn = fns::ShowHUDMessage_fn;
/// A menu creator as `Register` takes it: no arguments, a new menu holding one reference.
pub type MenuCreator = unsafe extern "C" fn() -> *mut classes::IMenu;

type RegisterFn = unsafe extern "C" fn(this: *mut classes::EventSourceHolder_MenuOpenClose, name: *const c_char, create: Option<MenuCreator>);
/// `MenuTable::Find` (id 82135): the table is the UI singleton's `menuTable`; the out value is
/// `{menu (one reference added), create}`.
type FindFn = unsafe extern "C" fn(table: *mut c_void, name: *const classes::BSFixedString, out: *mut MenuTableValue) -> bool;
type LoadMovieFn = unsafe extern "C" fn(this: *mut classes::BSScaleformManager, menu: *mut classes::IMenu, out: *mut *mut classes::GFxMovieView, name: *const c_char, scale_mode: u32, alpha: f32) -> bool;
type AssignFn = unsafe extern "C" fn(this: *mut classes::GFxValue, other: *const classes::GFxValue) -> *mut classes::GFxValue;
type ObjectInvokeFn = unsafe extern "C" fn(this: *mut c_void, data: *mut c_void, result: *mut classes::GFxValue, name: *const c_char, args: *const classes::GFxValue, count: u32, is_display: bool) -> bool;
type ObjectGetMemberFn = unsafe extern "C" fn(this: *mut c_void, data: *mut c_void, name: *const c_char, out: *mut classes::GFxValue, is_display: bool) -> bool;
type ObjectSetMemberFn = unsafe extern "C" fn(this: *mut c_void, data: *mut c_void, name: *const c_char, value: *const classes::GFxValue, is_display: bool) -> bool;
type ObjectHasMemberFn = unsafe extern "C" fn(this: *mut c_void, data: *mut c_void, name: *const c_char, is_display: bool) -> bool;

type MenuVdtor = unsafe extern "C" fn(this: *mut classes::IMenu, flags: u32) -> *mut classes::IMenu;
type MenuProcessMessage = unsafe extern "C" fn(this: *mut classes::IMenu, message: *mut classes::UIMessage) -> u32;
type MenuAdvance = unsafe extern "C" fn(this: *mut classes::IMenu, interval: f32, time_ms: u32);
type MenuSlot = unsafe extern "C" fn(this: *mut classes::IMenu);

type MovieSetVisible = unsafe extern "C" fn(this: *mut classes::GFxMovieView, visible: bool);
type MovieGetVisible = unsafe extern "C" fn(this: *mut classes::GFxMovieView) -> bool;
type MovieCreateString = unsafe extern "C" fn(this: *mut classes::GFxMovieView, out: *mut classes::GFxValue, text: *const c_char);
type MovieCreateObject = unsafe extern "C" fn(this: *mut classes::GFxMovieView, out: *mut classes::GFxValue, class_name: *const c_char, args: *const classes::GFxValue, count: u32);
type MovieCreateArray = unsafe extern "C" fn(this: *mut classes::GFxMovieView, out: *mut classes::GFxValue);
type MovieCreateFunction = unsafe extern "C" fn(this: *mut classes::GFxMovieView, out: *mut classes::GFxValue, handler: *mut classes::GFxFunctionHandler, user_data: *mut c_void);
type MovieSetVariable = unsafe extern "C" fn(this: *mut classes::GFxMovieView, path: *const c_char, value: *const classes::GFxValue, set_type: i32) -> bool;
type MovieGetVariable = unsafe extern "C" fn(this: *mut classes::GFxMovieView, out: *mut classes::GFxValue, path: *const c_char) -> bool;
type MovieInvoke = unsafe extern "C" fn(this: *mut classes::GFxMovieView, path: *const c_char, result: *mut classes::GFxValue, args: *const classes::GFxValue, count: u32) -> bool;
type MovieRelease = unsafe extern "C" fn(this: *mut classes::GFxMovieView);

/// `GFxMovieView_vftable`: the one concrete movie-view vtable. It has no RTTI, so the ledger
/// carries it as a global and the generator emits no vtable for it.
const MOVIE_VIEW_VTABLE: u64 = 242_614;

#[repr(C)]
struct MenuTableValue {
    menu: *mut classes::IMenu,
    create: *const c_void,
}

pub(crate) struct Fns {
    queue: *const *mut classes::UIMessageQueue,
    stack_lock: *const AtomicU32,
    scaleform: *const *mut classes::BSScaleformManager,
    movie_vtable: usize,
    menu_vtable: [*const c_void; 9],
    register: RegisterFn,
    is_menu_open: fns::EventSourceHolder_MenuOpenClose__IsMenuOpen_fn,
    find: FindFn,
    unload: fns::EventSourceHolder_MenuOpenClose__UnloadMenu_fn,
    add_message: fns::UIMessageQueue__AddMessage_fn,
    show_hud_message: fns::ShowHUDMessage_fn,
    load_movie: LoadMovieFn,
    release_value: fns::GFxValue__ReleaseManaged_fn,
    assign: AssignFn,
    invoke: ObjectInvokeFn,
    get_member: ObjectGetMemberFn,
    set_member: ObjectSetMemberFn,
    has_member: ObjectHasMemberFn,
    render_menu_stack: usize,
}

unsafe impl Sync for Fns {}
unsafe impl Send for Fns {}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        let base_vtable = reloc.address(vtables::IMenu_vtable::ID)? as *const [*const c_void; 9];
        unsafe {
            Ok(Self {
                queue: reloc.address(globals::g_UIMessageQueue_ptr)? as *const *mut classes::UIMessageQueue,
                stack_lock: reloc.address(globals::g_UIMenuStackLock)? as *const AtomicU32,
                scaleform: reloc.address(globals::g_BSScaleformManager_ptr)? as *const *mut classes::BSScaleformManager,
                movie_vtable: reloc.address(MOVIE_VIEW_VTABLE)?,
                menu_vtable: *base_vtable,
                register: crate::resolve(reloc, fns::EventSourceHolder_MenuOpenClose__Register)?,
                is_menu_open: crate::resolve(reloc, fns::EventSourceHolder_MenuOpenClose__IsMenuOpen)?,
                find: crate::resolve(reloc, fns::MenuTable__Find)?,
                unload: crate::resolve(reloc, fns::EventSourceHolder_MenuOpenClose__UnloadMenu)?,
                add_message: crate::resolve(reloc, fns::UIMessageQueue__AddMessage)?,
                show_hud_message: crate::resolve(reloc, fns::ShowHUDMessage)?,
                load_movie: crate::resolve(reloc, fns::BSScaleformManager__LoadMovie)?,
                release_value: crate::resolve(reloc, fns::GFxValue__ReleaseManaged)?,
                assign: crate::resolve(reloc, fns::GFxValue__Assign)?,
                invoke: crate::resolve(reloc, fns::GFxValue__ObjectInterface__Invoke)?,
                get_member: crate::resolve(reloc, fns::GFxValue__ObjectInterface__GetMember)?,
                set_member: crate::resolve(reloc, fns::GFxValue__ObjectInterface__SetMember)?,
                has_member: crate::resolve(reloc, fns::GFxValue__ObjectInterface__HasMember)?,
                render_menu_stack: reloc.address(fns::RenderMenuStack)?,
            })
        }
    }
}

fn fns() -> &'static Fns {
    &crate::runtime().ui
}

unsafe fn slot<F: Copy>(vtable: *const *const c_void, index: usize) -> F {
    const { assert!(size_of::<F>() == size_of::<usize>(), "F must be a function pointer type") };
    unsafe { core::mem::transmute_copy::<*const c_void, F>(&*vtable.add(index)) }
}

fn c_string(text: &str) -> CString {
    CString::new(text.replace('\0', "")).expect("no interior nul left")
}

/// Creates, disabled, a detour on `RenderMenuStack` (id 82084), the draw of the open menus after
/// the world; the stack lock is held while it runs. Store it, then [`hook::Hook::enable`].
///
/// # Safety
/// `detour` must call the trampoline and return normally; it runs on the main or loading thread every frame.
pub unsafe fn hook_render_menu_stack(detour: RenderMenuStackFn) -> Result<hook::Hook<RenderMenuStackFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().render_menu_stack, detour) }
}

/// Creates, disabled, a detour on `UIMessageQueue::AddMessage` (id 13631); store it, then enable.
///
/// # Safety
/// As [`hook_render_menu_stack`]; called from several threads.
pub unsafe fn hook_add_message(detour: AddMessageFn) -> Result<hook::Hook<AddMessageFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().add_message as usize, detour) }
}

/// Creates, disabled, a detour on `ShowHUDMessage` (id 52933); store it, then enable.
///
/// # Safety
/// As [`hook_render_menu_stack`].
pub unsafe fn hook_show_hud_message(detour: ShowHudMessageFn) -> Result<hook::Hook<ShowHudMessageFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().show_hud_message as usize, detour) }
}

/// Queues a HUD notification (`ShowHUDMessage`, id 52933, the path behind `Debug.Notification`):
/// a `HUDData` to "HUD Menu" plus the named UI sound. With `suppress_repeats` a text shown in
/// the last five seconds is dropped.
pub fn show_hud_message(text: &str, sound: Option<&str>, suppress_repeats: bool) {
    let text = c_string(text);
    let sound = sound.map(c_string);
    unsafe { (fns().show_hud_message)(text.as_ptr(), sound.as_ref().map_or(core::ptr::null(), |s| s.as_ptr()), suppress_repeats) }
}

/// The kinds of `UIMessage` the queue carries, as the binary numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum MessageType {
    Update = 0,
    Show = 1,
    /// Show for a menu that is already open.
    Reshow = 2,
    Hide = 3,
    /// Hide and unload; the binary never queues it itself.
    ForceHide = 4,
    ScaleformEvent = 6,
    UserEvent = 7,
    InventoryUpdate = 8,
    Kinect = 0xC,
}

impl MessageType {
    #[must_use]
    pub const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Update,
            1 => Self::Show,
            2 => Self::Reshow,
            3 => Self::Hide,
            4 => Self::ForceHide,
            6 => Self::ScaleformEvent,
            7 => Self::UserEvent,
            8 => Self::InventoryUpdate,
            0xC => Self::Kinect,
            _ => return None,
        })
    }
}

/// What a menu's `ProcessMessage` answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageResult {
    /// Consumed.
    Handled,
    /// Refused: a show is dropped, a hide deferred.
    Refused,
    /// Passed to the next menu down the stack.
    Pass,
    /// Whatever `IMenu`'s own implementation answers (it hands Scaleform events to the movie, passes the rest).
    Default,
}

/// One queued message: the menu it is for, its kind and an optional payload.
#[repr(transparent)]
pub struct UiMessage(classes::UIMessage);

impl UiMessage {
    #[must_use]
    pub fn raw(&self) -> &classes::UIMessage {
        &self.0
    }

    #[must_use]
    pub fn menu_name(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr(&raw const self.0.menuName) }
    }

    #[must_use]
    pub fn kind_raw(&self) -> u32 {
        self.0.type_
    }

    #[must_use]
    pub fn kind(&self) -> Option<MessageType> {
        MessageType::from_raw(self.0.type_)
    }

    #[must_use]
    pub fn data(&self) -> *mut classes::IUIMessageData {
        self.0.data
    }
}

/// The UI message queue behind `g_UIMessageQueue_ptr` (id 400445): a spin-locked ring that
/// `ProcessMessages` drains once per frame.
#[repr(transparent)]
pub struct UiMessageQueue(classes::UIMessageQueue);

impl UiMessageQueue {
    /// `None` before the UI is constructed and after shutdown.
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().queue).cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::UIMessageQueue {
        &self.0
    }

    fn as_ptr(&self) -> *mut classes::UIMessageQueue {
        (&raw const self.0).cast_mut()
    }

    /// Queues a message for the menu registered under `menu` (`UIMessageQueue::AddMessage`,
    /// id 13631). `data` is null for show, hide and update.
    ///
    /// # Safety
    /// `data` must be null or a payload the queue may own and delete.
    pub unsafe fn add_message(&self, menu: &str, kind: MessageType, data: *mut classes::IUIMessageData) {
        let name = BSFixedString::new(menu);
        unsafe { (fns().add_message)(self.as_ptr(), name.raw(), kind as u32, data) }
    }

    pub fn show(&self, menu: &str) {
        unsafe { self.add_message(menu, MessageType::Show, core::ptr::null_mut()) }
    }

    pub fn hide(&self, menu: &str) {
        unsafe { self.add_message(menu, MessageType::Hide, core::ptr::null_mut()) }
    }

    /// Static `UIMessage` slots handed out since the last drain (64 at most).
    #[must_use]
    pub fn static_messages_used(&self) -> u32 {
        self.0.staticMessagesUsed
    }
}

/// A live menu (0x30 bytes of `IMenu`, then the subclass): its movie, depth, flags and class.
#[repr(transparent)]
pub struct IMenu(classes::IMenu);

impl IMenu {
    /// # Safety
    /// `p` must be null or point to a live `IMenu` for `'a`.
    #[must_use]
    pub unsafe fn from_ptr<'a>(p: *const classes::IMenu) -> Option<&'a Self> {
        unsafe { p.cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::IMenu {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *mut classes::IMenu {
        (&raw const self.0).cast_mut()
    }

    #[must_use]
    pub fn ref_count(&self) -> u32 {
        self.0.refCount
    }

    #[must_use]
    pub fn depth(&self) -> u8 {
        self.0.depth
    }

    #[must_use]
    pub fn flags(&self) -> MenuFlags {
        MenuFlags(self.0.flags)
    }

    /// Flag 0x40: on the stack.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.flags().contains(MenuFlags::IS_OPEN)
    }

    #[must_use]
    pub fn input_context(&self) -> u32 {
        self.0.inputContext
    }

    /// The Scaleform movie, `None` when it failed to load or the menu has none.
    #[must_use]
    pub fn movie(&self) -> Option<&Movie> {
        unsafe { Movie::from_ptr(self.0.movie) }
    }

    /// The ledger's vtable for this menu's class, `None` for a class the ledger has no vtable for.
    #[must_use]
    pub fn class(&self) -> Option<&'static layout::Vtable> {
        unsafe { crate::class_of(self.as_ptr().cast()) }
    }

    /// The class name without its `::vftable` suffix, `?` when unknown.
    #[must_use]
    pub fn class_name(&self) -> &'static str {
        self.class().map_or("?", |c| c.class.strip_suffix("::vftable").unwrap_or(c.class))
    }

    #[must_use]
    pub fn as_hud(&self) -> Option<&HudMenu> {
        (self.class()?.id == vtables::HUDMenu_vtable::ID).then(|| unsafe { &*self.as_ptr().cast::<HudMenu>() })
    }

    /// Drops one reference; at zero the menu's own destructor runs and frees it.
    unsafe fn release(menu: *mut classes::IMenu) {
        let count = unsafe { &*(&raw mut (*menu).refCount).cast::<AtomicU32>() };
        if count.fetch_sub(1, Ordering::AcqRel) == 1 {
            let dtor: MenuVdtor = unsafe { slot((*menu).__vftable.cast(), vtables::IMenu_vtable::SLOT_vdtor) };
            unsafe { dtor(menu, 1) };
        }
    }
}

/// The HUD (`HUDMenu`, 0x98 bytes): meters, the component list and the root `GFxValue` of its movie.
#[repr(transparent)]
pub struct HudMenu(classes::HUDMenu);

impl HudMenu {
    #[must_use]
    pub fn raw(&self) -> &classes::HUDMenu {
        &self.0
    }

    #[must_use]
    pub fn as_menu(&self) -> &IMenu {
        unsafe { &*(&raw const self.0).cast::<IMenu>() }
    }

    /// The `_root` display object of the HUD movie (a managed type 8 value); the HUD invokes `ShowMessage`, `SetPercent` and the compass calls on it.
    #[must_use]
    pub fn root(&self) -> &Value {
        unsafe { &*(&raw const self.0.root).cast::<Value>() }
    }

    #[must_use]
    pub fn component_count(&self) -> u32 {
        self.0.objects.size
    }
}

/// The open menus, read under the engine's recursive stack lock (`g_UIMenuStackLock`), which is
/// released on drop. Ascending depth: the HUD at 2, the cursor at 13.
pub struct MenuStack<'a> {
    menus: &'a [*mut classes::IMenu],
}

impl MenuStack<'_> {
    #[must_use]
    pub fn len(&self) -> usize {
        self.menus.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.menus.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &IMenu> {
        self.menus.iter().filter_map(|m| unsafe { IMenu::from_ptr(*m) })
    }
}

impl Drop for MenuStack<'_> {
    fn drop(&mut self) {
        unlock_stack();
    }
}

fn lock_stack() {
    let lock = unsafe { &*fns().stack_lock };
    let count = unsafe { &*fns().stack_lock.add(1) };
    let me = unsafe { GetCurrentThreadId() };
    let mut spins = 0u32;
    loop {
        if lock.load(Ordering::Acquire) == me {
            count.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if lock.compare_exchange(0, me, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            count.store(1, Ordering::Relaxed);
            return;
        }
        spins += 1;
        unsafe { Sleep(u32::from(spins > 10_000)) };
    }
}

fn unlock_stack() {
    let lock = unsafe { &*fns().stack_lock };
    let count = unsafe { &*fns().stack_lock.add(1) };
    if count.fetch_sub(1, Ordering::Relaxed) == 1 {
        lock.store(0, Ordering::Release);
    }
}

impl Ui {
    fn as_ptr(&self) -> *mut classes::EventSourceHolder_MenuOpenClose {
        (&raw const *self.raw()).cast_mut()
    }

    fn table(&self) -> *mut c_void {
        (&raw const self.raw().menuTable).cast_mut().cast()
    }

    /// The open menus in draw order, under the stack lock until the result is dropped.
    #[must_use]
    pub fn stack(&self) -> MenuStack<'_> {
        lock_stack();
        let ui = self.raw();
        let count = u32::from_le_bytes([ui.unknown_118[8], ui.unknown_118[9], ui.unknown_118[10], ui.unknown_118[11]]) as usize;
        let menus = if ui.menuStack.is_null() || count == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(ui.menuStack.cast::<*mut classes::IMenu>(), count) } };
        MenuStack { menus }
    }

    /// `IsMenuOpen` (id 82074): the menu is registered and its flag 0x40 is set.
    #[must_use]
    pub fn is_menu_open(&self, name: &str) -> bool {
        let name = BSFixedString::new(name);
        unsafe { (fns().is_menu_open)(self.as_ptr(), name.raw()) }
    }

    fn find(&self, name: &str) -> Option<MenuTableValue> {
        let name = BSFixedString::new(name);
        let mut value = MenuTableValue { menu: core::ptr::null_mut(), create: core::ptr::null() };
        unsafe { (fns().find)(self.table(), name.raw(), &raw mut value) }.then_some(value)
    }

    /// A name `Register` has seen (`MenuTable::Find`, id 82135).
    #[must_use]
    pub fn is_registered(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// The live menu registered under `name`: `None` when the name is unknown or the menu was
    /// never shown (or unloaded). The table keeps its own reference; the one the lookup adds is dropped here.
    #[must_use]
    pub fn menu(&self, name: &str) -> Option<&IMenu> {
        let value = self.find(name)?;
        if value.menu.is_null() {
            return None;
        }
        unsafe { IMenu::release(value.menu) };
        unsafe { IMenu::from_ptr(value.menu) }
    }

    #[must_use]
    pub fn hud(&self) -> Option<&HudMenu> {
        self.menu("HUD Menu")?.as_hud()
    }

    /// Registers (or replaces) the creator for `name` (`Register`, id 82086): the engine calls
    /// [`Menu::create`] when the menu is first shown. The table may rehash, so register at load
    /// or from the UI step.
    pub fn register<M: Menu>(&self, name: &str) {
        let name = c_string(name);
        unsafe { (fns().register)(self.as_ptr(), name.as_ptr(), Some(create_menu::<M>)) }
    }

    /// Drops the live menu from the table, keeping its creator (`UnloadMenu`, id 82095).
    pub fn unload_menu(&self, name: &str) {
        let name = BSFixedString::new(name);
        unsafe { (fns().unload)(self.as_ptr(), name.raw()) }
    }

    #[must_use]
    pub fn num_inventory_menus(&self) -> u32 {
        self.raw().numInventoryMenus
    }

    /// Open menus with flag 0x800, which allow saving.
    #[must_use]
    pub fn num_menus_allowing_save(&self) -> u32 {
        self.raw().numMenusAllowingSave
    }

    #[must_use]
    pub fn modal_menu_open(&self) -> bool {
        self.raw().modalMenuOpen != 0
    }

    /// Cleared by the `ToggleMenus` console command; `RenderMenuStack` draws nothing while it is clear.
    #[must_use]
    pub fn menus_visible(&self) -> bool {
        self.raw().menusVisible != 0
    }

    #[must_use]
    pub fn closing_all_menus(&self) -> bool {
        self.raw().closingAllMenus != 0
    }
}

/// How `LoadMovie` fits a movie to the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ScaleMode {
    NoScale = 0,
    ShowAll = 1,
    ExactFit = 2,
    NoBorder = 3,
}

/// The `IMenu` part of a menu of this mod's own, handed to the [`Menu`] callbacks.
#[repr(transparent)]
pub struct MenuBase(classes::IMenu);

impl Deref for MenuBase {
    type Target = IMenu;

    fn deref(&self) -> &IMenu {
        unsafe { &*(&raw const self.0).cast::<IMenu>() }
    }
}

impl MenuBase {
    /// Draw and message priority; the stack is kept sorted by it. 3 by default.
    pub fn set_depth(&mut self, depth: u8) {
        self.0.depth = depth;
    }

    pub fn set_flags(&mut self, flags: MenuFlags) {
        self.0.flags = flags.0;
    }

    /// The input context pushed while the menu is open; 0x13 is none.
    pub fn set_input_context(&mut self, context: u32) {
        self.0.inputContext = context;
    }

    /// Loads `Interface/<name>.swf` (or `Interface/Exported/<name>.gfx`) into this menu
    /// (`BSScaleformManager::LoadMovie`, id 82325), creating the menu's `FxDelegate` on the way.
    /// `false` when the file is missing; the movie stays `None`.
    pub fn load_movie(&mut self, name: &str, scale_mode: ScaleMode, background_alpha: f32) -> bool {
        let manager = unsafe { *fns().scaleform };
        if manager.is_null() {
            return false;
        }
        let name = c_string(name);
        unsafe { (fns().load_movie)(manager, &raw mut self.0, &raw mut self.0.movie, name.as_ptr(), scale_mode as u32, background_alpha) }
    }

    /// `IMenu`'s own advance: sets `CurrentTime` on the movie and advances it. Nothing without a movie.
    pub fn advance_movie(&mut self, interval: f32, time_ms: u32) {
        if !self.0.movie.is_null() {
            let f: MenuAdvance = unsafe { slot(fns().menu_vtable.as_ptr(), vtables::IMenu_vtable::SLOT_AdvanceMovie) };
            unsafe { f(&raw mut self.0, interval, time_ms) }
        }
    }

    /// `IMenu`'s own draw: the movie's `Display`. Nothing without a movie.
    pub fn display_movie(&mut self) {
        if !self.0.movie.is_null() {
            let f: MenuSlot = unsafe { slot(fns().menu_vtable.as_ptr(), vtables::IMenu_vtable::SLOT_PostDisplay) };
            unsafe { f(&raw mut self.0) }
        }
    }
}

/// A menu of this mod's own. Register the type under a name with [`Ui::register`]; the engine
/// creates it when the name is first shown, advances and draws it in depth order, delivers its
/// messages, and destroys it when it is hidden (unless `KEEP_LOADED` is set).
pub trait Menu: Sized + 'static {
    /// Builds the menu: set the depth, flags and input context on `base`, load a movie if there is one.
    fn create(base: &mut MenuBase) -> Self;

    /// Right after `create`, before the menu is stored in the table.
    fn post_create(&mut self, _base: &mut MenuBase) {}

    /// Every message `ProcessMessages` delivers to this menu: show (1) first, then hide (3), the
    /// per-frame update (0) and events.
    fn process_message(&mut self, _base: &mut MenuBase, _message: &UiMessage) -> MessageResult {
        MessageResult::Default
    }

    /// Per frame while open, from `AdvanceMenus`.
    fn advance(&mut self, base: &mut MenuBase, interval: f32, time_ms: u32) {
        base.advance_movie(interval, time_ms);
    }

    /// Per frame while open, from `RenderMenuStack`, back to front.
    fn display(&mut self, base: &mut MenuBase) {
        base.display_movie();
    }
}

#[repr(C)]
struct MenuObject<M: Menu> {
    base: classes::IMenu,
    vtable: [*const c_void; 9],
    menu: MaybeUninit<M>,
}

impl<M: Menu> MenuObject<M> {
    unsafe fn parts<'a>(this: *mut classes::IMenu) -> (&'a mut MenuBase, &'a mut M) {
        let object = unsafe { &mut *this.cast::<Self>() };
        (unsafe { &mut *(&raw mut object.base).cast::<MenuBase>() }, unsafe { object.menu.assume_init_mut() })
    }
}

unsafe extern "C" fn create_menu<M: Menu>() -> *mut classes::IMenu {
    let engine = &fns().menu_vtable;
    let mut object = Box::new(MenuObject::<M> {
        base: classes::IMenu {
            __vftable: core::ptr::null(),
            refCount: 1,
            unknown_C: [0; 4],
            movie: core::ptr::null_mut(),
            depth: 3,
            unknown_19: [0; 3],
            flags: 0,
            inputContext: 0x13,
            field_24: 0,
            fxDelegate: core::ptr::null_mut(),
        },
        vtable: [
            menu_dtor::<M> as MenuVdtor as *const c_void,
            engine[vtables::IMenu_vtable::SLOT_Accept],
            menu_post_create::<M> as MenuSlot as *const c_void,
            engine[vtables::IMenu_vtable::SLOT_Refresh],
            menu_process_message::<M> as MenuProcessMessage as *const c_void,
            menu_advance::<M> as MenuAdvance as *const c_void,
            menu_display::<M> as MenuSlot as *const c_void,
            engine[vtables::IMenu_vtable::SLOT_PreDisplay],
            menu_refresh_platform as MenuSlot as *const c_void,
        ],
        menu: MaybeUninit::uninit(),
    });
    object.base.__vftable = object.vtable.as_ptr().cast();
    let menu = M::create(unsafe { &mut *(&raw mut object.base).cast::<MenuBase>() });
    object.menu.write(menu);
    Box::into_raw(object).cast()
}

// The engine's own IMenu destructor without its free (bit 0 clear) releases the movie and the
// delegate; the object itself is a Box.
unsafe extern "C" fn menu_dtor<M: Menu>(this: *mut classes::IMenu, flags: u32) -> *mut classes::IMenu {
    let object = this.cast::<MenuObject<M>>();
    unsafe { (*object).menu.assume_init_drop() };
    let base: MenuVdtor = unsafe { slot(fns().menu_vtable.as_ptr(), vtables::IMenu_vtable::SLOT_vdtor) };
    unsafe { base(this, flags & !1) };
    if flags & 1 != 0 {
        drop(unsafe { Box::from_raw(object) });
    }
    this
}

unsafe extern "C" fn menu_post_create<M: Menu>(this: *mut classes::IMenu) {
    let (base, menu) = unsafe { MenuObject::<M>::parts(this) };
    menu.post_create(base);
}

unsafe extern "C" fn menu_process_message<M: Menu>(this: *mut classes::IMenu, message: *mut classes::UIMessage) -> u32 {
    let (base, menu) = unsafe { MenuObject::<M>::parts(this) };
    let Some(msg) = (unsafe { message.cast::<UiMessage>().as_ref() }) else { return 2 };
    match menu.process_message(base, msg) {
        MessageResult::Handled => 0,
        MessageResult::Refused => 1,
        MessageResult::Pass => 2,
        MessageResult::Default => {
            let f: MenuProcessMessage = unsafe { slot(fns().menu_vtable.as_ptr(), vtables::IMenu_vtable::SLOT_ProcessMessage) };
            unsafe { f(this, message) }
        }
    }
}

unsafe extern "C" fn menu_advance<M: Menu>(this: *mut classes::IMenu, interval: f32, time_ms: u32) {
    let (base, menu) = unsafe { MenuObject::<M>::parts(this) };
    menu.advance(base, interval, time_ms);
}

unsafe extern "C" fn menu_display<M: Menu>(this: *mut classes::IMenu) {
    let (base, menu) = unsafe { MenuObject::<M>::parts(this) };
    menu.display(base);
}

unsafe extern "C" fn menu_refresh_platform(this: *mut classes::IMenu) {
    if unsafe { !(*this).movie.is_null() } {
        let f: MenuSlot = unsafe { slot(fns().menu_vtable.as_ptr(), vtables::IMenu_vtable::SLOT_RefreshPlatform) };
        unsafe { f(this) }
    }
}

/// A Scaleform movie view (`GFxMovieView`, 0x2BF0 bytes): the object a menu's `movie` points at.
#[repr(transparent)]
pub struct Movie(classes::GFxMovieView);

impl Movie {
    /// # Safety
    /// `p` must be null or point to a live movie view for `'a`.
    #[must_use]
    pub unsafe fn from_ptr<'a>(p: *const classes::GFxMovieView) -> Option<&'a Self> {
        unsafe { p.cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::GFxMovieView {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *mut classes::GFxMovieView {
        (&raw const self.0).cast_mut()
    }

    fn vtable(&self) -> *const *const c_void {
        unsafe { *(&raw const self.0).cast::<*const *const c_void>() }
    }

    /// The view carries the engine's one concrete vtable (`GFxMovieView_vftable`, id 242614).
    #[must_use]
    pub fn is_engine_view(&self) -> bool {
        self.vtable() as usize == fns().movie_vtable
    }

    #[must_use]
    pub fn ref_count(&self) -> u32 {
        self.0.refCount
    }

    #[must_use]
    pub fn visible(&self) -> bool {
        let f: MovieGetVisible = unsafe { slot(self.vtable(), 9) };
        unsafe { f(self.as_ptr()) }
    }

    pub fn set_visible(&self, visible: bool) {
        let f: MovieSetVisible = unsafe { slot(self.vtable(), 8) };
        unsafe { f(self.as_ptr(), visible) }
    }

    /// A managed string value owned by this movie.
    #[must_use]
    pub fn create_string(&self, text: &str) -> Value {
        let text = c_string(text);
        let f: MovieCreateString = unsafe { slot(self.vtable(), 11) };
        let mut out = Value::undefined();
        unsafe { f(self.as_ptr(), out.as_mut_ptr(), text.as_ptr()) };
        out
    }

    /// A new plain `ActionScript` object.
    #[must_use]
    pub fn create_object(&self) -> Value {
        let f: MovieCreateObject = unsafe { slot(self.vtable(), 13) };
        let mut out = Value::undefined();
        unsafe { f(self.as_ptr(), out.as_mut_ptr(), core::ptr::null(), core::ptr::null(), 0) };
        out
    }

    #[must_use]
    pub fn create_array(&self) -> Value {
        let f: MovieCreateArray = unsafe { slot(self.vtable(), 14) };
        let mut out = Value::undefined();
        unsafe { f(self.as_ptr(), out.as_mut_ptr()) };
        out
    }

    /// An `ActionScript` function that calls `function` (a `'static` [`Function`]); set it as a
    /// member of `_root` or another object to let the movie call native code.
    #[must_use]
    pub fn create_function<H: FunctionHandler>(&self, function: &'static Function<H>) -> Value {
        let f: MovieCreateFunction = unsafe { slot(self.vtable(), 15) };
        let mut out = Value::undefined();
        unsafe { f(self.as_ptr(), out.as_mut_ptr(), function.as_ptr(), core::ptr::null_mut()) };
        out
    }

    /// Reads the variable at `path` (`_root.foo.bar`); `None` when it does not resolve.
    #[must_use]
    pub fn get_variable(&self, path: &str) -> Option<Value> {
        let path = c_string(path);
        let f: MovieGetVariable = unsafe { slot(self.vtable(), 17) };
        let mut out = Value::undefined();
        unsafe { f(self.as_ptr(), out.as_mut_ptr(), path.as_ptr()) }.then_some(out)
    }

    #[must_use]
    pub fn set_variable(&self, path: &str, value: &Value) -> bool {
        let path = c_string(path);
        let f: MovieSetVariable = unsafe { slot(self.vtable(), 16) };
        unsafe { f(self.as_ptr(), path.as_ptr(), value.as_ptr(), 0) }
    }

    /// Calls the method at `path` with `args`; `None` when the path does not resolve.
    #[must_use]
    pub fn invoke(&self, path: &str, args: &[Value]) -> Option<Value> {
        let path = c_string(path);
        let f: MovieInvoke = unsafe { slot(self.vtable(), 23) };
        let mut out = Value::undefined();
        let count = u32::try_from(args.len()).expect("argument count");
        unsafe { f(self.as_ptr(), path.as_ptr(), out.as_mut_ptr(), args.as_ptr().cast(), count) }.then_some(out)
    }

    /// Drops one reference; at zero the view is destroyed.
    ///
    /// # Safety
    /// Only for a reference the caller owns (one `LoadMovie` handed out and the menu no longer holds).
    pub unsafe fn release(&self) {
        let f: MovieRelease = unsafe { slot(self.vtable(), 69) };
        unsafe { f(self.as_ptr()) }
    }
}

/// The type tag in the low nibble of `GFxValue::type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ValueType {
    Undefined = 0,
    Null = 1,
    Bool = 2,
    Number = 3,
    String = 4,
    WideString = 5,
    Object = 6,
    Array = 7,
    DisplayObject = 8,
}

const VALUE_MANAGED: u32 = 0x40;
const VALUE_TYPE_MASK: u32 = 0xF;

/// A `GFxValue` (0x18 bytes): the value passed to and read from Scaleform. A managed value
/// (bit 0x40) keeps its object alive and is released on drop; release every value before its movie.
#[repr(transparent)]
pub struct Value(classes::GFxValue);

impl Value {
    #[must_use]
    pub const fn undefined() -> Self {
        Self(classes::GFxValue { objectInterface: core::ptr::null_mut(), type_: 0, unknown_C: [0; 4], data: 0 })
    }

    #[must_use]
    pub const fn null() -> Self {
        Self(classes::GFxValue { objectInterface: core::ptr::null_mut(), type_: 1, unknown_C: [0; 4], data: 0 })
    }

    #[must_use]
    pub const fn boolean(value: bool) -> Self {
        Self(classes::GFxValue { objectInterface: core::ptr::null_mut(), type_: 2, unknown_C: [0; 4], data: value as u64 })
    }

    #[must_use]
    pub fn number(value: f64) -> Self {
        Self(classes::GFxValue { objectInterface: core::ptr::null_mut(), type_: 3, unknown_C: [0; 4], data: value.to_bits() })
    }

    /// An unmanaged string value over `text`, which must outlive every use of the value.
    #[must_use]
    pub fn string(text: &'static CStr) -> Self {
        Self(classes::GFxValue { objectInterface: core::ptr::null_mut(), type_: 4, unknown_C: [0; 4], data: text.as_ptr() as u64 })
    }

    #[must_use]
    pub fn raw(&self) -> &classes::GFxValue {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const classes::GFxValue {
        &raw const self.0
    }

    fn as_mut_ptr(&mut self) -> *mut classes::GFxValue {
        &raw mut self.0
    }

    #[must_use]
    pub fn type_raw(&self) -> u32 {
        self.0.type_
    }

    #[must_use]
    pub fn value_type(&self) -> Option<ValueType> {
        Some(match self.0.type_ & VALUE_TYPE_MASK {
            0 => ValueType::Undefined,
            1 => ValueType::Null,
            2 => ValueType::Bool,
            3 => ValueType::Number,
            4 => ValueType::String,
            5 => ValueType::WideString,
            6 => ValueType::Object,
            7 => ValueType::Array,
            8 => ValueType::DisplayObject,
            _ => return None,
        })
    }

    #[must_use]
    pub fn is_managed(&self) -> bool {
        self.0.type_ & VALUE_MANAGED != 0
    }

    #[must_use]
    pub fn is_object(&self) -> bool {
        matches!(self.value_type(), Some(ValueType::Object | ValueType::Array | ValueType::DisplayObject))
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        (self.value_type() == Some(ValueType::Bool)).then_some(self.0.data & 0xFF != 0)
    }

    #[must_use]
    pub fn as_number(&self) -> Option<f64> {
        (self.value_type() == Some(ValueType::Number)).then_some(f64::from_bits(self.0.data))
    }

    /// The text of an unmanaged string value (a managed one holds a string node, not the characters).
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        if self.value_type() != Some(ValueType::String) || self.is_managed() || self.0.data == 0 {
            return None;
        }
        unsafe { CStr::from_ptr(self.0.data as *const c_char) }.to_str().ok()
    }

    fn object_call<R>(&self, call: impl FnOnce(*mut c_void, *mut c_void, bool) -> R) -> Option<R> {
        if !self.is_object() || self.0.objectInterface.is_null() {
            return None;
        }
        Some(call(self.0.objectInterface, self.0.data as *mut c_void, self.value_type() == Some(ValueType::DisplayObject)))
    }

    /// A member of an object, array or display object; `None` when it does not exist.
    #[must_use]
    pub fn get_member(&self, name: &str) -> Option<Value> {
        let name = c_string(name);
        let mut out = Value::undefined();
        self.object_call(|iface, data, display| unsafe { (fns().get_member)(iface, data, name.as_ptr(), out.as_mut_ptr(), display) })?.then_some(out)
    }

    #[must_use]
    pub fn set_member(&self, name: &str, value: &Value) -> bool {
        let name = c_string(name);
        self.object_call(|iface, data, display| unsafe { (fns().set_member)(iface, data, name.as_ptr(), value.as_ptr(), display) }) == Some(true)
    }

    #[must_use]
    pub fn has_member(&self, name: &str) -> bool {
        let name = c_string(name);
        self.object_call(|iface, data, display| unsafe { (fns().has_member)(iface, data, name.as_ptr(), display) }) == Some(true)
    }

    /// Calls the method `name` of this object (`GFxValue::ObjectInterface::Invoke`, id 82256).
    #[must_use]
    pub fn invoke(&self, name: &str, args: &[Value]) -> Option<Value> {
        let name = c_string(name);
        let count = u32::try_from(args.len()).expect("argument count");
        let mut out = Value::undefined();
        self.object_call(|iface, data, display| unsafe { (fns().invoke)(iface, data, out.as_mut_ptr(), name.as_ptr(), args.as_ptr().cast(), count, display) })?.then_some(out)
    }

    /// Copies `other` into this value (`GFxValue::Assign`, id 33582: add-ref and release as needed).
    pub fn assign(&mut self, other: &Value) {
        unsafe { (fns().assign)(self.as_mut_ptr(), other.as_ptr()) };
    }
}

impl Drop for Value {
    fn drop(&mut self) {
        if self.is_managed() {
            unsafe { (fns().release_value)(self.as_mut_ptr()) }
        }
    }
}

impl Clone for Value {
    fn clone(&self) -> Self {
        let mut out = Self::undefined();
        out.assign(self);
        out
    }
}

impl core::fmt::Debug for Value {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.value_type() {
            Some(ValueType::Bool) => write!(f, "{}", self.as_bool().unwrap_or(false)),
            Some(ValueType::Number) => write!(f, "{}", self.as_number().unwrap_or(0.0)),
            Some(ValueType::String) => write!(f, "{:?}", self.as_str().unwrap_or("<managed string>")),
            Some(t) => write!(f, "{t:?}{}", if self.is_managed() { " (managed)" } else { "" }),
            None => write!(f, "GFxValue type {:#x}", self.0.type_),
        }
    }
}

/// The arguments of a native callback: `this`, the arguments, the movie and the return slot.
#[repr(transparent)]
pub struct Params(classes::GFxFunctionHandler__Params);

impl Params {
    #[must_use]
    pub fn raw(&self) -> &classes::GFxFunctionHandler__Params {
        &self.0
    }

    #[must_use]
    pub fn args(&self) -> &[Value] {
        if self.0.args.is_null() || self.0.argCount == 0 {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(self.0.args.cast::<Value>(), self.0.argCount as usize) }
    }

    #[must_use]
    pub fn this_value(&self) -> Option<&Value> {
        unsafe { self.0.thisValue.cast::<Value>().as_ref() }
    }

    #[must_use]
    pub fn movie(&self) -> Option<&Movie> {
        unsafe { Movie::from_ptr(self.0.movie) }
    }

    pub fn set_return(&mut self, value: &Value) {
        if let Some(ret) = unsafe { self.0.retVal.cast::<Value>().as_mut() } {
            ret.assign(value);
        }
    }
}

/// What a native `ActionScript` callback does.
pub trait FunctionHandler: Sync + 'static {
    fn call(&self, params: &mut Params);
}

#[repr(C)]
struct FunctionVtable<H: FunctionHandler> {
    destroy: unsafe extern "C" fn(this: *mut Function<H>, flags: u32) -> *mut Function<H>,
    call: unsafe extern "C" fn(this: *mut Function<H>, params: *mut classes::GFxFunctionHandler__Params),
}

/// The `GFxFunctionHandler` object Scaleform calls (vtable, reference count, then the handler);
/// a `static` whose count starts at one, so the movie never destroys it.
#[repr(C)]
pub struct Function<H: FunctionHandler> {
    vtable: &'static FunctionVtable<H>,
    ref_count: AtomicU32,
    _pad: u32,
    handler: H,
}

impl<H: FunctionHandler> Function<H> {
    const VTABLE: &'static FunctionVtable<H> = &FunctionVtable { destroy: Self::destroy, call: Self::call };

    #[must_use]
    pub const fn new(handler: H) -> Self {
        Self { vtable: Self::VTABLE, ref_count: AtomicU32::new(1), _pad: 0, handler }
    }

    #[must_use]
    pub fn handler(&self) -> &H {
        &self.handler
    }

    #[must_use]
    pub fn ref_count(&self) -> u32 {
        self.ref_count.load(Ordering::Relaxed)
    }

    fn as_ptr(&'static self) -> *mut classes::GFxFunctionHandler {
        core::ptr::from_ref(self).cast_mut().cast()
    }

    unsafe extern "C" fn destroy(this: *mut Self, _flags: u32) -> *mut Self {
        this
    }

    unsafe extern "C" fn call(this: *mut Self, params: *mut classes::GFxFunctionHandler__Params) {
        if let (Some(this), Some(params)) = (unsafe { this.as_ref() }, unsafe { params.cast::<Params>().as_mut() }) {
            this.handler.call(params);
        }
    }
}

const _: () = assert!(core::mem::offset_of!(Function<Noop>, ref_count) == 8);

struct Noop;

impl FunctionHandler for Noop {
    fn call(&self, _: &mut Params) {}
}
