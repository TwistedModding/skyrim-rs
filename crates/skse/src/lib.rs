//! SKSE plugin ABI: version data, entry points and the loader interfaces.

use core::ffi::{c_char, c_void};

pub mod log;
pub mod runtime;

pub type PluginHandle = u32;
pub const INVALID_HANDLE: PluginHandle = u32::MAX;

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

#[repr(C)]
pub struct Interface {
    pub skse_version: u32,
    pub runtime_version: u32,
    pub editor_version: u32,
    pub is_editor: u32,
    pub query_interface: unsafe extern "C" fn(id: u32) -> *mut c_void,
    pub get_plugin_handle: unsafe extern "C" fn() -> PluginHandle,
    pub get_release_index: unsafe extern "C" fn() -> u32,
    pub get_plugin_info: unsafe extern "C" fn(name: *const c_char) -> *const PluginInfo,
}

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

#[repr(C)]
pub struct Message {
    pub sender: *const c_char,
    pub kind: u32,
    pub data_len: u32,
    pub data: *mut c_void,
}

pub type MessageCallback = unsafe extern "C" fn(msg: *mut Message);

#[repr(C)]
pub struct MessagingInterface {
    pub interface_version: u32,
    pub register_listener:
        unsafe extern "C" fn(listener: PluginHandle, sender: *const c_char, handler: MessageCallback) -> bool,
    pub dispatch: unsafe extern "C" fn(
        sender: PluginHandle,
        kind: u32,
        data: *mut c_void,
        data_len: u32,
        receiver: *const c_char,
    ) -> bool,
    pub get_event_dispatcher: unsafe extern "C" fn(dispatcher: u32) -> *mut c_void,
}

impl MessagingInterface {
    pub const VERSION: u32 = 2;
    /// Message kinds as the loader logs them (`dispatch message (N) to plugin listeners`): 0, 1, 6
    /// and 8 arrive at startup in that order, 2 before and 3 after a save load, 4 at a save
    /// (all seen in the in-game runs); 5 and 7 are retained from the first build and unverified.
    pub const POST_LOAD: u32 = 0;
    pub const POST_POST_LOAD: u32 = 1;
    pub const PRE_LOAD_GAME: u32 = 2;
    pub const POST_LOAD_GAME: u32 = 3;
    pub const SAVE_GAME: u32 = 4;
    pub const DELETE_GAME: u32 = 5;
    pub const INPUT_LOADED: u32 = 6;
    pub const NEW_GAME: u32 = 7;
    pub const DATA_LOADED: u32 = 8;

    #[must_use]
    pub fn listen_skse(&self, handle: PluginHandle, handler: MessageCallback) -> bool {
        unsafe { (self.register_listener)(handle, c"SKSE".as_ptr(), handler) }
    }
}

pub type SerializationCallback = unsafe extern "C" fn(intfc: *const SerializationInterface);
pub type FormDeleteCallback = unsafe extern "C" fn(handle: u64);

#[repr(C)]
pub struct SerializationInterface {
    pub version: u32,
    pub set_unique_id: unsafe extern "C" fn(plugin: PluginHandle, uid: u32),
    pub set_revert_callback: unsafe extern "C" fn(plugin: PluginHandle, callback: SerializationCallback),
    pub set_save_callback: unsafe extern "C" fn(plugin: PluginHandle, callback: SerializationCallback),
    pub set_load_callback: unsafe extern "C" fn(plugin: PluginHandle, callback: SerializationCallback),
    pub set_form_delete_callback: unsafe extern "C" fn(plugin: PluginHandle, callback: FormDeleteCallback),
    pub write_record: unsafe extern "C" fn(kind: u32, version: u32, buf: *const c_void, len: u32) -> bool,
    pub open_record: unsafe extern "C" fn(kind: u32, version: u32) -> bool,
    pub write_record_data: unsafe extern "C" fn(buf: *const c_void, len: u32) -> bool,
    pub get_next_record_info: unsafe extern "C" fn(kind: *mut u32, version: *mut u32, len: *mut u32) -> bool,
    pub read_record_data: unsafe extern "C" fn(buf: *mut c_void, len: u32) -> u32,
    pub resolve_handle: unsafe extern "C" fn(handle: u64, out: *mut u64) -> bool,
    pub resolve_form_id: unsafe extern "C" fn(form_id: u32, out: *mut u32) -> bool,
}

impl SerializationInterface {
    pub const VERSION: u32 = 4;
}

/// Receives the game's `BSScript::Internal::VirtualMachine` (`skyrim::papyrus::VirtualMachine::from_ptr`
/// checks and types it); the loader calls it on the game's main thread while the engine registers
/// its own natives, before `DataLoaded`.
pub type RegisterFunctions = unsafe extern "C" fn(registry: *mut c_void) -> bool;

#[repr(C)]
pub struct PapyrusInterface {
    pub interface_version: u32,
    pub register: unsafe extern "C" fn(callback: RegisterFunctions) -> bool,
}

impl PapyrusInterface {
    pub const VERSION: u32 = 1;

    /// Queues `callback` for the registration pass; `false` when the loader refused it.
    #[must_use]
    pub fn register(&self, callback: RegisterFunctions) -> bool {
        unsafe { (self.register)(callback) }
    }
}

