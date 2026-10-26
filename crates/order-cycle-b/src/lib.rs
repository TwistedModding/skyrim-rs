//! Framework suite plugin: the other half of the load order cycle with `order-cycle-a`.

use std::io::Write as _;

framework::plugin! {
    name: "skyrim-rs-order-cycle-b",
    load: load,
    load_before: ["skyrim-rs-order-cycle-a"],
}

fn load(host: &'static framework::Host) -> bool {
    skse::log::append(host.skse().runtime_version, "skyrim-rs-order").and_then(|mut f| writeln!(f, "skyrim-rs-order-cycle-b")).is_ok()
}
