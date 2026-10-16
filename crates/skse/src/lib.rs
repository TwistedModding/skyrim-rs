//! SKSE plugin ABI: version data, entry points and the loader interfaces.

use core::ffi::{c_char, c_void};

pub mod log;
pub mod runtime;

pub type PluginHandle = u32;
pub const INVALID_HANDLE: PluginHandle = u32::MAX;

/// A plugin's name and version as the loader keeps them (0x18 bytes). 2.0.19 has the plugin fill it
/// in `SKSEPlugin_Query` and reads only the name and version; 2.3.1 fills it from the version data
/// after every plugin's `SKSEPlugin_Load` has run.
#[repr(C)]
pub struct PluginInfo {
    pub info_version: u32,
    pub name: *const c_char,
    pub version: u32,
}

impl PluginInfo {
    pub const VERSION: u32 = 1;
}

#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum InterfaceId {
    Scaleform = 1,
    Papyrus,
    Serialization,
    Task,
    Messaging,
    Object,
    Trampoline,
}

/// The struct handed to `SKSEPlugin_Query`, `SKSEPlugin_Preload` and `SKSEPlugin_Load` (0x30 bytes):
/// one constant object in the loader's read-only data, the same for every plugin and call.
#[repr(C)]
pub struct Interface {
    /// Packed loader version (0x02030010 for 2.3.1).
    pub skse_version: u32,
    /// Packed runtime version (0x01070680 for 1.7.104).
    pub runtime_version: u32,
    /// Always 0; the loader never reads them.
    pub reserved: [u32; 2],
    /// The table for ids 1 to 7, null (and a log line) for any other id.
    pub query_interface: unsafe extern "C" fn(id: u32) -> *mut c_void,
    /// The calling plugin's handle. Only valid inside `Query`, `Preload` and `Load`: outside them
    /// the loader logs the misuse and crashes on purpose.
    pub get_plugin_handle: unsafe extern "C" fn() -> PluginHandle,
    /// The loader's release index (75 for 2.3.1).
    pub get_release_index: unsafe extern "C" fn() -> u32,
    /// A loaded plugin's info by name, case-insensitive; null for an unknown name and, on 2.3.1,
    /// for every name until all `Load` calls have returned.
    pub get_plugin_info: unsafe extern "C" fn(name: *const c_char) -> *const PluginInfo,
}

const _: () = assert!(size_of::<Interface>() == 0x30);
const _: () = assert!(core::mem::offset_of!(Interface, query_interface) == 0x10);
const _: () = assert!(core::mem::offset_of!(Interface, get_plugin_info) == 0x28);

impl Interface {
    /// # Safety
    /// `T` must be the interface struct SKSE hands out for `id`.
    #[must_use]
    pub unsafe fn query<T>(&self, id: InterfaceId) -> Option<&'static T> {
        unsafe { (self.query_interface)(id as u32).cast::<T>().as_ref() }
    }

    #[must_use]
    pub fn messaging(&self) -> Option<&'static MessagingInterface> {
        unsafe { self.query(InterfaceId::Messaging) }
    }

    #[must_use]
    pub fn serialization(&self) -> Option<&'static SerializationInterface> {
        unsafe { self.query(InterfaceId::Serialization) }
    }

    #[must_use]
    pub fn papyrus(&self) -> Option<&'static PapyrusInterface> {
        unsafe { self.query(InterfaceId::Papyrus) }
    }

    #[must_use]
    pub fn plugin_handle(&self) -> PluginHandle {
        unsafe { (self.get_plugin_handle)() }
    }
}

/// A message as a listener receives it (0x18 bytes).
#[repr(C)]
pub struct Message {
    pub sender: *const c_char,
    pub kind: u32,
    pub data_len: u32,
    pub data: *mut c_void,
}

pub type MessageCallback = unsafe extern "C" fn(msg: *mut Message);

