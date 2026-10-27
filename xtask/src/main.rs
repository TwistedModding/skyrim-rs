use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("gen") => generate(),
        Some("crash") if args.len() >= 3 => decode(Path::new(&args[2]), args.get(3).map(Path::new)),
        _ => {
            eprintln!("usage: cargo xtask gen                      (reads SKYRIMRE_DIR or ../{})", xtask::RE_REPO);
            eprintln!("       cargo crash-decode <file.dmp> [out.log]  (the crash log from a minidump; stdout without out.log; the decode feature)");
            ExitCode::FAILURE
        }
    }
}

fn generate() -> ExitCode {
    let re = xtask::re_dir();
    let (files, stats) = match xtask::generate(&re, &xtask::commit_of(&re)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let root = xtask::workspace_root();
    match std::fs::remove_dir_all(root.join(xtask::MODULE_DIR)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            eprintln!("{}: {e}", xtask::MODULE_DIR);
            return ExitCode::FAILURE;
        }
        _ => {}
    }
    for (path, text) in &files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        println!("wrote {} ({} bytes)", path.display(), text.len());
    }
    println!(
        "classes {} exact + {} partial, fields {} ({} with a semantic meaning), asserts {} size + {} offset, vtables {} with {} slots ({} named, {} typed), functions {} ({} typed), globals {}, enum values {}, flags {}, symbol table {}",
        stats.exact_classes,
        stats.partial_classes,
        stats.fields,
        stats.field_meanings,
        stats.size_asserts,
        stats.offset_asserts,
        stats.vtables,
        stats.slots,
        stats.named_slots,
        stats.typed_slots,
        stats.functions,
        stats.typed_functions,
        stats.globals,
        stats.enum_values,
        stats.flags,
        stats.symbols
    );
    let api = &stats.api;
    println!(
        "derefs {} ({} refused), base accessors {}, field accessors {}, singletons {}, statics {}, functions reached only as slots {}, left out: vfunc_<n> names {}, CRT initializers {}",
        api.derefs, api.derefs_refused, api.base_accessors, api.field_accessors, api.singletons, api.statics, api.virtual_only, api.vfunc_names, api.initializers
    );
    for (tier, t) in &api.tiers {
        println!(
            "{:>9}: methods {} safe + {} unsafe, associated {} + {}, free {} + {}, slots {} + {}, unexpressible {} ({})",
            tier.label(),
            t.safe_methods,
            t.unsafe_methods,
            t.safe_associated,
            t.unsafe_associated,
            t.safe_free,
            t.unsafe_free,
            t.safe_slots,
            t.unsafe_slots,
            t.unexpressible.values().sum::<usize>(),
            t.unexpressible.iter().map(|(why, n)| format!("{} {n}", why.label())).collect::<Vec<_>>().join(", ")
        );
    }
    ExitCode::SUCCESS
}

#[cfg(not(feature = "decode"))]
fn decode(_: &Path, _: Option<&Path>) -> ExitCode {
    eprintln!("the decoder is behind the `decode` feature: cargo run -p xtask --features decode -- crash <file.dmp> [out.log]");
    ExitCode::FAILURE
}

#[cfg(feature = "decode")]
fn decode(dump: &Path, out: Option<&Path>) -> ExitCode {
    let text = match xtask::decode(dump) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("{}: {e}", dump.display());
            return ExitCode::FAILURE;
        }
    };
    let Some(path) = out else {
        print!("{text}");
        return ExitCode::SUCCESS;
    };
    match std::fs::write(path, &text) {
        Ok(()) => {
            println!("wrote {} ({} bytes)", path.display(), text.len());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}
