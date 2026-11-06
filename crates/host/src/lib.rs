// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! `skyrim_runtime_extender.dll`, the framework's host: an SKSE plugin that installs the overlay and loads the
//! framework plugins in `Data\SKSE\Plugins\skyrim-runtime-extender\` (see the `framework` crate).

skse::plugin! {
    name: "skyrim-runtime-extender",
    author: "TwistedModding",
    version: 1,
    load: framework::host::load,
}