/// Interface id 5. Every table starts with a qword whose low dword is the table version.
#[repr(C)]
pub struct MessagingInterface {
    pub interface_version: u32,
    /// `sender` matches case-insensitively, "SKSE" being the loader (handle 0); null listens to
    /// every plugin except the loader and the caller.
    pub register_listener:
        unsafe extern "C" fn(listener: PluginHandle, sender: *const c_char, handler: MessageCallback) -> bool,
    pub dispatch: unsafe extern "C" fn(
        sender: PluginHandle,
        kind: u32,
        data: *mut c_void,
        data_len: u32,
        receiver: *const c_char,
    ) -> bool,
    /// Ids 0 to 4 return event sources the loader owns; any other id logs and returns null.
    pub get_event_dispatcher: unsafe extern "C" fn(dispatcher: u32) -> *mut c_void,
}

impl MessagingInterface {
    pub const VERSION: u32 = 2;
    /// Kinds the loader sends as sender "SKSE"; at startup 0, 1, 6 and 8 arrive in that order.
    /// After every plugin's `Load`, before any game hook; no data.
    pub const PLUGINS_LOADED: u32 = 0;
    /// Right after [`Self::PLUGINS_LOADED`]; no data.
    pub const PLUGINS_LOADED_2: u32 = 1;
    /// Before the game loads a save; the save name, `data_len` its length.
    pub const PRE_LOAD_GAME: u32 = 2;
    /// After the load returns, the co-save already read; the result as a bool in the `data` field
    /// itself, `data_len` 1.
    pub const POST_LOAD_GAME: u32 = 3;
    /// Before the game saves, the co-save written after; the save name.
    pub const SAVE_GAME: u32 = 4;
    /// Before a save is deleted; the save name (static only, never observed).
    pub const DELETE_GAME: u32 = 5;
    /// After the player controls are constructed; no data.
    pub const CONTROLS_READY: u32 = 6;
    /// A new game started from the main menu; an 8-byte value in the `data` field (static only).
    pub const NEW_GAME: u32 = 7;
    /// After a step of the game's init thread, before the main menu, with the data files loaded
    /// (form lookups succeed); no data.
    pub const DATA_LOADED: u32 = 8;

    #[must_use]
    pub fn listen_skse(&self, handle: PluginHandle, handler: MessageCallback) -> bool {
        unsafe { (self.register_listener)(handle, c"SKSE".as_ptr(), handler) }
    }
}

pub type SerializationCallback = unsafe extern "C" fn(intfc: *const SerializationInterface);
pub type HandleDeleteCallback = unsafe extern "C" fn(vm_handle: u64);

/// Interface id 3, the co-save (`<save>.skse`, written after [`MessagingInterface::SAVE_GAME`] and
/// read before [`MessagingInterface::POST_LOAD_GAME`]).
#[repr(C)]
pub struct SerializationInterface {
    pub version: u32,
    pub set_uid: unsafe extern "C" fn(plugin: PluginHandle, uid: u32),
    pub set_revert_callback: unsafe extern "C" fn(plugin: PluginHandle, callback: SerializationCallback),
    pub set_save_callback: unsafe extern "C" fn(plugin: PluginHandle, callback: SerializationCallback),
    pub set_load_callback: unsafe extern "C" fn(plugin: PluginHandle, callback: SerializationCallback),
    /// Called from the loader's hook on the game's script handle release.
    pub set_handle_delete_callback: unsafe extern "C" fn(plugin: PluginHandle, callback: HandleDeleteCallback),
    pub write_record: unsafe extern "C" fn(kind: u32, version: u32, buf: *const c_void, len: u32) -> bool,
    pub open_record: unsafe extern "C" fn(kind: u32, version: u32) -> bool,
    pub write_record_data: unsafe extern "C" fn(buf: *const c_void, len: u32) -> bool,
    pub read_record_header: unsafe extern "C" fn(kind: *mut u32, version: *mut u32, len: *mut u32) -> bool,
    /// Clamped to the current record.
    pub read_record_data: unsafe extern "C" fn(buf: *mut c_void, len: u32) -> u32,
    pub remap_handle: unsafe extern "C" fn(old: u64, new: *mut u64) -> bool,
    /// Maps a saved form id's load-order byte (or 0xFE light prefix and index) to the current one;
    /// 0xFF ids are unchanged.
    pub remap_form_id: unsafe extern "C" fn(old: u32, new: *mut u32) -> bool,
}

impl SerializationInterface {
    pub const VERSION: u32 = 4;
}

