//! The RE repository files the generator reads, hashed as they are loaded.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
pub struct Symbol {
    pub id: Option<u64>,
    #[serde(deserialize_with = "hex")]
    pub rva: u64,
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub confidence: String,
    pub evidence: String,
    pub sig_confidence: String,
}

#[derive(Deserialize)]
pub struct Field {
    pub off: u64,
    pub size: u64,
    #[serde(rename = "type")]
    pub ty: String,
    pub name: String,
    #[serde(default)]
    pub src: String,
}

#[derive(Deserialize)]
pub struct Layout {
    pub size: Option<u64>,
    pub size_conf: Option<String>,
    pub fields: Vec<Field>,
    pub primary_base: Option<String>,
    pub size_lower: u64,
    pub size_upper: Option<u64>,
    pub size_kind: String,
    pub size_evidence: String,
}

#[derive(Deserialize)]
pub struct NoRttiStruct {
    pub size: u64,
    #[serde(default)]
    pub lower_bound: bool,
    pub fields: Vec<Field>,
    pub description: String,
}

#[derive(Deserialize)]
pub struct Label {
    pub rva: u64,
    pub cls: String,
    pub pointer: bool,
    pub detail: String,
}

#[derive(Deserialize)]
pub struct ClassStructs {
    pub structs: BTreeMap<String, NoRttiStruct>,
    pub labels: Vec<Label>,
}

#[derive(Deserialize)]
pub struct Col {
    pub offset: u64,
    pub vtable: u64,
    pub slots: Vec<u64>,
}

#[derive(Deserialize)]
pub struct Base {
    pub name: String,
    pub mdisp: i64,
    pub pdisp: i64,
}

#[derive(Deserialize)]
pub struct RttiClass {
    pub name: String,
    pub cols: Vec<Col>,
    pub bases: Vec<Base>,
}

#[derive(Deserialize)]
pub struct Rtti {
    pub classes: Vec<RttiClass>,
}

#[derive(Deserialize)]
pub struct FormType {
    pub id: u64,
    pub sig: String,
    pub cls: Option<String>,
}

#[derive(Deserialize)]
pub struct SemField {
    pub offset: u64,
    pub name: String,
    pub meaning: String,
    pub confidence: String,
}

#[derive(Deserialize)]
pub struct SemClass {
    pub name: String,
    pub meaning: String,
    pub confidence: String,
    pub fields: Vec<SemField>,
}

#[derive(Deserialize)]
pub struct SemFunction {
    pub id: Option<u64>,
    pub name: String,
    pub signature: String,
    pub meaning: String,
    pub confidence: String,
}

#[derive(Deserialize)]
pub struct SemEnumValue {
    pub value: u64,
    pub name: String,
}

#[derive(Deserialize)]
pub struct SemEnum {
    pub name: String,
    pub width: u64,
    pub meaning: String,
    pub values: Vec<SemEnumValue>,
}

#[derive(Deserialize)]
pub struct SemFlag {
    pub value: u64,
    pub name: String,
    pub meaning: String,
    pub confidence: String,
}

#[derive(Deserialize)]
pub struct SemFlagSet {
    pub name: String,
    pub width: u64,
    pub values: Vec<SemFlag>,
}

#[derive(Deserialize)]
pub struct SemSlot {
    pub index: u64,
    pub name: String,
    pub signature: String,
    pub meaning: String,
    pub confidence: String,
}

#[derive(Deserialize)]
pub struct SemVtable {
    pub class: String,
    pub slots: Vec<SemSlot>,
}

#[derive(Deserialize)]
pub struct SemGlobal {
    pub id: Option<u64>,
    #[serde(rename = "type")]
    pub ty: String,
    pub meaning: String,
    pub accessor_id: Option<u64>,
    pub confidence: String,
}

/// One `export/semantic/<subsystem>.json`.
#[derive(Deserialize)]
pub struct Semantic {
    pub subsystem: String,
    pub classes: Vec<SemClass>,
    pub functions: Vec<SemFunction>,
    pub enums: Vec<SemEnum>,
    pub flags: Vec<SemFlagSet>,
    pub vtables: Vec<SemVtable>,
    pub globals: Vec<SemGlobal>,
}

pub struct Inputs {
    pub symbols: Vec<Symbol>,
    pub layouts: BTreeMap<String, Layout>,
    pub structs: ClassStructs,
    pub rtti: Rtti,
    pub formtypes: Vec<FormType>,
    pub semantic: Vec<Semantic>,
    /// `(relative path, sha256)` of every file read, in read order.
    pub hashes: Vec<(String, String)>,
}

fn hex<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let s = String::deserialize(d)?;
    u64::from_str_radix(s.trim_start_matches("0x"), 16).map_err(serde::de::Error::custom)
}

pub fn read(re_dir: &Path) -> Result<Inputs, Box<dyn Error>> {
    let mut hashes = Vec::new();
    let mut load = |rel: &str| -> Result<Vec<u8>, Box<dyn Error>> {
        let bytes = fs::read(re_dir.join(rel)).map_err(|e| format!("{}: {e}", re_dir.join(rel).display()))?;
        hashes.push((rel.to_owned(), format!("{:x}", Sha256::digest(&bytes))));
        Ok(bytes)
    };
    let symbols = csv::Reader::from_reader(load("export/symbols.csv")?.as_slice())
        .deserialize()
        .collect::<Result<Vec<Symbol>, _>>()?;
    let layouts = serde_json::from_slice(&load("data/layouts.json")?)?;
    let structs = serde_json::from_slice(&load("data/class_structs.json")?)?;
    let rtti = serde_json::from_slice(&load("data/rtti.json")?)?;
    let formtypes = serde_json::from_slice(&load("data/formtypes.json")?)?;
    let mut names: Vec<String> = fs::read_dir(re_dir.join("export/semantic"))?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| std::path::Path::new(n).extension().is_some_and(|e| e == "json"))
        .collect();
    names.sort();
    let mut semantic = Vec::new();
    for name in names {
        semantic.push(serde_json::from_slice(&load(&format!("export/semantic/{name}"))?)?);
    }
    Ok(Inputs { symbols, layouts, structs, rtti, formtypes, semantic, hashes })
}
