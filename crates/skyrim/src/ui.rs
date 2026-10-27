//! UI and Scaleform (subsystem pass 8): menus of a mod's own ([`Menu`]), native `ActionScript`
//! callbacks ([`Function`]), the owned Scaleform value ([`Value`]), the menu stack read under the
//! engine's lock, the HUD notification and the hooks on the menu draw and the message queue, and
//! what the generated UI singleton, message queue and movie view leave `unsafe`: lookups by `&str`,
//! a show or hide without a payload, and the movie view's slots (it has no RTTI, so no generated vtable).
//!
//! Threads: the queue is drained, menus are advanced and drawn by the "UI" job or the main
//! thread. Queue messages from the main thread: the 64 reusable messages are counted without a
//! lock. Reading the stack takes the engine's recursive lock, which the draw holds, so a detour on
//! `RenderMenuStack` may read it freely. Scaleform calls belong on the UI step: inside a menu's
//! own slots or a detour there.

use core::ffi::{CStr, c_char, c_void};
use core::mem::MaybeUninit;
use std::ffi::CString;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use sre::raw::{fns, vtables};
use sre::{EventSourceHolder_MenuOpenClose, GFxMovieView, HUDMenu, IMenu, UIMessage, UIMessageQueue};

use crate::BSFixedString;
use crate::SpinGuard;

pub type RenderMenuStackFn = fns::RenderMenuStack_fn;
pub type AddMessageFn = fns::UIMessageQueue__AddMessage_fn;
pub type ShowHudMessageFn = fns::ShowHUDMessage_fn;

/// `MenuTable::Find` (id 82135): the table is the UI singleton's `menuTable`; the out value is
/// `{menu (one reference added), create}`.
type FindFn = unsafe extern "C" fn(table: *mut c_void, name: *const sre::BSFixedString, out: *mut MenuTableValue) -> bool;
type LoadMovieFn = unsafe extern "C" fn(this: *mut sre::BSScaleformManager, menu: *mut IMenu, out: *mut *mut GFxMovieView, name: *const c_char, scale_mode: u32, alpha: f32) -> bool;
type ObjectInvokeFn = unsafe extern "C" fn(this: *mut c_void, data: *mut c_void, result: *mut sre::GFxValue, name: *const c_char, args: *const sre::GFxValue, count: u32, is_display: bool) -> bool;
type ObjectGetMemberFn = unsafe extern "C" fn(this: *mut c_void, data: *mut c_void, name: *const c_char, out: *mut sre::GFxValue, is_display: bool) -> bool;
type ObjectSetMemberFn = unsafe extern "C" fn(this: *mut c_void, data: *mut c_void, name: *const c_char, value: *const sre::GFxValue, is_display: bool) -> bool;
type ObjectHasMemberFn = unsafe extern "C" fn(this: *mut c_void, data: *mut c_void, name: *const c_char, is_display: bool) -> bool;

type MenuVdtor = unsafe extern "C" fn(this: *mut IMenu, flags: u32) -> *mut IMenu;
type MenuProcessMessage = unsafe extern "C" fn(this: *mut IMenu, message: *mut UIMessage) -> u32;
type MenuAdvance = unsafe extern "C" fn(this: *mut IMenu, interval: f32, time_ms: u32);
type MenuSlot = unsafe extern "C" fn(this: *mut IMenu);

type MovieSetVisible = unsafe extern "C" fn(this: *mut GFxMovieView, visible: bool);
type MovieGetVisible = unsafe extern "C" fn(this: *mut GFxMovieView) -> bool;
type MovieCreateString = unsafe extern "C" fn(this: *mut GFxMovieView, out: *mut sre::GFxValue, text: *const c_char);
type MovieCreateObject = unsafe extern "C" fn(this: *mut GFxMovieView, out: *mut sre::GFxValue, class_name: *const c_char, args: *const sre::GFxValue, count: u32);
type MovieCreateArray = unsafe extern "C" fn(this: *mut GFxMovieView, out: *mut sre::GFxValue);
type MovieCreateFunction = unsafe extern "C" fn(this: *mut GFxMovieView, out: *mut sre::GFxValue, handler: *mut sre::GFxFunctionHandler, user_data: *mut c_void);
type MovieSetVariable = unsafe extern "C" fn(this: *mut GFxMovieView, path: *const c_char, value: *const sre::GFxValue, set_type: i32) -> bool;
type MovieGetVariable = unsafe extern "C" fn(this: *mut GFxMovieView, out: *mut sre::GFxValue, path: *const c_char) -> bool;
type MovieInvoke = unsafe extern "C" fn(this: *mut GFxMovieView, path: *const c_char, result: *mut sre::GFxValue, args: *const sre::GFxValue, count: u32) -> bool;
type MovieRelease = unsafe extern "C" fn(this: *mut GFxMovieView);

