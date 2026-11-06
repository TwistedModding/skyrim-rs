Scriptname SkyrimRsTest Hidden
; Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
; This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
; distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
; This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
{Declarations of the natives crates/testplugin registers through skyrim::papyrus; compiled by scripts/compile-papyrus.ps1.}

int Function Ping(int x) native global
int Function FormId(Form akForm) native global
float Function PlayerHealth(Actor akActor) native global
string Function Echo(string s) native global
float Function Scale(float x, bool double) native global
Form Function Player() native global
Function Tick() native global
string Function Describe(ObjectReference akRef, int count, string suffix, bool flag) native global
float[] Function Halves(int[] values) native global
Form[] Function Forms() native global
int[] Function FormIds() global
	{Forms() as a script reads it, -1 for a None. DevBench itself crashes on a Form[] when the address of Form's type object has bit 3 set.}
	Form[] forms = Forms()
	int[] ids = new int[3]
	int i = 0
	while i < forms.Length && i < ids.Length
		if forms[i]
			ids[i] = forms[i].GetFormID()
		else
			ids[i] = -1
		endif
		i += 1
	endwhile
	return ids
EndFunction
int Function DoubleNow(int x) native global
int Function DoubleLater(int x) native global
Function RecordEvent(int count, string text) native global
int Function EventCount() native global
Function RecordRelay(string kind, int count) native global
