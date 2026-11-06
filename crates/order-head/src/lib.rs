// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Framework suite plugin: its priority puts it before the suite's plugin, whose file name sorts
//! first, and it asks for a plugin that does not exist. Appends its name to `skyrim-rs-order.log`
//! when its `load` runs.

use std::io::Write as _;

framework::plugin! {
    name: "skyrim-rs-order-head",
    load: load,
    load_after: ["skyrim-rs-nowhere"],
    priority: 1,
}

fn load(host: &'static framework::Host) -> bool {
    skse::log::append(host.skse().runtime_version, "skyrim-rs-order").and_then(|mut f| writeln!(f, "skyrim-rs-order-head")).is_ok()
}