/// `GFxMovieView_vftable`: the one concrete movie-view vtable. It has no RTTI, so the ledger
/// carries it as a global and the generator emits no vtable for it.
const MOVIE_VIEW_VTABLE: u64 = 242_614;

#[repr(C)]
struct MenuTableValue {
    menu: *mut IMenu,
    create: *const c_void,
}

/// The engine's own `IMenu` vtable, whose slots a mod's menu forwards to.
struct MenuVtable([*const c_void; 9]);

unsafe impl Send for MenuVtable {}
unsafe impl Sync for MenuVtable {}

fn engine_menu_vtable() -> &'static [*const c_void; 9] {
    static VTABLE: OnceLock<MenuVtable> = OnceLock::new();
    &VTABLE
        .get_or_init(|| {
            let base = crate::reloc().address(vtables::IMenu_vtable::ID).expect("IMenu vtable id") as *const [*const c_void; 9];
            MenuVtable(unsafe { *base })
        })
        .0
}

unsafe fn slot<F: Copy>(vtable: *const *const c_void, index: usize) -> F {
    const { assert!(size_of::<F>() == size_of::<usize>(), "F must be a function pointer type") };
    unsafe { core::mem::transmute_copy::<*const c_void, F>(&*vtable.add(index)) }
}

fn c_string(text: &str) -> CString {
    CString::new(text.replace('\0', "")).expect("no interior nul left")
}

fn create<F: Copy>(id: u64, detour: F) -> Result<hook::Hook<F>, hook::Error> {
    unsafe { hook::Hook::create(crate::reloc().address(id)?, detour) }
}

/// Creates, disabled, a detour on `RenderMenuStack` (id 82084), the draw of the open menus after
/// the world; the stack lock is held while it runs. Store it, then [`hook::Hook::enable`].
///
/// # Safety
/// `detour` must call the trampoline and return normally; it runs on the main or loading thread every frame.
pub unsafe fn hook_render_menu_stack(detour: RenderMenuStackFn) -> Result<hook::Hook<RenderMenuStackFn>, hook::Error> {
    create(fns::RenderMenuStack, detour)
}

/// Creates, disabled, a detour on `UIMessageQueue::AddMessage` (id 13631); store it, then enable.
///
/// # Safety
/// As [`hook_render_menu_stack`]; called from several threads.
pub unsafe fn hook_add_message(detour: AddMessageFn) -> Result<hook::Hook<AddMessageFn>, hook::Error> {
    create(fns::UIMessageQueue__AddMessage, detour)
}

/// Creates, disabled, a detour on `ShowHUDMessage` (id 52933); store it, then enable.
///
/// # Safety
/// As [`hook_render_menu_stack`].
pub unsafe fn hook_show_hud_message(detour: ShowHudMessageFn) -> Result<hook::Hook<ShowHudMessageFn>, hook::Error> {
    create(fns::ShowHUDMessage, detour)
}

