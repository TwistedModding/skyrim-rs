//! The console command `SkyrimRs`: `gui [n]` opens the overlay (on page `n`) and lists the pages,
//! the framework plugins, the SKSE plugins, the plugin names served and the interfaces exported; `hooks` lists every hook site with its entry state
//! and subscribers, every byte patch, the plugins' event sinks and the dispatcher's last messages; `commands` lists the console
//! commands plugins registered or replaced and the Papyrus natives they bound; `toggle <id>` switches a subscriber or a patch; `keys`
//! lists the hotkeys and the input filters; `bind <n>` captures the next key pressed for hotkey `n` as the Settings tab's
//! "Set" button does; `capture [n]` writes the next overlay frame to `skyrim-rs-overlay.bmp`
//! (`skyrim-rs-overlay-<n>.bmp`) in the SKSE log folder.

use skyrim::input::{Arg, CommandContext, Param};
use sre::ScriptParamType;

use super::{HOOKS, NAME, alias, command_lines, filters, flush_settings, gui, interface, keys, sink_lines, skse_plugins};
use crate::hooks::{Hooks, PatchId, PatchState, SiteReport, SubscriptionId};

/// How a site's entry looked at install and looks now, the module behind another module's patch,
/// and whether the site broke (its detour overwritten in refuse mode: the subscribers no longer run).
fn site_state(site: &SiteReport) -> String {
    let mut state = if site.validation == site.current { format!("entry {}", site.current) } else { format!("entry {} at install, {} now", site.validation, site.current) };
    if let Some(by) = &site.patched_by {
        state.push_str(", patched by ");
        state.push_str(by);
    }
    if site.broken {
        state.push_str(", broken: its subscribers no longer run");
    }
    state
}

fn gui_lines() -> Vec<String> {
    let mut lines = vec![format!("SkyrimRs: overlay open, font {}, capture {:?}", overlay::font_source(), overlay::capture())];
    lines.extend(gui::pages().iter().enumerate().map(|(i, (page, shown))| format!("page {i}: {page}{}", if *shown { ", shown" } else { "" })));
    for (rank, r) in gui::plugin_rows().iter().enumerate() {
        let reason = if r.reason.is_empty() { String::new() } else { format!(": {}", r.reason) };
        lines.push(format!("plugin {rank}: {} {} ({}): {}{reason}", r.name, r.version, r.file, r.status));
    }
    lines.push(format!("legacy plugins, loaded by {}:", gui::hosted_by()));
    for p in skse_plugins() {
        let state = if p.stub { "a stub" } else if p.loaded { "loaded" } else { "not loaded" };
        lines.push(match &p.data {
            Some(d) => format!("legacy {}: {} {} by {}, {state}", p.file, d.name, gui::version_text(d.version), d.author),
            None => format!("legacy {}: no version data, {state}", p.file),
        });
    }
    lines.extend(alias::lines());
    lines.extend(interface::lines());
    lines
}

fn key_lines() -> Vec<String> {
    let focus = if overlay::keyboard_focus() { "a widget has the keyboard focus" } else { "no widget has the keyboard focus" };
    let mut lines = vec![format!("SkyrimRs: overlay {}, {focus}", if overlay::is_open() { "open" } else { "closed" })];
    for (i, b) in keys::list().iter().enumerate() {
        let game = if b.game.is_empty() { String::new() } else { format!(", game: {}", b.game) };
        lines.push(format!("key {i} {}: {} (default {}, {:?}{game})", b.name, keys::show(b.key), keys::show(b.default), b.scope));
    }
    lines.extend(filters::lines());
    let last = keys::last_message();
    if !last.is_empty() {
        lines.push(format!("keys: {last}"));
    }
    lines
}

fn hook_lines() -> Vec<String> {
    let mut lines = vec![format!("hook compatibility mode {}", Hooks::compat())];
    for site in HOOKS.diagnostics() {
        lines.push(format!("site {} at {:#x}: {} calls, last {} ns, mean {} ns, last outcome {:?}, {}", site.name, site.address, site.timing.calls, site.timing.last_ns, site.timing.mean_ns, site.last_outcome, site_state(&site)));
        lines.extend(site.entries.iter().map(|e| format!("  {e}")));
    }
    lines.extend(HOOKS.patches().iter().map(ToString::to_string));
    lines.extend(sink_lines());
    lines.extend(HOOKS.messages().iter().map(|m| format!("message: {m}")));
    lines
}

