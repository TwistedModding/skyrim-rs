# Skyrim Runtime Extender

Skyrim Runtime Extender is a plugin loader for Skyrim Special Edition. `skyrim_runtime_extender.dll` is an SKSE plugin that loads its own plugins from `Data\SKSE\Plugins\skyrim-runtime-extender\`, draws an in-game menu styled like the game's own, gives every plugin a tab, settings, hotkeys and hot reload, and carries a crash logger that writes a readable report when the game dies. This package holds the loader alone; plugins come in their own packages (the example plugin, Improved Camera, Reversed Z). Nothing in it changes the game on its own.

## Requirements

- Skyrim Special Edition from Steam or GOG, one of these versions, with its script extender:

  | Game | SKSE | Address Library file in `Data\SKSE\Plugins` |
  |---|---|---|
  | 1.7.104 | 2.3.1 | `versionlib-1-7-104-0.bin` |
  | 1.6.1170 | 2.2.6 | `versionlib-1-6-1170-0.bin` |
  | 1.6.1179 (GOG) | 2.2.6 (GOG) | `versionlib-1-6-1179-0.bin` |
  | 1.6.353 | 2.1.5 | `versionlib-1-6-353-0.bin` |
  | 1.5.97 | 2.0.19 | `version-1-5-97-0.bin` |

  On any other version the loader stops with a `game layer:` line in its log. On GOG, the logs and crash files this README places in `Documents\My Games\Skyrim Special Edition\SKSE\` are in `Documents\My Games\Skyrim Special Edition GOG\SKSE\`.
- Address Library for SKSE Plugins (https://www.nexusmods.com/skyrimspecialedition/mods/32444), the file for your game version from the table above.

## Install

1. Install this package as a mod with your mod manager, or copy its `SKSE` folder into the game's `Data` folder. You end up with `Data\SKSE\Plugins\skyrim_runtime_extender.dll` and `Data\SKSE\Plugins\!!!skyrim-runtime-extender-timing.dll`. The second one only measures how long each SKSE plugin takes to load, for the Debug tab; its name makes the script extender load it before every other plugin. Leave it out if you don't want that.
2. Install any Skyrim Runtime Extender plugins you want; each package puts its DLL in `Data\SKSE\Plugins\skyrim-runtime-extender\`.
3. Start the game through `skse64_loader.exe` and load a save or start a new game.
4. Check `Documents\My Games\Skyrim Special Edition\SKSE\skyrim-runtime-extender.log`. It should say which loader hosts it (`hosted by the script extender, ...`), that the crash handler is installed (`crash handler installed, ...`), which font and cursor the menu uses (`overlay font: Futura CondensedLight from ...`, `overlay cursor: the game's, from ...cursormenu.swf`) and one `loaded <file>: <plugin>` line per plugin.

Ignore `optional-standalone` unless you have a game without SKSE; its own README explains it.