/// Queues a HUD notification (`ShowHUDMessage`, id 52933, the path behind `Debug.Notification`):
/// a `HUDData` to "HUD Menu" plus the named UI sound. With `suppress_repeats` a text shown in
/// the last five seconds is dropped.
pub fn show_hud_message(text: &str, sound: Option<&str>, suppress_repeats: bool) {
    let text = c_string(text);
    let sound = sound.map(c_string);
    unsafe { sre::show_hud_message(text.as_ptr(), sound.as_ref().map_or(core::ptr::null(), |s| s.as_ptr()), suppress_repeats) }
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

pub trait UIMessageQueueExt {
    /// Queues a message for the menu registered under `menu` (`UIMessageQueue::AddMessage`,
    /// id 13631). `data` is null for show, hide and update.
    ///
    /// # Safety
    /// `data` must be null or a payload the queue may own and delete.
    unsafe fn queue(&self, menu: &str, kind: MessageType, data: *mut sre::IUIMessageData);
    fn show(&self, menu: &str);
    fn hide(&self, menu: &str);
}

impl UIMessageQueueExt for UIMessageQueue {
    unsafe fn queue(&self, menu: &str, kind: MessageType, data: *mut sre::IUIMessageData) {
        let name = BSFixedString::new(menu);
        self.add_message(name.raw(), kind as u32, unsafe { data.as_ref() });
    }

    fn show(&self, menu: &str) {
        unsafe { self.queue(menu, MessageType::Show, core::ptr::null_mut()) }
    }

    fn hide(&self, menu: &str) {
        unsafe { self.queue(menu, MessageType::Hide, core::ptr::null_mut()) }
    }
}

/// Drops one reference; at zero the menu's own destructor runs and frees it.
unsafe fn release_menu(menu: *mut IMenu) {
    let count = unsafe { AtomicU32::from_ptr(&raw mut (*menu).refCount) };
    if count.fetch_sub(1, Ordering::AcqRel) == 1 {
        let dtor: MenuVdtor = unsafe { slot((*menu).__vftable.cast(), vtables::IMenu_vtable::SLOT_vdtor) };
        unsafe { dtor(menu, 1) };
    }
}

/// The open menus, read under the engine's recursive stack lock (`g_UIMenuStackLock`), which is
/// released on drop. Ascending depth: the HUD at 2, the cursor at 13.
pub struct MenuStack<'a> {
    menus: &'a [*mut IMenu],
    _lock: SpinGuard,
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
        self.menus.iter().filter_map(|m| unsafe { m.as_ref() })
    }
}

/// What the generated UI singleton (`EventSourceHolder_MenuOpenClose`) leaves raw: the stack under
/// its lock, the menu table by `&str`, and the registration of a mod's own [`Menu`].
pub trait MenuManagerExt {
    /// The open menus in draw order, under the stack lock until the result is dropped.
    fn stack(&self) -> MenuStack<'_>;
    /// `IsMenuOpen` (id 82074): the menu is registered and its flag 0x40 is set.
    fn is_open(&self, name: &str) -> bool;
    /// A name `Register` has seen (`MenuTable::Find`, id 82135).
    fn is_registered(&self, name: &str) -> bool;
    /// The live menu registered under `name`: `None` when the name is unknown or the menu was
    /// never shown (or unloaded). The table keeps its own reference; the one the lookup adds is dropped here.
    fn menu(&self, name: &str) -> Option<&IMenu>;
    /// The live HUD, `HUDMenu` behind "HUD Menu".
    fn hud(&self) -> Option<&HUDMenu>;
    /// Registers (or replaces) the creator for `name` (`Register`, id 82086): the engine calls
    /// [`Menu::create`] when the menu is first shown. The table may rehash, so register at load
    /// or from the UI step.
    fn register_menu<M: Menu>(&self, name: &str);
    /// Drops the live menu from the table, keeping its creator (`UnloadMenu`, id 82095).
    fn unload(&self, name: &str);
}

