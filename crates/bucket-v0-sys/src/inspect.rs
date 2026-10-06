//! A small WebAssembly reader: the import and export sections of a module, and the check of the imports against the documented
//! function set ([`crate::FUNCTIONS`]). Enough for a build step or a test to prove that a module links against `bucket_v0`.
use crate::{FUNCTIONS, MODULE};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What an import is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportKind {
    /// A function with this signature.
    Func {
        /// Parameter types (`"i32"`, `"i64"`, `"f32"`, `"f64"`, `"v128"`, `"funcref"`, `"externref"`).
        params: Vec<String>,
        /// Result types.
        results: Vec<String>,
    },
    /// A table.
    Table,
    /// A memory (`shared` for a shared memory).
    Memory {
        /// Shared memory (threads).
        shared: bool,
        /// Initial size in pages.
        min: u64,
        /// Maximum size in pages, if declared.
        max: Option<u64>,
    },
    /// A global.
    Global,
    /// An exception tag.
    Tag,
}

/// One import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// Module name.
    pub module: String,
    /// Field name.
    pub name: String,
    /// Kind.
    pub kind: ImportKind,
}

/// What a module imports and exports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleInfo {
    /// Imports in order.
    pub imports: Vec<Import>,
    /// Export names with their kind (0 function, 1 table, 2 memory, 3 global, 4 tag).
    pub exports: Vec<(String, u8)>,
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.i).ok_or("unexpected end of module")?;
        self.i += 1;
        Ok(v)
    }

    fn leb(&mut self) -> Result<u64, String> {
        let (mut v, mut shift) = (0u64, 0u32);
        loop {
            let b = self.byte()?;
            if shift < 64 {
                v |= u64::from(b & 0x7f) << shift;
            }
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
            if shift > 70 {
                return Err("LEB128 too long".to_string());
            }
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.i.checked_add(n).filter(|e| *e <= self.b.len()).ok_or("unexpected end of module")?;
        let s = &self.b[self.i..end];
        self.i = end;
        Ok(s)
    }

    fn name(&mut self) -> Result<String, String> {
        let n = self.leb()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| "a name is not UTF-8".to_string())
    }
}

fn val_type(b: u8) -> Result<String, String> {
    Ok(match b {
        0x7f => "i32",
        0x7e => "i64",
        0x7d => "f32",
        0x7c => "f64",
        0x7b => "v128",
        0x70 => "funcref",
        0x6f => "externref",
        other => return Err(format!("unknown value type 0x{other:02x}")),
    }
    .to_string())
}

fn limits(r: &mut Reader<'_>) -> Result<(bool, u64, Option<u64>), String> {
    let flag = r.byte()?;
    let min = r.leb()?;
    let max = if flag & 1 != 0 { Some(r.leb()?) } else { None };
    Ok((flag & 2 != 0, min, max))
}

/// Read the import and export sections of a module.
pub fn parse_module(wasm: &[u8]) -> Result<ModuleInfo, String> {
    if wasm.get(0..4) != Some(b"\0asm") {
        return Err("not a WebAssembly module".into());
    }
    if wasm.get(4..8) != Some(&[1, 0, 0, 0]) {
        return Err("unsupported WebAssembly version".into());
    }
    let mut r = Reader { b: wasm, i: 8 };
    let mut types: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    let mut info = ModuleInfo::default();
    while r.i < wasm.len() {
        let id = r.byte()?;
        let size = r.leb()? as usize;
        let body = r.take(size)?;
        let mut s = Reader { b: body, i: 0 };
        match id {
            1 => {
                for _ in 0..s.leb()? {
                    if s.byte()? != 0x60 {
                        return Err("a type is not a function type".into());
                    }
                    let mut params = Vec::new();
                    for _ in 0..s.leb()? {
                        params.push(val_type(s.byte()?)?);
                    }
                    let mut results = Vec::new();
                    for _ in 0..s.leb()? {
                        results.push(val_type(s.byte()?)?);
                    }
                    types.push((params, results));
                }
            }
            2 => {
                for _ in 0..s.leb()? {
                    let module = s.name()?;
                    let name = s.name()?;
                    let kind = match s.byte()? {
                        0 => {
                            let t = s.leb()? as usize;
                            let (params, results) =
                                types.get(t).cloned().ok_or("an import has an unknown type")?;
                            ImportKind::Func { params, results }
                        }
                        1 => {
                            s.byte()?;
                            limits(&mut s)?;
                            ImportKind::Table
                        }
                        2 => {
                            let (shared, min, max) = limits(&mut s)?;
                            ImportKind::Memory { shared, min, max }
                        }
                        3 => {
                            s.byte()?;
                            s.byte()?;
                            ImportKind::Global
                        }
                        4 => {
                            s.byte()?;
                            s.leb()?;
                            ImportKind::Tag
                        }
                        other => return Err(format!("unknown import kind {other}")),
                    };
                    info.imports.push(Import { module, name, kind });
                }
            }
            7 => {
                for _ in 0..s.leb()? {
                    let name = s.name()?;
                    let kind = s.byte()?;
                    s.leb()?;
                    info.exports.push((name, kind));
                }
            }
            _ => {}
        }
    }
    Ok(info)
}

/// The result of checking a module's imports against the documented function set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Problems: an undocumented function, a wrong signature, an import from another module, a documented function that is missing.
    pub problems: Vec<String>,
    /// Number of `bucket_v0` functions the module imports.
    pub bucket_functions: usize,
    /// The module imports a shared memory (a threads build).
    pub shared_memory: bool,
}

impl ImportReport {
    /// True when nothing is wrong.
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Check the imports of a module: every import is a function of module `bucket_v0` that the API documents with the signature
/// it has here (the one allowed exception is the shared memory `env.memory` of a threads build), and every documented function
/// is imported, so the module's link requirements are exactly the documented set.
pub fn check_imports(info: &ModuleInfo) -> ImportReport {
    let mut rep = ImportReport::default();
    let mut seen: Vec<&str> = Vec::new();
    for imp in &info.imports {
        match (&imp.kind, imp.module.as_str(), imp.name.as_str()) {
            (ImportKind::Memory { shared, .. }, "env", "memory") => rep.shared_memory = *shared,
            (ImportKind::Func { params, results }, m, name) if m == MODULE => {
                match FUNCTIONS.iter().find(|f| f.name == name) {
                    None => rep.problems.push(format!("`{MODULE}.{name}` is not a documented function")),
                    Some(f) => {
                        let want_results: Vec<&str> =
                            if f.result.is_empty() { Vec::new() } else { alloc::vec![f.result] };
                        let got_params: Vec<&str> = params.iter().map(String::as_str).collect();
                        let got_results: Vec<&str> = results.iter().map(String::as_str).collect();
                        if got_params != f.params || got_results != want_results {
                            rep.problems.push(format!(
                                "`{MODULE}.{name}` is imported as ({}) -> ({}), documented as ({}) -> ({})",
                                got_params.join(", "),
                                got_results.join(", "),
                                f.params.join(", "),
                                want_results.join(", ")
                            ));
                        }
                        if seen.contains(&name) {
                            rep.problems.push(format!("`{MODULE}.{name}` is imported twice"));
                        }
                        seen.push(name);
                        rep.bucket_functions += 1;
                    }
                }
            }
            _ => rep.problems.push(format!("unexpected import `{}.{}`", imp.module, imp.name)),
        }
    }
    for f in FUNCTIONS {
        if !seen.contains(&f.name) {
            rep.problems.push(format!("documented function `{MODULE}.{}` is not imported", f.name));
        }
    }
    rep
}
