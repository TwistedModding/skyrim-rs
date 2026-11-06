Scriptname SkyrimRsCrashTest
; Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
; This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
; distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
; This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.

; The crash suite's script side: Crash is the test plugin's native that faults on purpose;
; CrashFromScript calls it from script code so the crash log shows a script frame under the native.
Function Crash() global native

Function CrashFromScript() global
    Crash()
EndFunction