/// Receives the game's `BSScript::Internal::VirtualMachine` (`skyrim::papyrus::VirtualMachine::from_ptr`
/// checks and types it); the loader calls it from its hook in the game's script VM constructor,
/// after the engine's and the loader's own natives, before `DataLoaded`.
pub type RegisterFunctions = unsafe extern "C" fn(registry: *mut c_void);

#[repr(C)]
pub struct PapyrusInterface {
    pub interface_version: u32,
    pub register: unsafe extern "C" fn(callback: RegisterFunctions) -> bool,
}

impl PapyrusInterface {
    pub const VERSION: u32 = 1;

    /// Queues `callback` for the registration pass (the loader always answers `true`).
    #[must_use]
    pub fn register(&self, callback: RegisterFunctions) -> bool {
        unsafe { (self.register)(callback) }
    }
}

/// Interface id 4. A task is an object whose vtable's slot 0 runs it and slot 1 disposes of it.
#[repr(C)]
pub struct TaskInterface {
    pub interface_version: u32,
    /// Queues a task for the main loop, run after the game drains its own task queue.
    pub add_task: unsafe extern "C" fn(task: *mut c_void),
    /// Queues a task for the UI message processing; dropped without dispose while the game's UI
    /// message queue does not exist.
    pub add_ui_task: unsafe extern "C" fn(task: *mut c_void),
}

impl TaskInterface {
    pub const VERSION: u32 = 2;
}

/// Interface id 7: memory within rel32 reach of the game for a plugin's branches and code.
#[repr(C)]
pub struct TrampolineInterface {
    pub interface_version: u32,
    pub alloc_branch: unsafe extern "C" fn(handle: PluginHandle, size: u64) -> *mut c_void,
    pub alloc_local: unsafe extern "C" fn(handle: PluginHandle, size: u64) -> *mut c_void,
}

impl TrampolineInterface {
    pub const VERSION: u32 = 1;
}

/// The two bit words of the version data as SKSE 2.3.1 reads them: the flags at +0x304 and the
/// "version independence method" at +0x308 (the loader's own words). A method bit above 2 is
/// "disabled, unsupported version independence method".
pub mod compat {
    /// +0x304 bit 0: passes the 1.6.629 test, as [`METHOD_STRUCTS_1_6_629`] does.
    pub const FLAG_STRUCTS_1_6_629: u32 = 1 << 0;
    /// +0x304 bit 1: skips the link-timestamp gate ("must be recompiled for new address library").
    pub const FLAG_NO_TIMESTAMP_GATE: u32 = 1 << 1;
    /// +0x308 bit 0: the plugin reads Address Library ids. The runtime's `versionlib` file must
    /// open ("address library needs to be updated" otherwise), then the timestamp gate applies
    /// unless [`FLAG_NO_TIMESTAMP_GATE`] is set or the runtime is in the compatible list.
    pub const METHOD_ADDRESS_LIBRARY: u32 = 1 << 0;
    /// +0x308 bit 1: version independent with no file check.
    pub const METHOD_NO_FILE_CHECK: u32 = 1 << 1;
    /// +0x308 bit 2: built for the structure layouts of 1.6.629 and later. A plugin with method
    /// bit 0 or 1 needs it or [`FLAG_STRUCTS_1_6_629`] ("only compatible with versions earlier
    /// than 1.6.629"), whatever its compatible list says.
    pub const METHOD_STRUCTS_1_6_629: u32 = 1 << 2;
}

/// The data behind the exported `SKSEPlugin_Version` symbol, 0x350 bytes, which SKSE 2.3.1 copies
/// before any plugin code runs (2.0.19 never reads it).
#[repr(C)]
pub struct PluginVersionData {
    /// Only zero ("disabled, bad version data") or non-zero is tested.
    pub data_version: u32,
    pub plugin_version: u32,
    /// Empty is "disabled, no name specified".
    pub name: [u8; 256],
    /// Where real plugins put the author; the loader never reads it.
    pub author: [u8; 256],
    /// Where real plugins put a contact address; the loader never reads it.
    pub support_email: [u8; 252],
    /// +0x304, [`compat`]`::FLAG_*`.
    pub flags: u32,
    /// +0x308, [`compat`]`::METHOD_*`.
    pub method: u32,
    /// Packed runtime versions the plugin was built for, zero-terminated; read only when the
    /// method has neither bit 0 nor bit 1, or to excuse a failed timestamp gate.
    pub compatible_versions: [u32; 16],
    /// Packed minimum SKSE version ("requires newer script extender" below it), 0 for none.
    pub minimum_skse: u32,
}