/// The two flag words of the version data, named by what SKSE 2.3.1 does with each bit.
/// `independence` is the word at +0x304,
/// `independence_ex` the word at +0x308.
pub mod independence {
    /// +0x304 bit 0: set by most plugins that resolve addresses through the Address Library; with
    /// it set the loader does not require [`EX_STRUCTS_1_6_629`] on a runtime the plugin does not list.
    pub const ADDRESS_LIBRARY: u32 = 1 << 0;
    /// +0x304 bit 1: skips the link-timestamp gate ("must be recompiled for new address library").
    pub const SIGNATURES: u32 = 1 << 1;
    /// +0x308 bit 0: the plugin reads Address Library ids; the runtime's file must exist
    /// ("address library needs to be updated" otherwise) and the build must postdate the id change
    /// unless [`SIGNATURES`] is set.
    pub const EX_ADDRESS_LIBRARY_IDS: u32 = 1 << 0;
    /// +0x308 bit 1: version independent without Address Library ids.
    pub const EX_NO_ADDRESS_LIBRARY: u32 = 1 << 1;
    /// +0x308 bit 2: built for the structure layouts of 1.6.629 and later ("only compatible with
    /// versions earlier than 1.6.629" without it or [`ADDRESS_LIBRARY`] on an unlisted runtime).
    pub const EX_STRUCTS_1_6_629: u32 = 1 << 2;
}

/// The data behind the exported `SKSEPlugin_Version` symbol, 0x350 bytes: the layout every field
/// of which was read from at least two real plugin DLLs.
#[repr(C)]
pub struct PluginVersionData {
    pub data_version: u32,
    pub plugin_version: u32,
    pub name: [u8; 256],
    pub author: [u8; 256],
    pub support_email: [u8; 252],
    /// +0x304, the `independence` bits.
    pub independence: u32,
    /// +0x308, the `independence_ex` bits.
    pub independence_ex: u32,
    /// Packed runtime versions the plugin was built for, zero-terminated.
    pub compatible_versions: [u32; 16],
    /// Packed minimum SKSE version ("requires newer script extender" below it), 0 for none.
    pub minimum_skse: u32,
}

const _: () = assert!(core::mem::offset_of!(PluginVersionData, independence) == 0x304);
const _: () = assert!(core::mem::offset_of!(PluginVersionData, independence_ex) == 0x308);
const _: () = assert!(core::mem::offset_of!(PluginVersionData, compatible_versions) == 0x30C);
const _: () = assert!(core::mem::offset_of!(PluginVersionData, minimum_skse) == 0x34C);

const _: () = assert!(size_of::<PluginVersionData>() == 848);

impl PluginVersionData {
    pub const VERSION: u32 = 1;

    #[must_use]
    pub const fn new(name: &str, author: &str, plugin_version: u32, independence: u32, independence_ex: u32) -> Self {
        Self {
            data_version: Self::VERSION,
            plugin_version,
            name: fill(name),
            author: fill(author),
            support_email: [0; 252],
            independence,
            independence_ex,
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

/// Declares a plugin: exports `SKSEPlugin_Version`, which SKSE 2.3.1 reads before any code runs,
/// and `SKSEPlugin_Query` / `SKSEPlugin_Load`, which SKSE 2.0.19 calls in that order (both seen in
/// the loader logs of the two installs).
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
/// The defaults (`independence` = `SIGNATURES`, `independence_ex` = `EX_ADDRESS_LIBRARY_IDS |
/// EX_STRUCTS_1_6_629`) are what a mod on `layout` needs on every runtime SKSE 2.3.1 accepts:
/// the Address Library file must exist, the 1.6.629 structure check passes, and the link-timestamp
/// gate is skipped. Pass `independence:` and `independence_ex:` to override both.
#[macro_export]
macro_rules! plugin {
    (
        name: $name:literal,
        author: $author:literal,
        version: $version:expr,
        load: $load:path
        $(, independence: $ind:expr, independence_ex: $ex:expr)?
        $(,)?
    ) => {
        #[unsafe(no_mangle)]
        #[allow(non_upper_case_globals)]
        pub static SKSEPlugin_Version: $crate::PluginVersionData = $crate::PluginVersionData::new(
            $name,
            $author,
            $version,
            $crate::plugin!(@ind $($ind)?),
            $crate::plugin!(@ex $($ex)?),
        );

        #[unsafe(no_mangle)]
        #[allow(non_snake_case)]
        pub extern "C" fn SKSEPlugin_Query(skse: &$crate::Interface, info: &mut $crate::PluginInfo) -> bool {
            info.info_version = $crate::PluginInfo::VERSION;
            info.name = concat!($name, "\0").as_ptr().cast();
            info.version = $version;
            skse.is_editor == 0
        }

        #[unsafe(no_mangle)]
        #[allow(non_snake_case)]
        pub extern "C" fn SKSEPlugin_Load(skse: &$crate::Interface) -> bool {
            $load(skse)
        }
    };
    (@ind) => {
        $crate::independence::SIGNATURES
    };
    (@ind $e:expr) => {
        $e
    };
    (@ex) => {
        $crate::independence::EX_ADDRESS_LIBRARY_IDS | $crate::independence::EX_STRUCTS_1_6_629
    };
    (@ex $e:expr) => {
        $e
    };
}
