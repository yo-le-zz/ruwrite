//! Extraction of functions, variables and line numbers from DWARF.
//!
//! Only what ruwrite needs is kept, in plain owned structures, so the parsed
//! result outlives the borrowed DWARF sections.

use std::borrow::Cow;

use gimli::{AttributeValue, EndianSlice, Operation, RunTimeEndian, UnitOffset};
use object::{Object, ObjectSection};

use crate::error::{Error, Result};
use crate::value::{Class, ScalarType};

type Slice<'a> = EndianSlice<'a, RunTimeEndian>;
type Dwarf<'a> = gimli::Dwarf<Slice<'a>>;
type Unit<'a> = gimli::Unit<Slice<'a>>;
type Entry<'a> = gimli::DebuggingInformationEntry<Slice<'a>>;

/// Where the compiler expects the "frame base" of a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameBase {
    /// A plain register, by DWARF number (7 = `rsp`, 6 = `rbp` on x86_64).
    Register(u16),
    /// The canonical frame address (value of the stack pointer before the call).
    Cfa,
    /// Anything else.
    Unknown,
}

/// A local variable stored in the stack frame.
#[derive(Debug, Clone)]
pub struct DwLocal {
    pub name: String,
    pub ty: ScalarType,
    /// Offset from the frame base (`DW_OP_fbreg`).
    pub frame_offset: i64,
    pub decl_line: Option<u32>,
    /// Source file of the declaration, as recorded by the compiler.
    pub decl_file: Option<String>,
}

/// A function described by DWARF.
#[derive(Debug, Clone)]
pub struct DwFunction {
    /// Fully qualified name (`crate::module::function`).
    pub name: String,
    pub frame_base: FrameBase,
    /// Address ranges of the machine code (usually exactly one).
    pub ranges: Vec<(u64, u64)>,
    pub locals: Vec<DwLocal>,
}

/// A variable with a fixed address (`static`, `static mut`).
#[derive(Debug, Clone)]
pub struct DwStatic {
    /// Fully qualified name (`crate::module::NAME`).
    pub name: String,
    pub address: u64,
    pub ty: ScalarType,
}

#[derive(Debug, Clone, Copy)]
struct LineRow {
    address: u64,
    line: u32,
    end_sequence: bool,
}

/// Everything extracted from the DWARF sections.
#[derive(Debug, Default)]
pub struct DebugInfo {
    pub functions: Vec<DwFunction>,
    pub statics: Vec<DwStatic>,
    lines: Vec<LineRow>,
}

impl DebugInfo {
    /// Source line of the instruction at `address`, if the line table covers it.
    pub fn line_at(&self, address: u64) -> Option<u32> {
        let idx = self.lines.partition_point(|row| row.address <= address);
        let row = self.lines.get(idx.checked_sub(1)?)?;
        (!row.end_sequence && row.line != 0).then_some(row.line)
    }
}

/// Parses the DWARF sections of `file`.
pub fn load(file: &object::File<'_>) -> Result<DebugInfo> {
    let endian = if file.is_little_endian() {
        RunTimeEndian::Little
    } else {
        RunTimeEndian::Big
    };

    let sections =
        gimli::DwarfSections::load(|id| -> std::result::Result<Cow<[u8]>, object::Error> {
            match file.section_by_name(id.name()) {
                Some(section) => section.uncompressed_data(),
                None => Ok(Cow::Borrowed(&[][..])),
            }
        })
        .map_err(|e| Error::NoDebugInfo(e.to_string()))?;
    let dwarf: Dwarf<'_> = sections.borrow(|data| EndianSlice::new(data, endian));

    let mut info = DebugInfo::default();
    let mut headers = dwarf.units();
    while let Some(header) = headers.next().map_err(dwarf_error)? {
        let unit = dwarf.unit(header).map_err(dwarf_error)?;
        read_unit(&dwarf, &unit, &mut info).map_err(dwarf_error)?;
    }

    if info.functions.is_empty() && info.statics.is_empty() {
        return Err(Error::NoDebugInfo(
            "the executable has no DWARF debug information (was it stripped?)".into(),
        ));
    }

    // Les lignes d'une même adresse : la fin de séquence passe avant le début
    // de la suivante, afin que la recherche retombe sur le début.
    info.lines
        .sort_by_key(|row| (row.address, !row.end_sequence));
    info.statics
        .sort_by(|a, b| (a.address, &a.name).cmp(&(b.address, &b.name)));
    info.statics
        .dedup_by(|a, b| a.address == b.address && a.name == b.name);
    Ok(info)
}

