//! The hivex backend (everywhere but Windows): hivex is linked dynamically and changes the hive file
//! in place.

use crate::{Error, Node, Result, Value};
use std::ffi::{CStr, CString, c_char, c_int};
use std::path::Path;

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

    /// The Windows version the hive is for. hivex keeps the file's format, so this changes nothing.
    pub fn set_os_version(&mut self, _major: u32, _minor: u32) {}

    pub fn root(&self) -> Node {
        Node(unsafe { ffi::hivex_root(self.h) })
    }

    /// The key's name as stored (for the root key, the name the hive gives it).
    pub fn name(&self, node: Node) -> Result<String> {
        unsafe { take_string(ffi::hivex_node_name(self.h, node.0)) }
            .ok_or_else(|| os_error("hivex_node_name"))
    }

    /// The subkeys, in the order of the hive's index (sorted by name).
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

    /// Adds the subkey `name`, which must not exist yet; it shares the parent's security
    /// descriptor.
    pub(crate) fn add_child(&mut self, node: Node, name: &str) -> Result<Node> {
        let n = cstring(name)?;
        match unsafe { ffi::hivex_node_add_child(self.h, node.0, n.as_ptr()) } {
            0 => Err(os_error(&format!("hivex_node_add_child {name}"))),
            c => Ok(Node(c)),
        }
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

    /// Creates or replaces one value (a replaced value keeps its place and its name's case, as in
    /// Windows).
    pub fn set(&mut self, node: Node, v: &Value) -> Result<()> {
        let stored = self
            .values(node)?
            .into_iter()
            .find(|o| o.name.eq_ignore_ascii_case(&v.name))
            .map_or_else(|| v.name.clone(), |o| o.name);
        let key = cstring(&stored)?;
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
