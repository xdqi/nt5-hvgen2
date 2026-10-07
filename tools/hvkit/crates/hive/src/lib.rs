//! Offline Windows registry hives (regf), read and written through hivex, which is linked
//! dynamically. hivex handles every regf layout XP and Server 2003 use (`lf`/`lh`/`li`/`ri` subkey
//! lists, big data) and the slack Windows leaves after the last hbin; it never reuses free cells, so
//! a hive grows a little every time it is changed.
//!
//! Key and value names are case-insensitive, as in Windows. Value data is raw bytes; [`Value`] has
//! helpers for the usual types.

pub mod reg;

use std::ffi::{CStr, CString, c_char, c_int};
use std::fmt;
use std::path::Path;

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

fn os_error(what: &str) -> Error {
    Error(format!("{what}: {}", std::io::Error::last_os_error()))
}

mod ffi {
    use std::ffi::{c_char, c_int, c_void};

    #[repr(C)]
    pub struct HiveH(c_void);

    #[repr(C)]
    pub struct SetValue {
        pub key: *mut c_char,
        pub t: c_int,
        pub len: usize,
        pub value: *mut c_char,
    }

    pub const HIVEX_OPEN_WRITE: c_int = 4;

    unsafe extern "C" {
        pub fn hivex_open(filename: *const c_char, flags: c_int) -> *mut HiveH;
        pub fn hivex_close(h: *mut HiveH) -> c_int;
        pub fn hivex_root(h: *mut HiveH) -> usize;
        pub fn hivex_node_name(h: *mut HiveH, node: usize) -> *mut c_char;
        pub fn hivex_node_children(h: *mut HiveH, node: usize) -> *mut usize;
        pub fn hivex_node_get_child(h: *mut HiveH, node: usize, name: *const c_char) -> usize;
        pub fn hivex_node_parent(h: *mut HiveH, node: usize) -> usize;
        pub fn hivex_node_values(h: *mut HiveH, node: usize) -> *mut usize;
        pub fn hivex_value_key(h: *mut HiveH, val: usize) -> *mut c_char;
        pub fn hivex_value_value(
            h: *mut HiveH,
            val: usize,
            t: *mut c_int,
            len: *mut usize,
        ) -> *mut c_char;
        pub fn hivex_node_add_child(h: *mut HiveH, parent: usize, name: *const c_char) -> usize;
        pub fn hivex_node_delete_child(h: *mut HiveH, node: usize) -> c_int;
        pub fn hivex_node_set_value(
            h: *mut HiveH,
            node: usize,
            val: *const SetValue,
            flags: c_int,
        ) -> c_int;
        pub fn hivex_node_set_values(
            h: *mut HiveH,
            node: usize,
            nr: usize,
            vals: *const SetValue,
            flags: c_int,
        ) -> c_int;
        pub fn hivex_commit(h: *mut HiveH, filename: *const c_char, flags: c_int) -> c_int;
        pub fn free(p: *mut c_void);
    }
}

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

/// A hive file opened with hivex. Changes stay in memory until [`Hive::commit`].
pub struct Hive {
    h: *mut ffi::HiveH,
    writable: bool,
}

/// Takes ownership of a malloc'd C string from hivex.
unsafe fn take_string(p: *mut c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { ffi::free(p.cast()) };
    Some(s)
}

/// Takes ownership of a malloc'd, 0-terminated array of handles from hivex.
unsafe fn take_handles(p: *mut usize) -> Option<Vec<usize>> {
    if p.is_null() {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let v = unsafe { *p.add(i) };
        if v == 0 {
            break;
        }
        out.push(v);
        i += 1;
    }
    unsafe { ffi::free(p.cast()) };
    Some(out)
}

fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error(format!("name contains a NUL: {s:?}")))
}

impl Hive {
    pub fn open(path: &Path, writable: bool) -> Result<Hive> {
        let p = cstring(&path.to_string_lossy())?;
        let h = unsafe {
            ffi::hivex_open(p.as_ptr(), if writable { ffi::HIVEX_OPEN_WRITE } else { 0 })
        };
        if h.is_null() {
            return Err(os_error(&format!("{}: hivex_open", path.display())));
        }
        Ok(Hive { h, writable })
    }

    pub fn root(&self) -> Node {
        Node(unsafe { ffi::hivex_root(self.h) })
    }

    pub fn name(&self, node: Node) -> Result<String> {
        unsafe { take_string(ffi::hivex_node_name(self.h, node.0)) }
            .ok_or_else(|| os_error("hivex_node_name"))
    }

    pub fn children(&self, node: Node) -> Result<Vec<Node>> {
        let v = unsafe { take_handles(ffi::hivex_node_children(self.h, node.0)) }
            .ok_or_else(|| os_error("hivex_node_children"))?;
        Ok(v.into_iter().map(Node).collect())
    }

