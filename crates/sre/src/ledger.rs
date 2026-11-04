//! What the ledger names, looked up at compile time by `member_offset!` and its kin, so that
//! hand-written code cannot name an item no profile can speak for.

use crate::generated::ledger_tables::{BASES, MEMBERS, SIZES, SLOTS};

/// The canonical offset of `Class.member`.
#[must_use]
pub const fn member(name: &str) -> Option<usize> {
    match value(MEMBERS, name) {
        Some((at, end)) => Some(hex(MEMBERS.as_bytes(), at, end)),
        None => None,
    }
}

/// The canonical displacement of the base `Class:Base`; a virtual base has none.
#[must_use]
pub const fn base(name: &str) -> Option<usize> {
    match value(BASES, name) {
        Some((at, end)) => Some(hex(BASES.as_bytes(), at, end)),
        None => None,
    }
}

/// Whether the ledger has a class of this name.
#[must_use]
pub const fn size(class: &str) -> bool {
    value(SIZES, class).is_some()
}

/// Whether the ledger has this slot: the vtable at `vtable_offset` of `class` holds more than `index` entries.
#[must_use]
pub const fn slot(class: &str, vtable_offset: u32, index: u32) -> bool {
    let Some((mut at, end)) = value(SLOTS, class) else { return false };
    let table = SLOTS.as_bytes();
    while at < end {
        let colon = until(table, at, b':');
        let stop = until(table, colon + 1, b',');
        let stop = if stop < end { stop } else { end };
        if hex(table, at, colon) == vtable_offset as usize {
            return (index as usize) < hex(table, colon + 1, stop);
        }
        at = stop + 1;
    }
    false
}

/// The bounds of the value on the line of `key`, by binary search over lines sorted by key.
const fn value(table: &str, key: &str) -> Option<(usize, usize)> {
    let (table, key) = (table.as_bytes(), key.as_bytes());
    let (mut low, mut high) = (0, table.len());
    while low < high {
        let mut line = low + (high - low) / 2;
        while line > low && table[line - 1] != b'\n' {
            line -= 1;
        }
        let tab = until(table, line, b'\t');
        let end = until(table, tab, b'\n');
        match compare(table, line, tab, key) {
            core::cmp::Ordering::Equal => return Some((tab + 1, end)),
            core::cmp::Ordering::Less => low = end + 1,
            core::cmp::Ordering::Greater => high = line,
        }
    }
    None
}

const fn compare(table: &[u8], start: usize, end: usize, key: &[u8]) -> core::cmp::Ordering {
    let mut i = 0;
    while start + i < end && i < key.len() {
        if table[start + i] != key[i] {
            return if table[start + i] < key[i] { core::cmp::Ordering::Less } else { core::cmp::Ordering::Greater };
        }
        i += 1;
    }
    if start + i < end {
        core::cmp::Ordering::Greater
    } else if i < key.len() {
        core::cmp::Ordering::Less
    } else {
        core::cmp::Ordering::Equal
    }
}

/// The first position of `byte` from `at`, or the end of the table.
const fn until(table: &[u8], mut at: usize, byte: u8) -> usize {
    while at < table.len() && table[at] != byte {
        at += 1;
    }
    at
}

const fn hex(table: &[u8], mut at: usize, end: usize) -> usize {
    let mut out = 0;
    while at < end {
        let digit = table[at];
        out = out * 16 + (if digit >= b'a' { digit - b'a' + 10 } else { digit - b'0' }) as usize;
        at += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_from_the_tables() {
        assert_eq!(member("PlayerCharacter.source_BGSActorCellEvent"), Some(0x2E0));
        assert_eq!(member("PlayerCharacter.noSuchMember"), None);
        assert_eq!(member("PlayerCharacter.source_BGSActorCellEven"), None, "a prefix is no match");
        assert_eq!(base("PlayerCharacter:BSTEventSource<BGSActorCellEvent>"), Some(0x2E0));
        assert_eq!(base("PlayerCharacter:NoSuchBase"), None);
        assert!(size("hkpListShape::ChildInfo") && !size("NoSuchClass"));
        assert!(slot("IMenu", 0, 8) && !slot("IMenu", 0, 900) && !slot("IMenu", 0x8, 0) && !slot("NoSuchClass", 0, 0));
        assert!(slot("NoRTTI_1A2ADE8", 0, 23), "a table only the difference files hold");
    }
}
