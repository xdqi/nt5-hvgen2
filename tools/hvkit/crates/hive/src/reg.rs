//! `.reg` files ("Windows Registry Editor Version 5.00"): export in the exact format of
//! `reg.exe export`, and import with the semantics of `reg.exe import`.
//!
//! A hive file has no name for its root key; a .reg file names every key by its full path. The root
//! given here (e.g. `HKEY_LOCAL_MACHINE\XPMIG`) stands for the hive's root key in both directions.

use crate::{
    Error, Hive, Node, REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_SZ, Result, Value,
    utf16z,
};

const HEADER: &str = "Windows Registry Editor Version 5.00";

/// Exports `from` and everything below it, named as `root` (the path `from` has in the export), as
/// text with CRLF line ends. [`to_utf16`] makes the file `reg.exe export` writes.
pub fn export(h: &Hive, from: Node, root: &str) -> Result<String> {
    let mut out = format!("{HEADER}\r\n\r\n");
    export_key(h, from, root, &mut out)?;
    Ok(out)
}

fn export_key(h: &Hive, node: Node, path: &str, out: &mut String) -> Result<()> {
    out.push('[');
    out.push_str(path);
    out.push_str("]\r\n");
    for v in h.values(node)? {
        out.push_str(&value_line(&v));
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    for c in h.children(node)? {
        let name = h.name(c)?;
        export_key(h, c, &format!("{path}\\{name}"), out)?;
    }
    Ok(())
}

fn quote(s: &str) -> String {
    let mut q = String::with_capacity(s.len() + 2);
    q.push('"');
    for c in s.chars() {
        if c == '\\' || c == '"' {
            q.push('\\');
        }
        q.push(c);
    }
    q.push('"');
    q
}

/// A REG_SZ as reg.exe writes it: the UTF-16 text up to the first NUL, whether the data is
/// terminated or not (XP's hives have both, and strings with padding after the NUL). Data of odd
/// length (never seen) is written as hex(1).
fn as_plain_string(data: &[u8]) -> Option<String> {
    if !data.len().is_multiple_of(2) {
        return None;
    }
    let w: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    Some(String::from_utf16_lossy(&w[..end]))
}

/// One value as reg.exe writes it (continuation lines joined with "\\\r\n  "). reg.exe writes
/// in text mode, so a LF inside a name or string comes out as CR LF (a CR LF as CR CR LF).
pub fn value_line(v: &Value) -> String {
    let text = |s: &str| quote(s).replace('\n', "\r\n");
    let name = if v.name.is_empty() {
        "@".to_string()
    } else {
        text(&v.name)
    };
    if v.ty == REG_SZ
        && let Some(s) = as_plain_string(&v.data)
    {
        return format!("{name}={}", text(&s));
    }
    if let Some(d) = v.as_dword() {
        return format!("{name}=dword:{d:08x}");
    }
    let mut out = if v.ty == REG_BINARY {
        format!("{name}=hex:")
    } else {
        format!("{name}=hex({:x}):", v.ty)
    };
    // Lines stay within 79 columns plus the backslash; continuation lines are indented by two.
    let mut col = out.chars().count();
    for (i, b) in v.data.iter().enumerate() {
        if col + 3 > 79 {
            out.push_str("\\\r\n  ");
            col = 2;
        }
        out.push_str(&format!("{b:02x}"));
        if i + 1 < v.data.len() {
            out.push(',');
        }
        col += 3;
    }
    out
}

/// The text as reg.exe writes it: UTF-16LE with a byte order mark.
pub fn to_utf16(text: &str) -> Vec<u8> {
    [0xff, 0xfe]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect()
}

/// The text of a .reg file: UTF-16LE with a byte order mark, or UTF-8 (with or without one).
pub fn decode(bytes: &[u8]) -> Result<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        if rest.len() % 2 != 0 {
            return Err(Error("odd length for a UTF-16 file".into()));
        }
        let w: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&w).map_err(|_| Error("invalid UTF-16".into()));
    }
    let b = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    String::from_utf8(b.to_vec()).map_err(|_| Error("neither UTF-16LE with a BOM nor UTF-8".into()))
}

