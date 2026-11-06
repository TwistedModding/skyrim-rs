// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The compat suite's plain SKSE plugin a (`legacy_patch::load_a`).

skse::plugin! {
    name: "skyrim-rs-legacy-patch-a",
    author: "TwistedModding",
    version: 1,
    load: legacy_patch::load_a,
}