impl MenuManagerExt for EventSourceHolder_MenuOpenClose {
    fn stack(&self) -> MenuStack<'_> {
        let lock = SpinGuard::lock(unsafe { &*sre::g_ui_menu_stack_lock().cast::<sre::BSSpinLock>() });
        let count = u32::from_le_bytes([self.unknown_118[8], self.unknown_118[9], self.unknown_118[10], self.unknown_118[11]]) as usize;
        let menus = if self.menuStack.is_null() || count == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(self.menuStack.cast::<*mut IMenu>(), count) } };
        MenuStack { menus, _lock: lock }
    }

    fn is_open(&self, name: &str) -> bool {
        self.is_menu_open(BSFixedString::new(name).raw())
    }

    fn is_registered(&self, name: &str) -> bool {
        find(self, name).is_some()
    }

    fn menu(&self, name: &str) -> Option<&IMenu> {
        let value = find(self, name)?;
        if value.menu.is_null() {
            return None;
        }
        unsafe { release_menu(value.menu) };
        unsafe { value.menu.as_ref() }
    }

    fn hud(&self) -> Option<&HUDMenu> {
        let menu = self.menu("HUD Menu")?;
        (crate::class_of(menu)?.id == vtables::HUDMenu_vtable::ID).then(|| unsafe { &*core::ptr::from_ref(menu).cast::<HUDMenu>() })
    }

    fn register_menu<M: Menu>(&self, name: &str) {
        let name = c_string(name);
        unsafe { self.register(name.as_ptr(), create_menu::<M> as *const c_void) }
    }

    fn unload(&self, name: &str) {
        self.unload_menu(BSFixedString::new(name).raw());
    }
}

fn find(ui: &EventSourceHolder_MenuOpenClose, name: &str) -> Option<MenuTableValue> {
    let name = BSFixedString::new(name);
    let mut value = MenuTableValue { menu: core::ptr::null_mut(), create: core::ptr::null() };
    let table = (&raw const ui.menuTable).cast_mut().cast();
    let find: FindFn = unsafe { crate::engine_fn(fns::MenuTable__Find) };
    unsafe { find(table, name.raw(), &raw mut value) }.then_some(value)
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
pub struct MenuBase(IMenu);

impl core::ops::Deref for MenuBase {
    type Target = IMenu;

    fn deref(&self) -> &IMenu {
        &self.0
    }
}

impl MenuBase {
    /// Draw and message priority; the stack is kept sorted by it. 3 by default.
    pub fn set_depth(&mut self, depth: u8) {
        self.0.depth = depth;
    }

    pub fn set_flags(&mut self, flags: sre::IMenu_flags) {
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
        let Some(manager) = sre::BSScaleformManager::singleton() else { return false };
        let name = c_string(name);
        let load: LoadMovieFn = unsafe { crate::engine_fn(fns::BSScaleformManager__LoadMovie) };
        unsafe { load(core::ptr::from_ref(manager).cast_mut(), &raw mut self.0, &raw mut self.0.movie, name.as_ptr(), scale_mode as u32, background_alpha) }
    }

    /// `IMenu`'s own advance: sets `CurrentTime` on the movie and advances it. Nothing without a movie.
    pub fn advance_movie(&mut self, interval: f32, time_ms: u32) {
        if !self.0.movie.is_null() {
            let f: MenuAdvance = unsafe { slot(engine_menu_vtable().as_ptr(), vtables::IMenu_vtable::SLOT_AdvanceMovie) };
            unsafe { f(&raw mut self.0, interval, time_ms) }
        }
    }

    /// `IMenu`'s own draw: the movie's `Display`. Nothing without a movie.
    pub fn display_movie(&mut self) {
        if !self.0.movie.is_null() {
            let f: MenuSlot = unsafe { slot(engine_menu_vtable().as_ptr(), vtables::IMenu_vtable::SLOT_PostDisplay) };
            unsafe { f(&raw mut self.0) }
        }
    }
}

/// A menu of this mod's own. Register the type under a name with [`MenuManagerExt::register_menu`];
/// the engine creates it when the name is first shown, advances and draws it in depth order,
/// delivers its messages, and destroys it when it is hidden (unless `KEEP_LOADED` is set).
pub trait Menu: Sized + 'static {
    /// Builds the menu: set the depth, flags and input context on `base`, load a movie if there is one.
    fn create(base: &mut MenuBase) -> Self;

    /// Right after `create`, before the menu is stored in the table.
    fn post_create(&mut self, _base: &mut MenuBase) {}

    /// Every message `ProcessMessages` delivers to this menu: show (1) first, then hide (3), the
    /// per-frame update (0) and events.
    fn process_message(&mut self, _base: &mut MenuBase, _message: &UIMessage) -> MessageResult {
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
    base: IMenu,
    vtable: [*const c_void; 9],
    menu: MaybeUninit<M>,
}

impl<M: Menu> MenuObject<M> {
    unsafe fn parts<'a>(this: *mut IMenu) -> (&'a mut MenuBase, &'a mut M) {
        let object = unsafe { &mut *this.cast::<Self>() };
        (unsafe { &mut *(&raw mut object.base).cast::<MenuBase>() }, unsafe { object.menu.assume_init_mut() })
    }
}