/// What an import changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportStats {
    pub keys: usize,
    pub keys_deleted: usize,
    pub values_set: usize,
    pub values_deleted: usize,
}

enum Data {
    Delete,
    Set(u32, Vec<u8>),
}

/// Applies a .reg file to the hive. `root` is the path that stands for the hive's root; without it,
/// the first two components of the first key (e.g. `HKEY_LOCAL_MACHINE\XPMIG`).
pub fn import(h: &mut Hive, text: &str, root: Option<&str>) -> Result<ImportStats> {
    let lines = logical_lines(text);
    let mut it = lines
        .iter()
        .map(|(n, l)| (*n, l.as_str()))
        .filter(|(_, l)| !l.trim().is_empty());
    let regedit4 = match it.next() {
        Some((_, l)) if l.trim() == HEADER => false,
        Some((_, l)) if l.trim() == "REGEDIT4" => true,
        _ => {
            return Err(Error(format!(
                "not a .reg file: the first line is not \"{HEADER}\" or \"REGEDIT4\""
            )));
        }
    };
    let mut root = root.map(|r| r.trim_end_matches('\\').to_string());
    let mut stats = ImportStats::default();
    let mut current: Option<Node> = None;
    for (n, line) in it {
        let line = line.trim();
        let at = |e: String| Error(format!("line {n}: {e}"));
        if line.starts_with(';') {
            continue;
        }
        if let Some(key) = line.strip_prefix('[') {
            let key = key
                .strip_suffix(']')
                .ok_or_else(|| at("key without ']'".into()))?;
            let (delete, key) = match key.strip_prefix('-') {
                Some(k) => (true, k),
                None => (false, key),
            };
            let root = root
                .get_or_insert_with(|| key.splitn(3, '\\').take(2).collect::<Vec<_>>().join("\\"));
            let sub =
                relative(key, root).ok_or_else(|| at(format!("key {key} is not below {root}")))?;
            if delete {
                current = None;
                if let Some(k) = h.find(h.root(), sub)? {
                    if k == h.root() {
                        return Err(at("refusing to delete the root key".into()));
                    }
                    h.delete(k).map_err(|e| at(e.0))?;
                    stats.keys_deleted += 1;
                }
            } else {
                current = Some(h.create(h.root(), sub).map_err(|e| at(e.0))?);
                stats.keys += 1;
            }
            continue;
        }
        let Some(node) = current else {
            return Err(at(format!(
                "value outside a key (or in a deleted one): {line}"
            )));
        };
        let (name, data) = parse_value(line, regedit4).map_err(|e| at(e.0))?;
        match data {
            Data::Delete => {
                if h.delete_value(node, &name).map_err(|e| at(e.0))? {
                    stats.values_deleted += 1;
                }
            }
            Data::Set(ty, data) => {
                h.set(node, &Value { name, ty, data })
                    .map_err(|e| at(e.0))?;
                stats.values_set += 1;
            }
        }
    }
    Ok(stats)
}

/// `key` relative to `root` (case-insensitive), or None if it is not below it.
fn relative<'a>(key: &'a str, root: &str) -> Option<&'a str> {
    if key.len() < root.len()
        || !key.is_char_boundary(root.len())
        || !key[..root.len()].eq_ignore_ascii_case(root)
    {
        return None;
    }
    match &key[root.len()..] {
        "" => Some(""),
        rest => rest.strip_prefix('\\'),
    }
}

/// Lines with their numbers, hex continuations ("\\" at the end) joined.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut cont = false;
    for (i, raw) in text.lines().enumerate() {
        let l = raw.trim_end_matches('\r');
        let piece = if cont { l.trim_start() } else { l };
        let (piece, next_cont) = match piece.trim_end().strip_suffix('\\') {
            Some(p) if is_hex_value(if cont { &out.last().unwrap().1 } else { piece }) => (p, true),
            _ => (piece, false),
        };
        if cont {
            out.last_mut().unwrap().1.push_str(piece);
        } else {
            out.push((i + 1, piece.to_string()));
        }
        cont = next_cont;
    }
    out
}

