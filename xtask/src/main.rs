use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("gen") => generate(),
        _ => {
            eprintln!("usage: cargo xtask gen    (reads SKYRIMRE_DIR or ../{})", xtask::RE_REPO);
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
    ExitCode::SUCCESS
}
