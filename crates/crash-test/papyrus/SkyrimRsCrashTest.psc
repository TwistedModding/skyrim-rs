Scriptname SkyrimRsCrashTest

; The crash suite's script side: Crash is the test plugin's native that faults on purpose;
; CrashFromScript calls it from script code so the crash log shows a script frame under the native.
Function Crash() global native

Function CrashFromScript() global
    Crash()
EndFunction
