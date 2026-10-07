//! Offline Windows registry hives (regf). Two backends behind one API:
//!
//! - elsewhere than Windows, hivex, linked dynamically (`hivex` module). It handles every regf layout
//!   XP and Server 2003 use (`lf`/`lh`/`li`/`ri` subkey lists, big data) and the slack Windows leaves
//!   after the last hbin; it changes the file in place and never reuses free cells, so a hive grows a
//!   little every time it is changed. A new key shares its parent's security descriptor.
//! - on Windows, Microsoft's Offline Registry Library, offreg.dll from System32, loaded when the
//!   first hive is opened (`offreg` module). It writes a new, compacted hive file in the format of
//!   the Windows version given by [`Hive::set_os_version`]. A new key gets the security descriptor
//!   Windows would give it: its parent's inheritable entries.
//!
//! Key and value names are case-insensitive, as in Windows. Value data is raw bytes; [`Value`] has
//! helpers for the usual types.

pub mod reg;

#[cfg(not(windows))]
mod hivex;
#[cfg(not(windows))]
pub use hivex::Hive;
#[cfg(windows)]
mod offreg;
#[cfg(windows)]
pub use offreg::Hive;

use std::fmt;

pub const REG_NONE: u32 = 0;
pub const REG_SZ: u32 = 1;
pub const REG_EXPAND_SZ: u32 = 2;
pub const REG_BINARY: u32 = 3;
pub const REG_DWORD: u32 = 4;
pub const REG_MULTI_SZ: u32 = 7;
pub const REG_QWORD: u32 = 11;

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// A key in an open hive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node(usize);

/// A value: name ("" for the default value), type and raw data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    pub name: String,
    pub ty: u32,
    pub data: Vec<u8>,
}

impl Value {
    pub fn new(name: &str, ty: u32, data: Vec<u8>) -> Value {
        Value {
            name: name.to_string(),
            ty,
            data,
        }
    }

    /// REG_SZ (or REG_EXPAND_SZ with `ty`): UTF-16LE with a terminating NUL.
    pub fn string(name: &str, ty: u32, s: &str) -> Value {
        Value::new(name, ty, utf16z(s))
    }

    pub fn dword(name: &str, v: u32) -> Value {
        Value::new(name, REG_DWORD, v.to_le_bytes().to_vec())
    }

    pub fn multi_string(name: &str, items: &[&str]) -> Value {
        let mut data: Vec<u8> = items.iter().flat_map(|s| utf16z(s)).collect();
        data.extend_from_slice(&[0, 0]);
        Value::new(name, REG_MULTI_SZ, data)
    }

    /// The data as a DWORD, if it is 4 bytes of REG_DWORD.
    pub fn as_dword(&self) -> Option<u32> {
        (self.ty == REG_DWORD && self.data.len() == 4)
            .then(|| u32::from_le_bytes(self.data[..4].try_into().unwrap()))
    }

    /// The data as UTF-16LE strings split at NULs (trailing empty ones dropped).
    pub fn as_strings(&self) -> Vec<String> {
        let w: Vec<u16> = self
            .data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let mut out: Vec<String> = w.split(|&c| c == 0).map(String::from_utf16_lossy).collect();
        while out.last().is_some_and(|s| s.is_empty()) {
            out.pop();
        }
        out
    }
}

pub fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// The parts of the API that both backends share.
impl Hive {
    /// The key at a backslash-separated path below `from` (empty path: `from` itself).
    pub fn find(&self, from: Node, path: &str) -> Result<Option<Node>> {
        let mut node = from;
        for part in path.split('\\').filter(|p| !p.is_empty()) {
            match self.child(node, part)? {
                Some(c) => node = c,
                None => return Ok(None),
            }
        }
        Ok(Some(node))
    }

    /// The key at `path` below `from`, creating the missing keys.
    pub fn create(&mut self, from: Node, path: &str) -> Result<Node> {
        let mut node = from;
        for part in path.split('\\').filter(|p| !p.is_empty()) {
            node = match self.child(node, part)? {
                Some(c) => c,
                None => self.add_child(node, part)?,
            };
        }
        Ok(node)
    }

    /// The value `name` of a key, compared case-insensitively.
    pub fn value(&self, node: Node, name: &str) -> Result<Option<Value>> {
        Ok(self
            .values(node)?
            .into_iter()
            .find(|v| v.name.eq_ignore_ascii_case(name)))
    }

    /// The current control set of a SYSTEM hive (`Select\Current`), e.g. "ControlSet001".
    pub fn current_control_set(&self) -> Result<String> {
        let select = self
            .find(self.root(), "Select")?
            .ok_or_else(|| Error("no Select key: not a SYSTEM hive".into()))?;
        let n = self
            .value(select, "Current")?
            .and_then(|v| v.as_dword())
            .ok_or_else(|| Error("Select\\Current is not a DWORD".into()))?;
        let cs = format!("ControlSet{n:03}");
        if self.find(self.root(), &cs)?.is_none() {
            return Err(Error(format!(
                "Select\\Current is {n}, but there is no {cs}"
            )));
        }
        Ok(cs)
    }

    /// Adds `item` to the REG_MULTI_SZ value `name` of `node` (created if missing) unless it is
    /// there already (compared case-insensitively); returns whether it was added.
    pub fn append_multi_sz(&mut self, node: Node, name: &str, item: &str) -> Result<bool> {
        let mut items = match self.value(node, name)? {
            Some(v) if v.ty == REG_MULTI_SZ => v.as_strings(),
            Some(v) => {
                return Err(Error(format!(
                    "{name}: type {} where REG_MULTI_SZ was expected",
                    v.ty
                )));
            }
            None => Vec::new(),
        };
        items.retain(|s| !s.is_empty());
        if items.iter().any(|s| s.eq_ignore_ascii_case(item)) {
            return Ok(false);
        }
        items.push(item.to_string());
        let refs: Vec<&str> = items.iter().map(String::as_str).collect();
        self.set(node, &Value::multi_string(name, &refs))?;
        Ok(true)
    }
}

/// The regf base block of a hive file, for checking it is clean before and after a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub sequence: (u32, u32),
    pub version: (u32, u32),
    /// Length of the hbins, from the base block.
    pub hbins: u32,
}

impl Header {
    pub fn read(b: &[u8]) -> Result<Header> {
        if b.len() < 0x200 || &b[..4] != b"regf" {
            return Err(Error("not a registry hive (no 'regf' signature)".into()));
        }
        let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        Ok(Header {
            sequence: (u(4), u(8)),
            version: (u(0x14), u(0x18)),
            hbins: u(0x28),
        })
    }

    /// A hive whose sequence numbers differ was not written completely; its log would be replayed.
    pub fn dirty(&self) -> bool {
        self.sequence.0 != self.sequence.1
    }
}