unsafe extern "C" fn create_menu<M: Menu>() -> *mut IMenu {
    let engine = engine_menu_vtable();
    let mut object = Box::new(MenuObject::<M> {
        base: IMenu {
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
            __engine: sre::Engine::new(),
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
unsafe extern "C" fn menu_dtor<M: Menu>(this: *mut IMenu, flags: u32) -> *mut IMenu {
    let object = this.cast::<MenuObject<M>>();
    unsafe { (*object).menu.assume_init_drop() };
    let base: MenuVdtor = unsafe { slot(engine_menu_vtable().as_ptr(), vtables::IMenu_vtable::SLOT_vdtor) };
    unsafe { base(this, flags & !1) };
    if flags & 1 != 0 {
        drop(unsafe { Box::from_raw(object) });
    }
    this
}

unsafe extern "C" fn menu_post_create<M: Menu>(this: *mut IMenu) {
    let (base, menu) = unsafe { MenuObject::<M>::parts(this) };
    menu.post_create(base);
}

unsafe extern "C" fn menu_process_message<M: Menu>(this: *mut IMenu, message: *mut UIMessage) -> u32 {
    let (base, menu) = unsafe { MenuObject::<M>::parts(this) };
    let Some(msg) = (unsafe { message.as_ref() }) else { return 2 };
    match menu.process_message(base, msg) {
        MessageResult::Handled => 0,
        MessageResult::Refused => 1,
        MessageResult::Pass => 2,
        MessageResult::Default => {
            let f: MenuProcessMessage = unsafe { slot(engine_menu_vtable().as_ptr(), vtables::IMenu_vtable::SLOT_ProcessMessage) };
            unsafe { f(this, message) }
        }
    }
}

unsafe extern "C" fn menu_advance<M: Menu>(this: *mut IMenu, interval: f32, time_ms: u32) {
    let (base, menu) = unsafe { MenuObject::<M>::parts(this) };
    menu.advance(base, interval, time_ms);
}

unsafe extern "C" fn menu_display<M: Menu>(this: *mut IMenu) {
    let (base, menu) = unsafe { MenuObject::<M>::parts(this) };
    menu.display(base);
}

unsafe extern "C" fn menu_refresh_platform(this: *mut IMenu) {
    if unsafe { !(*this).movie.is_null() } {
        let f: MenuSlot = unsafe { slot(engine_menu_vtable().as_ptr(), vtables::IMenu_vtable::SLOT_RefreshPlatform) };
        unsafe { f(this) }
    }
}

/// The movie view's own slots: it has no RTTI, so the generated `GFxMovieView` has no vtable methods.
pub trait GFxMovieViewExt {
    /// The view carries the engine's one concrete vtable (`GFxMovieView_vftable`, id 242614).
    fn is_engine_view(&self) -> bool;
    fn visible(&self) -> bool;
    fn set_visible(&self, visible: bool);
    /// A managed string value owned by this movie.
    fn create_string(&self, text: &str) -> Value;
    /// A new plain `ActionScript` object.
    fn create_object(&self) -> Value;
    fn create_array(&self) -> Value;
    /// An `ActionScript` function that calls `function` (a `'static` [`Function`]); set it as a
    /// member of `_root` or another object to let the movie call native code.
    fn create_function<H: FunctionHandler>(&self, function: &'static Function<H>) -> Value;
    /// Reads the variable at `path` (`_root.foo.bar`); `None` when it does not resolve.
    fn variable(&self, path: &str) -> Option<Value>;
    fn set_variable(&self, path: &str, value: &Value) -> bool;
    /// Calls the method at `path` with `args`; `None` when the path does not resolve.
    fn invoke(&self, path: &str, args: &[Value]) -> Option<Value>;
    /// Drops one reference; at zero the view is destroyed.
    ///
    /// # Safety
    /// Only for a reference the caller owns (one `LoadMovie` handed out and the menu no longer holds).
    unsafe fn release(&self);
}

fn movie_slot<F: Copy>(movie: &GFxMovieView, index: usize) -> F {
    unsafe { slot(*core::ptr::from_ref(movie).cast::<*const *const c_void>(), index) }
}

fn movie_ptr(movie: &GFxMovieView) -> *mut GFxMovieView {
    core::ptr::from_ref(movie).cast_mut()
}

impl GFxMovieViewExt for GFxMovieView {
    fn is_engine_view(&self) -> bool {
        let vtable = unsafe { *core::ptr::from_ref(self).cast::<usize>() };
        crate::reloc().address(MOVIE_VIEW_VTABLE).is_ok_and(|v| v == vtable)
    }

    fn visible(&self) -> bool {
        unsafe { movie_slot::<MovieGetVisible>(self, 9)(movie_ptr(self)) }
    }

    fn set_visible(&self, visible: bool) {
        unsafe { movie_slot::<MovieSetVisible>(self, 8)(movie_ptr(self), visible) }
    }

    fn create_string(&self, text: &str) -> Value {
        let text = c_string(text);
        let mut out = Value::undefined();
        unsafe { movie_slot::<MovieCreateString>(self, 11)(movie_ptr(self), out.as_mut_ptr(), text.as_ptr()) };
        out
    }

    fn create_object(&self) -> Value {
        let mut out = Value::undefined();
        unsafe { movie_slot::<MovieCreateObject>(self, 13)(movie_ptr(self), out.as_mut_ptr(), core::ptr::null(), core::ptr::null(), 0) };
        out
    }

    fn create_array(&self) -> Value {
        let mut out = Value::undefined();
        unsafe { movie_slot::<MovieCreateArray>(self, 14)(movie_ptr(self), out.as_mut_ptr()) };
        out
    }

    fn create_function<H: FunctionHandler>(&self, function: &'static Function<H>) -> Value {
        let mut out = Value::undefined();
        unsafe { movie_slot::<MovieCreateFunction>(self, 15)(movie_ptr(self), out.as_mut_ptr(), function.as_ptr(), core::ptr::null_mut()) };
        out
    }

    fn variable(&self, path: &str) -> Option<Value> {
        let path = c_string(path);
        let mut out = Value::undefined();
        unsafe { movie_slot::<MovieGetVariable>(self, 17)(movie_ptr(self), out.as_mut_ptr(), path.as_ptr()) }.then_some(out)
    }

    fn set_variable(&self, path: &str, value: &Value) -> bool {
        let path = c_string(path);
        unsafe { movie_slot::<MovieSetVariable>(self, 16)(movie_ptr(self), path.as_ptr(), value.as_ptr(), 0) }
    }

    fn invoke(&self, path: &str, args: &[Value]) -> Option<Value> {
        let path = c_string(path);
        let mut out = Value::undefined();
        let count = u32::try_from(args.len()).expect("argument count");
        unsafe { movie_slot::<MovieInvoke>(self, 23)(movie_ptr(self), path.as_ptr(), out.as_mut_ptr(), args.as_ptr().cast(), count) }.then_some(out)
    }

    unsafe fn release(&self) {
        unsafe { movie_slot::<MovieRelease>(self, 69)(movie_ptr(self)) }
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
pub struct Value(sre::GFxValue);

impl Value {
    const fn unmanaged(type_: u32, data: u64) -> Self {
        Self(sre::GFxValue { objectInterface: core::ptr::null_mut(), type_, unknown_C: [0; 4], data, __engine: sre::Engine::new() })
    }

    #[must_use]
    pub const fn undefined() -> Self {
        Self::unmanaged(0, 0)
    }

    #[must_use]
    pub const fn null() -> Self {
        Self::unmanaged(1, 0)
    }

    #[must_use]
    pub const fn boolean(value: bool) -> Self {
        Self::unmanaged(2, value as u64)
    }

    #[must_use]
    pub fn number(value: f64) -> Self {
        Self::unmanaged(3, value.to_bits())
    }

    /// An unmanaged string value over `text`, which must outlive every use of the value.
    #[must_use]
    pub fn string(text: &'static CStr) -> Self {
        Self::unmanaged(4, text.as_ptr() as u64)
    }

    /// A value an engine object holds (the HUD's `root`), viewed without taking it.
    #[must_use]
    pub fn view(raw: &sre::GFxValue) -> &Self {
        unsafe { &*core::ptr::from_ref(raw).cast::<Self>() }
    }

    #[must_use]
    pub fn raw(&self) -> &sre::GFxValue {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const sre::GFxValue {
        &raw const self.0
    }

    fn as_mut_ptr(&mut self) -> *mut sre::GFxValue {
        &raw mut self.0
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
        let get: ObjectGetMemberFn = unsafe { crate::engine_fn(fns::GFxValue__ObjectInterface__GetMember) };
        self.object_call(|iface, data, display| unsafe { get(iface, data, name.as_ptr(), out.as_mut_ptr(), display) })?.then_some(out)
    }

    #[must_use]
    pub fn set_member(&self, name: &str, value: &Value) -> bool {
        let name = c_string(name);
        let set: ObjectSetMemberFn = unsafe { crate::engine_fn(fns::GFxValue__ObjectInterface__SetMember) };
        self.object_call(|iface, data, display| unsafe { set(iface, data, name.as_ptr(), value.as_ptr(), display) }) == Some(true)
    }

    #[must_use]
    pub fn has_member(&self, name: &str) -> bool {
        let name = c_string(name);
        let has: ObjectHasMemberFn = unsafe { crate::engine_fn(fns::GFxValue__ObjectInterface__HasMember) };
        self.object_call(|iface, data, display| unsafe { has(iface, data, name.as_ptr(), display) }) == Some(true)
    }

    /// Calls the method `name` of this object (`GFxValue::ObjectInterface::Invoke`, id 82256).
    #[must_use]
    pub fn invoke(&self, name: &str, args: &[Value]) -> Option<Value> {
        let name = c_string(name);
        let count = u32::try_from(args.len()).expect("argument count");
        let mut out = Value::undefined();
        let invoke: ObjectInvokeFn = unsafe { crate::engine_fn(fns::GFxValue__ObjectInterface__Invoke) };
        self.object_call(|iface, data, display| unsafe { invoke(iface, data, out.as_mut_ptr(), name.as_ptr(), args.as_ptr().cast(), count, display) })?.then_some(out)
    }
}

impl Drop for Value {
    fn drop(&mut self) {
        if self.is_managed() {
            let release: fns::GFxValue__ReleaseManaged_fn = unsafe { crate::engine_fn(fns::GFxValue__ReleaseManaged) };
            unsafe { release(&raw mut self.0) };
        }
    }
}

/// `GFxValue::Assign` (id 33582): add-ref and release as needed.
unsafe fn assign(target: *mut sre::GFxValue, value: &sre::GFxValue) {
    let f: fns::GFxValue__Assign_fn = unsafe { crate::engine_fn(fns::GFxValue__Assign) };
    unsafe { f(target, value) };
}

impl Clone for Value {
    fn clone(&self) -> Self {
        let mut out = Self::undefined();
        unsafe { assign(&raw mut out.0, &self.0) };
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
pub struct Params(sre::GFxFunctionHandler__Params);

impl Params {
    #[must_use]
    pub fn raw(&self) -> &sre::GFxFunctionHandler__Params {
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
    pub fn movie(&self) -> Option<&GFxMovieView> {
        unsafe { self.0.movie.as_ref() }
    }

    pub fn set_return(&mut self, value: &Value) {
        if !self.0.retVal.is_null() {
            unsafe { assign(self.0.retVal, &value.0) };
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
    call: unsafe extern "C" fn(this: *mut Function<H>, params: *mut sre::GFxFunctionHandler__Params),
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

    fn as_ptr(&'static self) -> *mut sre::GFxFunctionHandler {
        core::ptr::from_ref(self).cast_mut().cast()
    }

    unsafe extern "C" fn destroy(this: *mut Self, _flags: u32) -> *mut Self {
        this
    }

    unsafe extern "C" fn call(this: *mut Self, params: *mut sre::GFxFunctionHandler__Params) {
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
