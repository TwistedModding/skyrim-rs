# drawcall-fix

A Skyrim Runtime Extender plugin for the CPU cost of Skyrim SE's shadow views: the sun's cascades, the focus maps and the local lights' maps are culled and turned into render passes once per view per frame, on the frame thread, and that work grows with every shadow caster in range. The plugin measures that work step by step (the measurement that drives the design) and carries the first optimisation, a per-frame memo across the cascade views. The engine facts and the runs are in the RE repository, `docs/subsystems/graphics.md` "Request DC".

## What it does

- `DrawcallFix profile <label>` records one CSV row per frame to `Documents\My Games\Skyrim Special Edition\SKSE\drawcall-fix-<label>.csv`: the time of every shadow and depth step of the frame (the shadow scene cull and its parts, each cascade's cull and draw, the local and focus maps, the depth pre-pass, the shadow masks, the colour draw), the counts of geometry registrations by accumulator mode, of `GetDepthRenderPasses` calls and of passes drawn; `DrawcallFix profile off` stops it. The detours cost a few hundred nanoseconds per frame plus about 10 ns per registration.
- The cascade memo (on by default, `DrawcallFix on|off`, setting `enabled`): `GetDepthRenderPasses` answers the same for every cascade within a frame, so a geometry the first cascade dropped is remembered and the later cascades' registration returns without the property call. A casting geometry needs a pass object per accumulator and is not cached. Rendering does not change.
- `DrawcallFix shadowdistance <units>` writes the live shadow distance and the setting, `DrawcallFix vsync <interval>` the swap chain's present interval, `DrawcallFix capture` asks RenderDoc for a frame when the game runs under it, `DrawcallFix` alone prints the state and the previous frame's counters.

## Measured (1.7.104, Riverwood save, noon, third person; `run-dc-novsync`)

Extra shadow-casting references spawned along a line inside the loaded cells; milliseconds on the frame thread per frame, medians. The frame interval is capped near 16.5 ms by the desktop compositor in this borderless setup, so only values above it are frame times.

| extra references | frame | RenderWorld | shadow scene cull | sun cascades cull | shadow map draws | depth pre-pass | colour draw | shadow side of RenderWorld |
|---|---|---|---|---|---|---|---|---|
| 0 | 16.5 | 2.9 | 0.59 | 0.50 | 0.66 | 0.32 | 0.89 | 43 % |
| 1,000 | 16.5 | 5.4 | 1.16 | 1.04 | 1.05 | 0.76 | 1.90 | 41 % |
| 3,000 | 16.6 | 11.1 | 2.44 | 2.29 | 2.29 | 1.72 | 4.04 | 43 % |
| 6,000 | 21.0 | 19.5 | 4.51 | 4.28 | 3.85 | 3.22 | 7.19 | 43 % |
| 10,000 | 23.1 | 21.5 | 6.28 | 6.02 | 4.94 | 3.00 | 6.55 | 52 % |

The cascade cull costs about 190 ns per registered geometry per cascade and the cascade draw about 170 ns per pass. At 10,000 extra references the two cascades alone are 10.9 ms of a 21.5 ms `RenderWorld`. RenderDoc captures of the same scene put 12,606 draws into the shadow targets against the plugin's 12,457 shadow passes at 3,000 references (`graphics.md` DC.4).

## Phase 1 result

The memo on against off at every level of one session (`run-dc-phase1`): no measurable change in any region. Nothing a cascade registers in this scene is dropped by `GetDepthRenderPasses`, so the memo never has an answer to give (at most one skip per frame); for a casting geometry the engine's own per-frame memo in the property's pass lists already reuses the pass object and the technique, and each accumulator still needs its own pass. The per-view cache question is answered by measurement: it is not where the time goes.

## Phase 2

The numbers justify taking the static casters off the per-geometry path altogether: a flat table of static casters culled on the CPU per cascade and drawn with one instanced draw per mesh into the engine's own cascade slices, with the engine's walk kept off those subtrees. The design, the engine facts it rests on and the open blockers (the scene event that maintains the table, the vertex format mapping, the per-pass cull and fade bytes, a per-pixel proof, and whether Community Shaders or ENB hook the shadow depth passes) are in `graphics.md` DC.6. It is not built yet.

## Install

`skyrim_rs_drawcall_fix.dll` goes into `Data\SKSE\Plugins\skyrim-runtime-extender\` under the loader; the log is `drawcall-fix.log` in the SKSE folder. Built and run on 1.7.104 with SKSE 2.3.1 only.