/// Switches subscription or patch `id`.
fn toggle(id: u64) -> String {
    let word = |enabled: bool| if enabled { "enabled" } else { "disabled" };
    let sub = SubscriptionId::new(id);
    if let Some(enabled) = HOOKS.diagnostics().iter().flat_map(|site| &site.entries).find(|e| e.id == sub).map(|e| e.enabled) {
        return if HOOKS.set_enabled(sub, !enabled) { format!("#{id} {}", word(!enabled)) } else { format!("no subscription #{id}") };
    }
    let patch = PatchId::new(id);
    match HOOKS.patches().into_iter().find(|p| p.id == patch) {
        Some(p) if p.state != PatchState::Applied && p.state != PatchState::Disabled => p.to_string(),
        Some(p) => {
            let applied = p.state == PatchState::Applied;
            HOOKS.set_patch_enabled(patch, !applied).map_or_else(|e| e.to_string(), |()| format!("patch #{id} {}", word(!applied)))
        }
        None => format!("no subscription or patch #{id}"),
    }
}

static PARAMS: [Param; 2] = [Param::new(c"what", ScriptParamType::String, false), Param::new(c"n", ScriptParamType::Integer, true)];

/// `SkyrimRs gui [n] | hooks | toggle <id> | keys | commands | bind <n> | capture [n]`.
pub(super) struct Command;

impl skyrim::input::Command for Command {
    const NAME: &'static core::ffi::CStr = c"SkyrimRs";
    const HELP: &'static core::ffi::CStr = c"skyrim-rs: gui [page], hooks, toggle <subscription id>, keys, commands, bind <key number>, capture [number]";
    const PARAMS: &'static [Param] = &PARAMS;

    fn execute(context: &mut CommandContext<'_>) -> bool {
        let is = |what: &str, word: &str| what.eq_ignore_ascii_case(word);
        let lines = match context.args() {
            [Arg::Str(what), Arg::Int(n)] if is(what, "gui") => {
                if usize::try_from(*n).is_ok_and(gui::show_page) {
                    overlay::set_open(true);
                    gui_lines()
                } else {
                    vec![format!("SkyrimRs: no page {n}")]
                }
            }
            [Arg::Str(what), ..] if is(what, "gui") => {
                overlay::set_open(true);
                gui_lines()
            }
            [Arg::Str(what), ..] if is(what, "keys") => key_lines(),
            [Arg::Str(what), ..] if is(what, "commands") => command_lines(),
            [Arg::Str(what), Arg::Int(n)] if is(what, "bind") => {
                let action = usize::try_from(*n).ok().and_then(|n| keys::list().get(n).map(|b| (n, b.name.clone())));
                vec![match action {
                    Some((n, name)) => {
                        gui::select_settings();
                        overlay::set_open(true);
                        keys::capture(Some(n));
                        format!("SkyrimRs: press the key for {name}")
                    }
                    None => format!("SkyrimRs: no key {n}"),
                }]
            }
            [Arg::Str(what), ..] if is(what, "hooks") => hook_lines(),
            [Arg::Str(what), Arg::Int(id)] if is(what, "toggle") => vec![format!("SkyrimRs: {}", u64::try_from(*id).map_or_else(|_| format!("no subscription #{id}"), toggle))],
            [Arg::Str(what), rest @ ..] if is(what, "capture") => {
                let file = match rest {
                    [Arg::Int(n)] => format!("skyrim-rs-overlay-{n}.bmp"),
                    _ => "skyrim-rs-overlay.bmp".to_owned(),
                };
                let path = super::LOG_DIR.get().cloned().unwrap_or_default().join(file);
                overlay::save_next_frame(path.clone(), |done| match done {
                    Ok(path) => log!("overlay frame written to {}", path.display()),
                    Err(e) => log!("overlay frame not written: {e}"),
                });
                vec![format!("SkyrimRs: the next overlay frame goes to {}", path.display())]
            }
            _ => vec!["SkyrimRs: gui [page], hooks, toggle <subscription id>, keys, commands, bind <key number> or capture [number]".to_owned()],
        };
        flush_settings(Some(NAME));
        for line in lines {
            skyrim::input::print(&line);
        }
        true
    }
}
