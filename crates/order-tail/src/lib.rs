//! Framework suite plugin: loads after every other plugin although its priority is the highest.
//! Appends its name to `skyrim-rs-order.log` when its `load` runs.

use std::io::Write as _;

framework::plugin! {
    name: "skyrim-rs-order-tail",
    load: load,
    load_after: ["*"],
    priority: 9,
}

fn load(host: &'static framework::Host) -> bool {
    skse::log::append(host.skse().runtime_version, "skyrim-rs-order").and_then(|mut f| writeln!(f, "skyrim-rs-order-tail")).is_ok()
}
