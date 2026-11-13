# skyrim-rs example plugin

A small skyrim-rs plugin that scales the health damage the player takes. The factor starts at 1.0 (no change) and persists; it is a slider in the plugin's tab of the skyrim-rs menu, applied by a hook and settable from Papyrus. Its source is skyrim-rs's example mod (`crates/example-mod`).

## Requirements

The Skyrim Runtime Extender loader (`Data\SKSE\Plugins\skyrim_runtime_extender.dll`), from its own package, with its requirements: Skyrim Special Edition 1.7.104 from Steam, SKSE 2.3.1 and Address Library for SKSE Plugins.

## Install

This mod is a template and is not packaged for release. Build it from this repository and install its DLL and compiled script by hand, so you end up with `Data\SKSE\Plugins\skyrim-runtime-extender\skyrim_rs_example_mod.dll` and `Data\Scripts\SkyrimRsExample.pex`. `skyrim-runtime-extender.log` then says `loaded skyrim_rs_example_mod.dll: skyrim-rs-example`.

## What to try

- Open the skyrim-rs menu (F11). Under Mods, the tab "Example mod" has two sections. Damage has the "damage scale" slider (0 to 3): set it, take a hit, and the "Last hit" line shows the damage before and after scaling. About says what the example does.
- Under Debug, Plugins, "Example mod" has a "Verbose log" checkbox, the number of hits scaled and a "Reset count" button. Tick the checkbox and every scaled hit adds a line `hit scaled: -10 became -5` to `skyrim-rs-example.log`; the checkbox stays as you left it the next time you start the game.
- In the game (menu closed), Shift+K opens the menu straight on the example's tab (`skyrim-rs-example.log` reads `open-tab fired on Shift+K: tab shown`). The menu's Settings tab lists the key as "Example mod: open-tab", where you can bind another.
- `SkyrimRs hooks` in the console lists the example's hook on `Actor::ModActorValueImpl` with its call count; `SkyrimRs toggle <n>` on it switches the scaling off and on again.
- The Papyrus native `SkyrimRsExample.SetDamageScale(float)` sets the factor and returns the value in effect. Any script can call it with `SkyrimRsExample.pex` in `Data\Scripts`, for example `SkyrimRsExample.SetDamageScale(1.5)`.
- The factor is kept in `Data\SKSE\Plugins\skyrim-runtime-extender\skyrim-rs-example.settings.json`, written after the first change as `{"damage_scale": 0.5}`; start the game again and the slider shows your value. Edit it with the game closed.

Send back `skyrim-rs-example.log` and `skyrim-runtime-extender.log` from `Documents\My Games\Skyrim Special Edition\SKSE\` with anything that looks wrong.