fn dwarf_error(e: gimli::Error) -> Error {
    Error::NoDebugInfo(e.to_string())
}

/// Position dans l'arbre des DIE : sert à reconstruire les noms qualifiés.
enum Scope {
    Namespace(String),
    Function(Option<usize>, String),
    Other,
}

struct Frame {
    depth: isize,
    scope: Scope,
}

fn read_unit<'a>(dwarf: &Dwarf<'a>, unit: &Unit<'a>, info: &mut DebugInfo) -> gimli::Result<()> {
    read_lines(unit, info)?;

    let mut stack: Vec<Frame> = Vec::new();
    let mut cursor = unit.entries();
    while let Some(entry) = cursor.next_dfs()? {
        let depth = entry.depth();
        while stack.last().is_some_and(|f| f.depth >= depth) {
            stack.pop();
        }

        let scope = match entry.tag() {
            gimli::DW_TAG_namespace => Scope::Namespace(
                string_attr(dwarf, unit, entry, gimli::DW_AT_name)?.unwrap_or_default(),
            ),
            gimli::DW_TAG_subprogram => read_function(dwarf, unit, entry, &stack, info)?,
            gimli::DW_TAG_variable => {
                read_variable(dwarf, unit, entry, &stack, info)?;
                Scope::Other
            }
            _ => Scope::Other,
        };

        // On ne mémorise que les nœuds qui ont des enfants à qualifier.
        if entry.has_children() {
            stack.push(Frame { depth, scope });
        }
    }
    Ok(())
}

fn read_lines<'a>(unit: &Unit<'a>, info: &mut DebugInfo) -> gimli::Result<()> {
    let Some(program) = unit.line_program.clone() else {
        return Ok(());
    };
    let mut rows = program.rows();
    while let Some((_, row)) = rows.next_row()? {
        info.lines.push(LineRow {
            address: row.address(),
            line: row
                .line()
                .map_or(0, |l| u32::try_from(l.get()).unwrap_or(0)),
            end_sequence: row.end_sequence(),
        });
    }
    Ok(())
}

/// Nom qualifié construit depuis les espaces de noms et fonctions englobants.
fn qualified(stack: &[Frame], name: &str) -> String {
    let mut parts: Vec<&str> = stack
        .iter()
        .filter_map(|f| match &f.scope {
            Scope::Namespace(n) | Scope::Function(_, n) if !n.is_empty() => Some(n.as_str()),
            _ => None,
        })
        .collect();
    parts.push(name);
    parts.join("::")
}

fn read_function<'a>(
    dwarf: &Dwarf<'a>,
    unit: &Unit<'a>,
    entry: &Entry<'a>,
    stack: &[Frame],
    info: &mut DebugInfo,
) -> gimli::Result<Scope> {
    let name = string_attr(dwarf, unit, entry, gimli::DW_AT_name)?.unwrap_or_default();

    let mut ranges = Vec::new();
    let mut iter = dwarf.die_ranges(unit, entry)?;
    while let Some(range) = iter.next()? {
        if range.begin < range.end {
            ranges.push((range.begin, range.end));
        }
    }
    // Sans code (déclaration, fonction éliminée) : rien à enregistrer.
    if ranges.is_empty() || name.is_empty() {
        return Ok(Scope::Function(None, name));
    }

    info.functions.push(DwFunction {
        name: qualified(stack, &name),
        frame_base: frame_base(unit, entry)?,
        ranges,
        locals: Vec::new(),
    });
    Ok(Scope::Function(Some(info.functions.len() - 1), name))
}