    pub fn parent(&self, node: Node) -> Option<Node> {
        match unsafe { ffi::hivex_node_parent(self.h, node.0) } {
            0 => None,
            p => Some(Node(p)),
        }
    }

    /// The subkey `name` of `node`, compared case-insensitively.
    pub fn child(&self, node: Node, name: &str) -> Result<Option<Node>> {
        let n = cstring(name)?;
        match unsafe { ffi::hivex_node_get_child(self.h, node.0, n.as_ptr()) } {
            0 => Ok(None),
            c => Ok(Some(Node(c))),
        }
    }

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
                None => {
                    let n = cstring(part)?;
                    match unsafe { ffi::hivex_node_add_child(self.h, node.0, n.as_ptr()) } {
                        0 => return Err(os_error(&format!("hivex_node_add_child {part}"))),
                        c => Node(c),
                    }
                }
            };
        }
        Ok(node)
    }

    /// Deletes a key and everything below it.
    pub fn delete(&mut self, node: Node) -> Result<()> {
        if unsafe { ffi::hivex_node_delete_child(self.h, node.0) } != 0 {
            return Err(os_error("hivex_node_delete_child"));
        }
        Ok(())
    }

    /// The values of a key, in the order they are stored.
    pub fn values(&self, node: Node) -> Result<Vec<Value>> {
        let handles = unsafe { take_handles(ffi::hivex_node_values(self.h, node.0)) }
            .ok_or_else(|| os_error("hivex_node_values"))?;
        let mut out = Vec::with_capacity(handles.len());
        for v in handles {
            let name = unsafe { take_string(ffi::hivex_value_key(self.h, v)) }
                .ok_or_else(|| os_error("hivex_value_key"))?;
            let (mut t, mut len): (c_int, usize) = (0, 0);
            let p = unsafe { ffi::hivex_value_value(self.h, v, &mut t, &mut len) };
            if p.is_null() {
                return Err(os_error(&format!("hivex_value_value {name:?}")));
            }
            let data = unsafe { std::slice::from_raw_parts(p.cast::<u8>(), len) }.to_vec();
            unsafe { ffi::free(p.cast()) };
            out.push(Value {
                name,
                ty: t as u32,
                data,
            });
        }
        Ok(out)
    }

    /// The value `name` of a key, compared case-insensitively.
    pub fn value(&self, node: Node, name: &str) -> Result<Option<Value>> {
        Ok(self
            .values(node)?
            .into_iter()
            .find(|v| v.name.eq_ignore_ascii_case(name)))
    }

    /// Creates or replaces one value.
    pub fn set(&mut self, node: Node, v: &Value) -> Result<()> {
        let key = cstring(&v.name)?;
        let mut data = v.data.clone();
        let sv = ffi::SetValue {
            key: key.as_ptr() as *mut c_char,
            t: v.ty as c_int,
            len: data.len(),
            value: data.as_mut_ptr().cast(),
        };
        if unsafe { ffi::hivex_node_set_value(self.h, node.0, &sv, 0) } != 0 {
            return Err(os_error(&format!("hivex_node_set_value {:?}", v.name)));
        }
        Ok(())
    }

    /// Deletes one value; returns whether it existed.
    pub fn delete_value(&mut self, node: Node, name: &str) -> Result<bool> {
        let values = self.values(node)?;
        if !values.iter().any(|v| v.name.eq_ignore_ascii_case(name)) {
            return Ok(false);
        }
        let mut keep: Vec<(CString, Vec<u8>, u32)> = Vec::new();
        for v in values
            .into_iter()
            .filter(|v| !v.name.eq_ignore_ascii_case(name))
        {
            keep.push((cstring(&v.name)?, v.data, v.ty));
        }
        let svs: Vec<ffi::SetValue> = keep
            .iter_mut()
            .map(|(k, d, t)| ffi::SetValue {
                key: k.as_ptr() as *mut c_char,
                t: *t as c_int,
                len: d.len(),
                value: d.as_mut_ptr().cast(),
            })
            .collect();
        if unsafe { ffi::hivex_node_set_values(self.h, node.0, svs.len(), svs.as_ptr(), 0) } != 0 {
            return Err(os_error(&format!(
                "hivex_node_set_values (deleting {name:?})"
            )));
        }
        Ok(true)
    }

    /// Writes the hive to `path`, or back to the file it was opened from.
    pub fn commit(&mut self, path: Option<&Path>) -> Result<()> {
        if !self.writable {
            return Err(Error("the hive was opened read-only".into()));
        }
        let p = path.map(|p| cstring(&p.to_string_lossy())).transpose()?;
        let ptr = p.as_ref().map_or(std::ptr::null(), |p| p.as_ptr());
        if unsafe { ffi::hivex_commit(self.h, ptr, 0) } != 0 {
            return Err(os_error("hivex_commit"));
        }
        Ok(())
    }
}

impl Drop for Hive {
    fn drop(&mut self) {
        unsafe { ffi::hivex_close(self.h) };
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
