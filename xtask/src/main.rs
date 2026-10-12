use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() != Some("gen") {
        eprintln!("usage: cargo xtask gen    (reads SKYRIMRE_DIR or ../{})", xtask::RE_REPO);
        return ExitCode::FAILURE;
    }
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
        "classes {} exact + {} partial, fields {} ({} with a semantic meaning), asserts {} size + {} offset, vtables {} with {} slots ({} named, {} typed), functions {} ({} typed), globals {}, enum values {}, flags {}",
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
        stats.flags
    );
    ExitCode::SUCCESS
}