fn frame_base(unit: &Unit<'_>, entry: &Entry<'_>) -> gimli::Result<FrameBase> {
    let Some(AttributeValue::Exprloc(expr)) = entry.attr_value(gimli::DW_AT_frame_base) else {
        return Ok(FrameBase::Unknown);
    };
    let mut ops = expr.operations(unit.encoding());
    Ok(match ops.next()? {
        Some(Operation::Register { register }) => FrameBase::Register(register.0),
        Some(Operation::CallFrameCFA) => FrameBase::Cfa,
        _ => FrameBase::Unknown,
    })
}

fn read_variable<'a>(
    dwarf: &Dwarf<'a>,
    unit: &Unit<'a>,
    entry: &Entry<'a>,
    stack: &[Frame],
    info: &mut DebugInfo,
) -> gimli::Result<()> {
    let Some(name) = string_attr(dwarf, unit, entry, gimli::DW_AT_name)? else {
        return Ok(());
    };
    let Some(AttributeValue::Exprloc(expr)) = entry.attr_value(gimli::DW_AT_location) else {
        return Ok(());
    };
    let mut ops = expr.operations(unit.encoding());
    let first = ops.next()?;
    if ops.next()?.is_some() {
        return Ok(()); // expression composite (registres, pièces...) : ignorée
    }

    match first {
        Some(Operation::Address { address }) => {
            let ty = describe_type(dwarf, unit, entry)?;
            info.statics.push(DwStatic {
                name: qualified(stack, &name),
                address,
                ty,
            });
        }
        Some(Operation::FrameOffset { offset }) => {
            // Variable locale : rattachée à la fonction englobante la plus proche.
            let owner = stack.iter().rev().find_map(|f| match f.scope {
                Scope::Function(idx, _) => Some(idx),
                _ => None,
            });
            if let Some(Some(idx)) = owner {
                let ty = describe_type(dwarf, unit, entry)?;
                let decl_line = entry
                    .attr_value(gimli::DW_AT_decl_line)
                    .and_then(|v| v.udata_value())
                    .and_then(|v| u32::try_from(v).ok());
                let decl_file = declaration_file(dwarf, unit, entry)?;
                info.functions[idx].locals.push(DwLocal {
                    name,
                    ty,
                    frame_offset: offset,
                    decl_line,
                    decl_file,
                });
            }
        }
        _ => {}
    }
    Ok(())
}

fn string_attr<'a>(
    dwarf: &Dwarf<'a>,
    unit: &Unit<'a>,
    entry: &Entry<'a>,
    name: gimli::DwAt,
) -> gimli::Result<Option<String>> {
    match entry.attr_value(name) {
        Some(value) => Ok(Some(
            dwarf
                .attr_string(unit, value)?
                .to_string_lossy()
                .into_owned(),
        )),
        None => Ok(None),
    }
}

/// Type d'une variable, réduit à ce qui sert à lire/écrire sa valeur.
fn describe_type<'a>(
    dwarf: &Dwarf<'a>,
    unit: &Unit<'a>,
    entry: &Entry<'a>,
) -> gimli::Result<ScalarType> {
    match entry.attr_value(gimli::DW_AT_type) {
        Some(AttributeValue::UnitRef(offset)) => type_at(dwarf, unit, offset, 0),
        _ => Ok(unknown_type()),
    }
}

fn unknown_type() -> ScalarType {
    ScalarType {
        name: "?".into(),
        class: Class::Other,
        size: 0,
    }
}