/// Whether a line is a value whose data is hex (the only kind that continues on the next line).
fn is_hex_value(line: &str) -> bool {
    split_name(line).is_some_and(|(_, rest)| rest.starts_with("hex"))
}

/// Splits `"name"=data` or `@=data` into the name and the data text.
fn split_name(line: &str) -> Option<(String, &str)> {
    if let Some(rest) = line.strip_prefix("@=") {
        return Some((String::new(), rest));
    }
    let mut chars = line.strip_prefix('"')?.char_indices();
    let mut name = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => name.push(chars.next()?.1),
            '"' => return Some((name, line[1 + i + 1..].strip_prefix('=')?)),
            c => name.push(c),
        }
    }
    None
}

fn parse_value(line: &str, regedit4: bool) -> Result<(String, Data)> {
    let (name, data) = split_name(line).ok_or_else(|| Error(format!("not a value: {line}")))?;
    let data = data.trim();
    if data == "-" {
        return Ok((name, Data::Delete));
    }
    if let Some(s) = data.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = s.chars();
        loop {
            match chars.next() {
                Some('\\') => out.push(
                    chars
                        .next()
                        .ok_or_else(|| Error("string ends in a backslash".into()))?,
                ),
                Some('"') => break,
                Some(c) => out.push(c),
                None => return Err(Error("unterminated string".into())),
            }
        }
        return Ok((name, Data::Set(REG_SZ, utf16z(&out))));
    }
    if let Some(d) = data.strip_prefix("dword:") {
        let v = u32::from_str_radix(d.trim(), 16).map_err(|_| Error(format!("bad dword: {d}")))?;
        return Ok((name, Data::Set(REG_DWORD, v.to_le_bytes().to_vec())));
    }
    let (ty, bytes) = if let Some(b) = data.strip_prefix("hex:") {
        (REG_BINARY, b)
    } else if let Some(rest) = data.strip_prefix("hex(") {
        let (t, b) = rest
            .split_once("):")
            .ok_or_else(|| Error(format!("bad hex type: {data}")))?;
        (
            u32::from_str_radix(t, 16).map_err(|_| Error(format!("bad hex type: {t}")))?,
            b,
        )
    } else {
        return Err(Error(format!("unknown value data: {data}")));
    };
    let mut raw = Vec::new();
    for x in bytes.split(',').map(str::trim).filter(|x| !x.is_empty()) {
        raw.push(u8::from_str_radix(x, 16).map_err(|_| Error(format!("bad hex byte: {x}")))?);
    }
    // REGEDIT4 writes REG_EXPAND_SZ and REG_MULTI_SZ in the ANSI code page; the hive holds UTF-16.
    if regedit4 && (ty == REG_EXPAND_SZ || ty == REG_MULTI_SZ) {
        raw = raw.iter().flat_map(|&c| [c, 0]).collect();
    }
    Ok((name, Data::Set(ty, raw)))
}

/// A value for people: strings decoded, DWORDs in hex and decimal, binary data in hex.
pub fn describe(v: &Value) -> String {
    let name = if v.name.is_empty() {
        "@".to_string()
    } else {
        quote(&v.name)
    };
    let shown = match v.ty {
        REG_SZ | REG_EXPAND_SZ => {
            let s = v.as_strings().into_iter().next().unwrap_or_default();
            format!(
                "{}{}",
                if v.ty == REG_EXPAND_SZ { "EXPAND " } else { "" },
                quote(&s)
            )
        }
        REG_MULTI_SZ => format!(
            "MULTI [{}]",
            v.as_strings()
                .iter()
                .map(|s| quote(s))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => match v.as_dword() {
            Some(d) => format!("dword:{d:08x} ({d})"),
            None => value_line(v)
                .split_once('=')
                .map(|(_, d)| d.replace("\\\r\n  ", ""))
                .unwrap_or_default(),
        },
    };
    format!("{name} = {shown}")
}