Upgrading from a build named `skyrim_rs.dll`: remove that DLL, or this loader refuses to start. On the first start, settings and other files from `Data\SKSE\Plugins\skyrim-rs\` are copied to the new folder, keybinds included; plugin DLLs are not, so reinstall your plugins.

## What to try

- Press F11 in game. The Skyrim Runtime Extender menu opens, in the look of the journal, with five tabs across the top: Mods, Plugins, Legacy plugins, Settings and Debug. Click a tab to switch. Drag the bottom-right corner to resize the menu (down to 820 by 500; the size is kept until you quit the game). F11 closes it again, and so does Escape or the "Exit" at the bottom right. While it is open the mouse belongs to the menu (the cursor is the game's own); the keyboard still moves the player unless you click a text field or press Tab. Escape then releases the field first and closes the menu on the second press. With the menu closed, Escape is the game's as usual.
- Mods: a sidebar lists every plugin that registered a tab, with its sections under it. Click the selected plugin to collapse or expand its sections; the choice is kept until you quit the game. With no plugin installed it is empty.
- Plugins: the loader's version and which loader hosts it, then every Skyrim Runtime Extender plugin with its version, file and status (`Running`, `Running, reloaded once` after a hot reload, `Stopped`), and any DLL in `Data\SKSE\Plugins\skyrim-runtime-extender\` the loader refused (`Not loaded`; hover it for the reason).
- Legacy plugins: every SKSE plugin in `Data\SKSE\Plugins` with its name, version, author, file and whether it is loaded. Why a plugin was not loaded is in SKSE's own log (`skse64.log`).
- Settings: every hotkey, the loader's ("Open and close this menu" F11, "Close this menu" Escape) and each plugin's, with a "Set" button that takes the next key you press, modifiers included, and an "Unbind" button that leaves the hotkey without a key. That includes the menu's own keys: with "Open and close this menu" unbound, open the menu with `SkyrimRs gui` in the console, or restore the key in the settings file. Hover a key to see what the game itself does with it, and a name to see its default. Two hotkeys never share a key: setting one onto a key another holds is refused and the refusal shows under the list. Below the keys, under Input, "Keys reach the game while no field in this menu has the focus": ticked by default; untick it and the menu keeps every key while it is open.
- Debug: Load times (how long the game took to reach each startup step, each SKSE plugin's load and message handling, each Skyrim Runtime Extender plugin's load), Data files (every loaded .esm, .esp and .esl with its load order index, type and read time, and how many full and light slots are used), Logs (every log in the SKSE log folder, with a filter) and System (Windows, processor, memory, graphics card and driver, and how much the game uses).
- The console (`~`) has `SkyrimRs` for checking what the menu shows and for the developer tools that are not in the menu: `SkyrimRs gui` (opens the menu and lists its pages, the plugins and the SKSE plugins), `SkyrimRs gui <n>` (opens it on page `n`), `SkyrimRs keys` (lists the hotkeys, numbered from 0, with their defaults and the last change), `SkyrimRs bind <n>` (takes the next key pressed for hotkey `n`, as its "Set" button does), `SkyrimRs hooks` (lists every hook site, its subscribers with call counts and timings, and the loader's last messages), `SkyrimRs toggle <n>` (switches subscriber `n` off and on again; its calls stop counting while it is off) and `SkyrimRs capture` (writes the next frame of the menu to `skyrim-runtime-extender-overlay.bmp` in the SKSE log folder; the game's own screenshot never shows the menu).
- `Data\SKSE\Plugins\skyrim-runtime-extender\skyrim-runtime-extender.settings.json` is the loader's settings file, plain JSON written with its defaults on the first run: every hotkey as `"key.skyrim-runtime-extender.toggle": "F11"`, `"key.skyrim-runtime-extender.close": "Escape"` and one line per plugin hotkey (a key name such as `"F10"` or `"Ctrl+Shift+K"`, any case, or `""` for unbound; remove the line to restore the default), and the capture rule `"overlay.capture": "focus"`. Edit it with the game closed: the loader reads it once at start and writes it within a second of a change in game. A file that does not parse is reported in `skyrim-runtime-extender.log` with the line and column, renamed to `skyrim-runtime-extender.settings.json.bad` and replaced by the defaults. Each plugin keeps its own `<plugin>.settings.json` in the same folder.
- Hooks chain with other SKSE plugins by default, including Community Shaders. No compatibility INI is required. `SkyrimRs hooks` identifies foreign patches and any individual subscriptions blocked because their plugin requires an exclusive hook. For troubleshooting, `compat = refuse` in `Data\SKSE\Plugins\skyrim-runtime-extender.ini` disables chaining globally; remove that setting to restore the default.
- The menu supports Community Shaders HDR mode without a CS configuration change. Visibility was tested with HDR Display 1.3.1 in the Discord development build `8f5fbca50`.

## Plugins and hot reload

Plugins talk to the loader through a stable C interface, so a plugin does not have to come from the same build as `skyrim_runtime_extender.dll`: it loads into any loader that has the functions it uses, and a function an older loader lacks reports Unsupported instead of crashing. Plugins built before the stable interface are refused with a line in `skyrim-runtime-extender.log` asking to rebuild them.

Replace a plugin's DLL in `Data\SKSE\Plugins\skyrim-runtime-extender\` while the game runs and the loader swaps the new build in within a second, logging every step in `skyrim-runtime-extender.log`. The Plugins tab's "Reload", "Unload", "Load" and "Rescan folder" buttons do the same without a new file.

## Crash logger

When the game dies with an unhandled exception, the loader writes two files to `Documents\My Games\Skyrim Special Edition\SKSE\`: `skyrim-runtime-extender-crash-<yyyymmdd-hhmmss>.dmp`, a minidump, and `skyrim-runtime-extender-crash-<yyyymmdd-hhmmss>.log`, the report. The log starts with a `Summary` block: the exception and where it happened, the address accessed, a plain explanation, the likely culprit and a crash signature, followed by the loader's facts, the registers, the call stack with game function names, every thread, every module and what the game objects on the stack were. It runs next to Crash Logger SSE if you have it; both write their logs. A panic in the loader or in a plugin built on it gets the same report, with the panic's source line and message in the summary. Whenever the game ends, `skyrim-runtime-extender.log` gets a last line, `the game ended: exit code ...`, naming the thread and the modules that ended it, so a quit to the desktop without any crash report still says who closed the game.

## What to send back

- `skyrim-runtime-extender.log` and each plugin's log from `Documents\My Games\Skyrim Special Edition\SKSE\`; `skse64.log` from the same folder helps too.
- If the game crashes: both `skyrim-runtime-extender-crash-<timestamp>.log` and `skyrim-runtime-extender-crash-<timestamp>.dmp` from the same folder. If you also run Crash Logger SSE (https://www.nexusmods.com/skyrimspecialedition/mods/59818), its log as well.
- How you installed (by hand or which mod manager), your Windows version, and what you did before anything went wrong. `skyrim-runtime-extender-overlay.bmp` from `SkyrimRs capture` is welcome when the menu looks off, and `skyrim-runtime-extender.settings.json` when a key does not do what you expect.
