use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("gen") => generate(),
        Some("crash") if args.len() >= 3 => decode(Path::new(&args[2]), args.get(3).map(Path::new)),
        Some("manifest") if args.len() == 3 => manifest(Path::new(&args[2]), None),
        Some("manifest") if args.len() == 5 && args[3] == "--check" => manifest(Path::new(&args[2]), Some(Path::new(&args[4]))),
        _ => {
            eprintln!("usage: cargo xtask gen                      (reads SKYRIMRE_DIR or ../{})", xtask::RE_REPO);
            eprintln!("       cargo xtask manifest <dll> [--check <profile>]  (what a DLL records it uses of the game; what a profile lacks of it)");
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
    for stale in xtask::generated_on_disk(&root).into_iter().filter(|p| !files.contains_key(p)) {
        let path = root.join(&stale);
        std::fs::remove_file(&path).unwrap();
        let _ = std::fs::remove_dir(path.parent().unwrap());
        println!("removed {}", path.display());
    }
    let mut unchanged = 0;
    for (path, text) in &files {
        let path = root.join(path);
        if std::fs::read(&path).is_ok_and(|old| old == text.as_bytes()) {
            unchanged += 1;
            continue;
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        println!("wrote {} ({} bytes)", path.display(), text.len());
    }
    println!("{unchanged} files unchanged");
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
    println!("runtimes compared: {}", if stats.compared.is_empty() { "none".to_owned() } else { stats.compared.join(", ") });
    if let Some(n) = stats.self_check_differences {
        println!("1.7.104 self-check: {n} rows not same");
    }
    for (group, c) in &stats.accessors {
        println!("accessors in {group}: fields {}, slots {}, bases {}, sizes {}", c.fields, c.slots, c.bases, c.sizes);
    }
    ExitCode::SUCCESS
}

fn manifest(dll: &Path, profile: Option<&Path>) -> ExitCode {
    let run = || -> Result<ExitCode, String> {
        let bytes = std::fs::read(dll).map_err(|e| format!("{}: {e}", dll.display()))?;
        let records = versionlib::manifest::read(&bytes).map_err(|e| format!("{}: {e}", dll.display()))?;
        let Some(path) = profile else {
            for record in &records {
                println!("{record}");
            }
            eprintln!("{} records", records.len());
            return Ok(ExitCode::SUCCESS);
        };
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let parsed = versionlib::profile::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let lacking = versionlib::manifest::lacking(&records, &parsed);
        for (record, why) in &lacking {
            println!("{record} {why}");
        }
        eprintln!("{} of {} records lacking", lacking.len(), records.len());
        Ok(if lacking.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
    };
    run().unwrap_or_else(|e| {
        eprintln!("{e}");
        ExitCode::FAILURE
    })
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
