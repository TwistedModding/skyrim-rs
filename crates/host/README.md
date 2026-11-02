# skyrim-rs loader

skyrim-rs is a plugin loader for Skyrim Special Edition. `skyrim_rs.dll` is an SKSE plugin that loads its own plugins from `Data\SKSE\Plugins\skyrim-rs\`, draws an in-game menu styled like the game's own, gives every plugin a tab, settings, hotkeys and hot reload, and carries a crash logger that writes a readable report when the game dies. This package holds the loader alone; plugins come in their own packages (the example plugin, Improved Camera, Reversed Z). Nothing in it changes the game on its own.

## Requirements

- Skyrim Special Edition 1.7.104 from Steam. No other game version is supported; on any other the loader stops with a `game layer:` line in its log.
- SKSE 2.3.1.
- Address Library for SKSE Plugins (https://www.nexusmods.com/skyrimspecialedition/mods/32444): the file `versionlib-1-7-104-0.bin` must be in `Data\SKSE\Plugins`.

## Install

1. Copy the `Data` folder of this package over the game's `Data` folder (or install the package as a mod with your mod manager). You end up with `Data\SKSE\Plugins\skyrim_rs.dll`.
2. Install any skyrim-rs plugins you want; each package puts its DLL in `Data\SKSE\Plugins\skyrim-rs\`.
3. Start the game through `skse64_loader.exe` and load a save or start a new game.
4. Check `Documents\My Games\Skyrim Special Edition\SKSE\skyrim-rs.log`. It should say which loader hosts it (`hosted by the script extender, ...`), that the crash handler is installed (`crash handler installed, ...`), which font and cursor the menu uses (`overlay font: Futura CondensedLight from ...`, `overlay cursor: the game's, from ...cursormenu.swf`) and one `loaded <file>: <plugin>` line per plugin.

Ignore `optional-standalone` unless you have a game without SKSE; its own README explains it.

## What to try