fn type_at<'a>(
    dwarf: &Dwarf<'a>,
    unit: &Unit<'a>,
    offset: UnitOffset<usize>,
    hops: u8,
) -> gimli::Result<ScalarType> {
    // Garde-fou contre les chaînes de typedef cycliques.
    if hops > 16 {
        return Ok(unknown_type());
    }
    let entry = unit.entry(offset)?;
    let name = string_attr(dwarf, unit, &entry, gimli::DW_AT_name)?;
    let byte_size = entry
        .attr_value(gimli::DW_AT_byte_size)
        .and_then(|v| v.udata_value())
        .unwrap_or(0) as usize;

    match entry.tag() {
        gimli::DW_TAG_typedef
        | gimli::DW_TAG_const_type
        | gimli::DW_TAG_volatile_type
        | gimli::DW_TAG_restrict_type => match entry.attr_value(gimli::DW_AT_type) {
            Some(AttributeValue::UnitRef(next)) => type_at(dwarf, unit, next, hops + 1),
            _ => Ok(unknown_type()),
        },
        gimli::DW_TAG_base_type => {
            let class = match entry.attr_value(gimli::DW_AT_encoding) {
                Some(AttributeValue::Encoding(enc)) => match enc {
                    gimli::DW_ATE_boolean => Class::Bool,
                    gimli::DW_ATE_signed | gimli::DW_ATE_signed_char => Class::Signed,
                    gimli::DW_ATE_unsigned | gimli::DW_ATE_unsigned_char | gimli::DW_ATE_UTF => {
                        Class::Unsigned
                    }
                    gimli::DW_ATE_float => Class::Float,
                    _ => Class::Other,
                },
                _ => Class::Other,
            };
            Ok(ScalarType {
                name: name.unwrap_or_else(|| "?".into()),
                class,
                size: byte_size,
            })
        }
        gimli::DW_TAG_array_type => array_type(dwarf, unit, &entry, hops),
        _ => Ok(ScalarType {
            name: name.unwrap_or_else(|| "?".into()),
            class: Class::Other,
            size: byte_size,
        }),
    }
}

/// `[T; N]` : taille = taille de T × N (une seule dimension, comme rustc).
fn array_type<'a>(
    dwarf: &Dwarf<'a>,
    unit: &Unit<'a>,
    entry: &Entry<'a>,
    hops: u8,
) -> gimli::Result<ScalarType> {
    let element = match entry.attr_value(gimli::DW_AT_type) {
        Some(AttributeValue::UnitRef(off)) => type_at(dwarf, unit, off, hops + 1)?,
        _ => return Ok(unknown_type()),
    };

    let mut count = None;
    let mut cursor = unit.entries_at_offset(entry.offset())?;
    cursor.next_dfs()?; // l'entrée du tableau elle-même
    while let Some(child) = cursor.next_dfs()? {
        if child.depth() <= 0 {
            break;
        }
        if child.tag() == gimli::DW_TAG_subrange_type {
            count = child
                .attr_value(gimli::DW_AT_count)
                .and_then(|v| v.udata_value())
                .or_else(|| {
                    child
                        .attr_value(gimli::DW_AT_upper_bound)
                        .and_then(|v| v.udata_value())
                        .map(|upper| upper + 1)
                });
            break;
        }
    }

    let count = count.unwrap_or(0) as usize;
    Ok(ScalarType {
        name: format!("[{}; {count}]", element.name),
        class: Class::Other,
        size: element.size * count,
    })
}

/// Chemin du fichier source où la variable est déclarée (`DW_AT_decl_file`).
fn declaration_file<'a>(
    dwarf: &Dwarf<'a>,
    unit: &Unit<'a>,
    entry: &Entry<'a>,
) -> gimli::Result<Option<String>> {
    let Some(AttributeValue::FileIndex(index)) = entry.attr_value(gimli::DW_AT_decl_file) else {
        return Ok(None);
    };
    let Some(program) = unit.line_program.as_ref() else {
        return Ok(None);
    };
    let header = program.header();
    let Some(file) = header.file(index) else {
        return Ok(None);
    };
    let name = dwarf
        .attr_string(unit, file.path_name())?
        .to_string_lossy()
        .into_owned();
    if name.starts_with('/') {
        return Ok(Some(name));
    }
    let mut path = String::new();
    if let Some(dir) = file.directory(header) {
        path.push_str(&dwarf.attr_string(unit, dir)?.to_string_lossy());
    }
    if !path.starts_with('/') {
        if let Some(comp_dir) = &unit.comp_dir {
            let base = comp_dir.to_string_lossy().into_owned();
            path = if path.is_empty() {
                base
            } else {
                format!("{base}/{path}")
            };
        }
    }
    if !path.is_empty() && !path.ends_with('/') {
        path.push('/');
    }
    path.push_str(&name);
    Ok(Some(path))
}
