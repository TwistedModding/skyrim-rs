//! Framework suite plugin: one half of a load order cycle with `order-cycle-b`, which the host
//! refuses. Its `load` would append its name to `skyrim-rs-order.log`.

use std::io::Write as _;

framework::plugin! {
    name: "skyrim-rs-order-cycle-a",
    load: load,
    load_before: ["skyrim-rs-order-cycle-b"],
}

fn load(host: &'static framework::Host) -> bool {
    skse::log::append(host.skse().runtime_version, "skyrim-rs-order").and_then(|mut f| writeln!(f, "skyrim-rs-order-cycle-a")).is_ok()
}