const _: () = assert!(core::mem::offset_of!(PluginVersionData, flags) == 0x304);
const _: () = assert!(core::mem::offset_of!(PluginVersionData, method) == 0x308);
const _: () = assert!(core::mem::offset_of!(PluginVersionData, compatible_versions) == 0x30C);
const _: () = assert!(core::mem::offset_of!(PluginVersionData, minimum_skse) == 0x34C);

const _: () = assert!(size_of::<PluginVersionData>() == 848);

impl PluginVersionData {
    pub const VERSION: u32 = 1;

    #[must_use]
    pub const fn new(name: &str, author: &str, plugin_version: u32, flags: u32, method: u32) -> Self {
        Self {
            data_version: Self::VERSION,
            plugin_version,
            name: fill(name),
            author: fill(author),
            support_email: [0; 252],
            flags,
            method,
            compatible_versions: [0; 16],
            minimum_skse: 0,
        }
    }
}

const fn fill<const N: usize>(s: &str) -> [u8; N] {
    let bytes = s.as_bytes();
    assert!(bytes.len() < N, "string does not fit the fixed field");
    let mut out = [0; N];
    let mut i = 0;
    while i < bytes.len() {
        out[i] = bytes[i];
        i += 1;
    }
    out
}

/// Declares a plugin: exports `SKSEPlugin_Version`, which SKSE 2.3.1 reads before any code runs
/// and then calls `SKSEPlugin_Load` on, and `SKSEPlugin_Query`, which SKSE 2.0.19 calls before
/// `SKSEPlugin_Load`. 2.3.1 also calls an optional `SKSEPlugin_Preload` before any `Load`; the
/// macro does not export one.
///
/// ```ignore
/// skse::plugin! {
///     name: "my-mod",
///     author: "me",
///     version: 1,
///     load: load,
/// }
/// fn load(skse: &skse::Interface) -> bool { true }
/// ```
///
/// The defaults (`flags` = `FLAG_NO_TIMESTAMP_GATE`, `method` = `METHOD_ADDRESS_LIBRARY |
/// METHOD_STRUCTS_1_6_629`) are what a mod on `layout` needs on every runtime SKSE 2.3.1 accepts:
/// the Address Library file must exist, the 1.6.629 test passes, and the link-timestamp gate is
/// skipped. Pass `flags:` and `method:` to override both.
#[macro_export]
macro_rules! plugin {
    (
        name: $name:literal,
        author: $author:literal,
        version: $version:expr,
        load: $load:path
        $(, flags: $flags:expr, method: $method:expr)?
        $(,)?
    ) => {
        #[unsafe(no_mangle)]
        #[allow(non_upper_case_globals)]
        pub static SKSEPlugin_Version: $crate::PluginVersionData = $crate::PluginVersionData::new(
            $name,
            $author,
            $version,
            $crate::plugin!(@flags $($flags)?),
            $crate::plugin!(@method $($method)?),
        );

        #[unsafe(no_mangle)]
        #[allow(non_snake_case)]
        pub extern "C" fn SKSEPlugin_Query(_: &$crate::Interface, info: &mut $crate::PluginInfo) -> bool {
            info.info_version = $crate::PluginInfo::VERSION;
            info.name = concat!($name, "\0").as_ptr().cast();
            info.version = $version;
            true
        }

        #[unsafe(no_mangle)]
        #[allow(non_snake_case)]
        pub extern "C" fn SKSEPlugin_Load(skse: &$crate::Interface) -> bool {
            $load(skse)
        }
    };
    (@flags) => {
        $crate::compat::FLAG_NO_TIMESTAMP_GATE
    };
    (@flags $e:expr) => {
        $e
    };
    (@method) => {
        $crate::compat::METHOD_ADDRESS_LIBRARY | $crate::compat::METHOD_STRUCTS_1_6_629
    };
    (@method $e:expr) => {
        $e
    };
}
