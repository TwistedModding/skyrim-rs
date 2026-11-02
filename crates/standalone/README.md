# Standalone loader (optional)

Only for a game without SKSE. If you run SKSE, skip this folder: with the script extender in the process this DLL does nothing but write `the script extender is loaded; it hosts the plugins` to `skyrim-runtime-extender-standalone.log`.

`dinput8.dll` is a proxy for the system's DirectInput DLL that loads the SKSE plugins in `Data\SKSE\Plugins` itself (`skyrim_runtime_extender.dll`, its plugins and other SKSE plugins alike) and gives them the plugin interface SKSE 2.3.1 gives, except Scaleform, serialization (co-saves) and the object table, which it does not implement. Plugins that need those will not work under it.

1. Install the main package as its README says. The Address Library file is still required.
2. Copy `dinput8.dll` next to `SkyrimSE.exe`.
3. Put a file named `steam_appid.txt` holding `489830` next to `SkyrimSE.exe`. Without it the game asks Steam to relaunch it through the launcher.
4. Start `SkyrimSE.exe` directly.

It logs to `skyrim-runtime-extender-standalone.log` in `Documents\My Games\Skyrim Special Edition\SKSE\`; `skyrim-runtime-extender.log` then says `hosted by the standalone loader, ...`. Send both back with anything from this mode. Remove `dinput8.dll` and `steam_appid.txt` to go back to normal.
