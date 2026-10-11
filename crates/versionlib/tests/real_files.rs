use std::path::PathBuf;

use versionlib::VersionDb;

fn data_dir() -> PathBuf {
    std::env::var_os("VERSIONLIB_DIR")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data"), PathBuf::from)
}

fn open(name: &str) -> Option<VersionDb> {
    let path = data_dir().join(name);
    if !path.exists() {
        eprintln!("skipped: {} not present", path.display());
        return None;
    }
    Some(VersionDb::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

fn ids_for(db: &VersionDb, offset: u64) -> Vec<u64> {
    db.iter().filter(|&(_, o)| o == offset).map(|(i, _)| i).collect()
}

fn check(db: &VersionDb, format: u32, version: [u32; 4]) {
    assert_eq!(db.format(), format);
    assert_eq!(db.version(), version);
    assert!(db.len() > 100_000);
    let mut last = None;
    for (id, offset) in db.iter() {
        assert!(last.is_none_or(|l| l < id), "ids out of order at {id}");
        assert!(offset < 0x4000_0000, "offset {offset:#x} for id {id} is outside any Skyrim image");
        assert_eq!(db.offset(id), Some(offset));
        last = Some(id);
    }
    assert_eq!(db.offset(u64::MAX), None);
}

#[test]
fn se_1_5_97() {
    let Some(db) = open("version-1-5-97-0.bin") else { return };
    check(&db, 1, [1, 5, 97, 0]);
    assert_eq!(db.offset(2), Some(0x10D0));
    let ids = ids_for(&db, 0x0019_4230);
    eprintln!("SE LookupFormByID 0x194230 -> ids {ids:?}");
    assert_eq!(ids.len(), 1);
}

#[test]
fn ae_1_6_1170() {
    let Some(db) = open("versionlib-1-6-1170-0.bin") else { return };
    check(&db, 2, [1, 6, 1170, 0]);
    assert_eq!(db.offset(1), Some(0x1022));
}

#[test]
fn ae_1_7_99_v5() {
    let Some(db) = open("versionlib-1-7-99-0.bin") else { return };
    check(&db, 5, [1, 7, 99, 0]);
    assert_eq!(db.offset(0), None);
    assert_eq!(db.offset(1), Some(0x1022));
    assert_eq!(db.len(), 565_073);
}

#[test]
fn ae_1_7_104_v5() {
    let Some(db) = open("versionlib-1-7-104-0.bin") else { return };
    check(&db, 5, [1, 7, 104, 0]);
    assert_eq!(db.len(), 565_759);
    assert_eq!(db.iter().count(), 435_162);
    assert_eq!(db.iter().last(), Some((565_758, 0x0202_17B0)));
    assert_eq!(db.offset(14617), Some(0x001E_5860));
}

#[test]
fn ae_ids_are_stable_across_1_6_and_1_7() {
    let (Some(old), Some(new)) = (open("versionlib-1-6-1170-0.bin"), open("versionlib-1-7-104-0.bin")) else {
        return;
    };
    let ids = ids_for(&new, 0x001E_5860);
    eprintln!("AE LookupFormByID 0x1E5860 (1.7.104) -> ids {ids:?}");
    assert_eq!(ids.len(), 1);
    let in_old = old.offset(ids[0]);
    eprintln!("same id in 1.6.1170 -> {in_old:x?}");
    assert!(in_old.is_some());
    let shared = new.iter().filter(|&(id, _)| old.offset(id).is_some()).count();
    assert!(shared > new.len() / 2, "only {shared} ids shared");
}