- Press F11 in game. The skyrim-rs menu opens, in the look of the journal, with four tabs across the top: Mods, Plugins, Legacy plugins and Settings. Click a tab to switch. Drag the bottom-right corner to resize the menu (down to 820 by 500; the size is kept until you quit the game). F11 closes it again, and so does Escape or the "Exit" at the bottom right. While it is open the mouse belongs to the menu (the cursor is the game's own); the keyboard still moves the player unless you click a text field or press Tab. Escape then releases the field first and closes the menu on the second press. With the menu closed, Escape is the game's as usual.
- Mods: a sidebar lists every plugin that registered a tab, with its sections under it. Click the selected plugin to collapse or expand its sections; the choice is kept until you quit the game. With no plugin installed it is empty; the example package adds the tab "Example mod".
- Plugins: the loader's version and which loader hosts it, then every skyrim-rs plugin with its version, file and status (`Running`, `Running, reloaded once` after a hot reload, `Stopped`), and any DLL in `Data\SKSE\Plugins\skyrim-rs\` the loader refused (`Not loaded`; hover it for the reason).
- Legacy plugins: every SKSE plugin in `Data\SKSE\Plugins` with its name, version, author, file and whether it is loaded. Why a plugin was not loaded is in SKSE's own log (`skse64.log`).
- Settings: every hotkey, the loader's ("Open and close this menu" F11, "Close this menu" Escape) and each plugin's, with a "Set" button that takes the next key you press, modifiers included, and an "Unbind" button that leaves the hotkey without a key. That includes the menu's own keys: with "Open and close this menu" unbound, open the menu with `SkyrimRs gui` in the console, or restore the key in the settings file. Hover a key to see what the game itself does with it, and a name to see its default. Two hotkeys never share a key: setting one onto a key another holds is refused and the refusal shows under the list. Below the keys, under Input, "Keys reach the game while no field in this menu has the focus": ticked by default; untick it and the menu keeps every key while it is open.
- The console (`~`) has `SkyrimRs` for checking what the menu shows and for the developer tools that are not in the menu: `SkyrimRs gui` (opens the menu and lists its pages, the plugins and the SKSE plugins), `SkyrimRs gui <n>` (opens it on page `n`), `SkyrimRs keys` (lists the hotkeys, numbered from 0, with their defaults and the last change), `SkyrimRs bind <n>` (takes the next key pressed for hotkey `n`, as its "Set" button does), `SkyrimRs hooks` (lists every hook site, its subscribers with call counts and timings, and the loader's last messages), `SkyrimRs toggle <n>` (switches subscriber `n` off and on again; its calls stop counting while it is off) and `SkyrimRs capture` (writes the next frame of the menu to `skyrim-rs-overlay.bmp` in the SKSE log folder; the game's own screenshot never shows the menu).
- `Data\SKSE\Plugins\skyrim-rs\skyrim-rs.settings.json` is the loader's settings file, plain JSON written with its defaults on the first run: every hotkey as `"key.skyrim-rs.toggle": "F11"`, `"key.skyrim-rs.close": "Escape"` and one line per plugin hotkey (a key name such as `"F10"` or `"Ctrl+Shift+K"`, any case, or `""` for unbound; remove the line to restore the default), and the capture rule `"overlay.capture": "focus"`. Edit it with the game closed: the loader reads it once at start and writes it within a second of a change in game. A file that does not parse is reported in `skyrim-rs.log` with the line and column, renamed to `skyrim-rs.settings.json.bad` and replaced by the defaults. Each plugin keeps its own `<plugin>.settings.json` in the same folder.
- Hooks chain with other SKSE plugins by default, including Community Shaders. No compatibility INI is required. `SkyrimRs hooks` identifies foreign patches and any individual subscriptions blocked because their plugin requires an exclusive hook. For troubleshooting, `compat = refuse` in `Data\SKSE\Plugins\skyrim-rs.ini` disables chaining globally; remove that setting to restore the default.
- The menu supports Community Shaders HDR mode without a CS configuration change. Visibility was tested with HDR Display 1.3.1 in the Discord development build `8f5fbca50`.

## Plugins and hot reload

Plugins talk to the loader through a stable C interface, so a plugin does not have to come from the same build as `skyrim_rs.dll`: it loads into any loader that has the functions it uses, and a function an older loader lacks reports Unsupported instead of crashing. Plugins built before the stable interface are refused with a line in `skyrim-rs.log` asking to rebuild them.

Replace a plugin's DLL in `Data\SKSE\Plugins\skyrim-rs\` while the game runs and the loader swaps the new build in within a second, logging every step in `skyrim-rs.log`. The Plugins tab's "Reload", "Unload", "Load" and "Rescan folder" buttons do the same without a new file.

## Crash logger

When the game dies with an unhandled exception, the loader writes two files to `Documents\My Games\Skyrim Special Edition\SKSE\`: `skyrim-rs-crash-<yyyymmdd-hhmmss>.dmp`, a minidump, and `skyrim-rs-crash-<yyyymmdd-hhmmss>.log`, the report. The log starts with a `Summary` block: the exception and where it happened, the address accessed, a plain explanation, the likely culprit and a crash signature, followed by the loader's facts, the registers, the call stack with game function names, every thread, every module and what the game objects on the stack were. It runs next to Crash Logger SSE if you have it; both write their logs.

## What to send back

- `skyrim-rs.log` and each plugin's log from `Documents\My Games\Skyrim Special Edition\SKSE\`; `skse64.log` from the same folder helps too.
- If the game crashes: both `skyrim-rs-crash-<timestamp>.log` and `skyrim-rs-crash-<timestamp>.dmp` from the same folder. If you also run Crash Logger SSE (https://www.nexusmods.com/skyrimspecialedition/mods/59818), its log as well.
- How you installed (by hand or which mod manager), your Windows version, and what you did before anything went wrong. `skyrim-rs-overlay.bmp` from `SkyrimRs capture` is welcome when the menu looks off, and `skyrim-rs.settings.json` when a key does not do what you expect.
