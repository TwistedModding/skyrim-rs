// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
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
