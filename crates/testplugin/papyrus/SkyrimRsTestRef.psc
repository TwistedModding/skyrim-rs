Scriptname SkyrimRsTestRef extends ObjectReference Hidden
; Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
; This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
; distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
; This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
{A method native and an event; the test plugin binds an object of this script to the player, so DevBench and a dispatch can call it and SendEvent reaches it.}

int Function SelfFormId() native

Event OnSkyrimRsEvent(int count, string text)
    SkyrimRsTest.RecordEvent(count, text)
EndEvent
