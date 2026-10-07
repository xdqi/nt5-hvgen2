//! The Windows backend: Microsoft's Offline Registry Library, offreg.dll in System32 (part of
//! Windows, not shipped with hvkit), through the `windows` crate's bindings. OROpenHive reads the
//! whole file and leaves it alone; ORSaveHive writes a new, compacted hive and cannot overwrite a
//! file, so `commit` saves next to the target and renames.
//!
//! offreg works with key handles. A [`Node`] is an index into a table of the keys this hive has
//! seen; their handles are opened when first needed and closed with the hive.

use crate::{Error, Header, Node, Result, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use windows::Wdk::System::OfflineRegistry::{
    ORCloseHive, ORCloseKey, ORCreateKey, ORDeleteKey, ORDeleteValue, OREnumKey, OREnumValue,
    ORHKEY, OROpenHive, OROpenKey, ORQueryInfoKey, ORSaveHive, ORSetValue,
};
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, WIN32_ERROR,
};
use windows::core::{PCWSTR, PWSTR};

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain([0]).collect()
}

fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn check(e: WIN32_ERROR, what: impl FnOnce() -> String) -> Result<()> {
    if e.is_ok() {
        return Ok(());
    }
    Err(Error(format!(
        "{}: {}",
        what(),
        std::io::Error::from_raw_os_error(e.0 as i32)
    )))
}

/// The Windows version that writes regf 1.`minor`, the oldest that reads it (ORSaveHive writes 1.3
/// for 5.0 and 1.5 for 5.1 and later).
fn os_for_format(minor: u32) -> (u32, u32) {
    match minor {
        0..=3 => (5, 0),
        4 | 5 => (5, 1),
        _ => (10, 0),
    }
}

/// A key this hive has seen.
struct Entry {
    /// Invalid (null) until opened, and again once deleted.
    handle: ORHKEY,
    /// None for the root key.
    parent: Option<usize>,
    /// The name; as stored in the hive once `exact`, else as asked for.
    name: String,
    exact: bool,
    deleted: bool,
}

/// A hive file opened with offreg. Changes stay in memory until [`Hive::commit`].
pub struct Hive {
    keys: RefCell<Vec<Entry>>,
    /// (parent, upper-case name) -> index into `keys`
    names: RefCell<HashMap<(usize, String), usize>>,
    path: PathBuf,
    writable: bool,
    os: (u32, u32),
}

impl Hive {
    pub fn open(path: &Path, writable: bool) -> Result<Hive> {
        let mut head = Vec::new();
        std::fs::File::open(path)
            .and_then(|f| std::io::Read::read_to_end(&mut std::io::Read::take(f, 0x200), &mut head))
            .map_err(|e| Error(format!("{}: {e}", path.display())))?;
        let header = Header::read(&head).map_err(|e| Error(format!("{}: {e}", path.display())))?;
        let mut root = ORHKEY::default();
        let p = wide(path.as_os_str());
        check(unsafe { OROpenHive(PCWSTR(p.as_ptr()), &mut root) }, || {
            format!("{}: OROpenHive", path.display())
        })?;
        Ok(Hive {
            keys: RefCell::new(vec![Entry {
                handle: root,
                parent: None,
                name: String::new(),
                exact: true,
                deleted: false,
            }]),
            names: RefCell::new(HashMap::new()),
            path: path.to_path_buf(),
            writable,
            os: os_for_format(header.version.1),
        })
    }

    /// The Windows version the hive is for, e.g. (5, 1) for XP: `commit` writes the file in its
    /// format (ORSaveHive). Default: the oldest version that reads the file's format.
    pub fn set_os_version(&mut self, major: u32, minor: u32) {
        self.os = (major, minor);
    }

    pub fn root(&self) -> Node {
        Node(0)
    }

    /// The key's handle, opened through its parent's if need be.
    fn handle(&self, node: Node) -> Result<ORHKEY> {
        let (parent, name) = {
            let keys = self.keys.borrow();
            let e = &keys[node.0];
            if e.deleted {
                return Err(Error(format!("key {:?} was deleted", e.name)));
            }
            if !e.handle.is_invalid() {
                return Ok(e.handle);
            }
            (
                e.parent.expect("the root key is always open"),
                e.name.clone(),
            )
        };
        let p = self.handle(Node(parent))?;
        let mut h = ORHKEY::default();
        let n = wstr(&name);
        check(unsafe { OROpenKey(p, PCWSTR(n.as_ptr()), &mut h) }, || {
            format!("OROpenKey {name}")
        })?;
        self.keys.borrow_mut()[node.0].handle = h;
        Ok(h)
    }

    /// The node for the subkey `name` of `parent`, registered if new, with `handle` if it is open
    /// already; `exact` if the name is as stored.
    fn entry(&self, parent: usize, name: &str, handle: ORHKEY, exact: bool) -> Node {
        let k = (parent, name.to_uppercase());
        let known = self.names.borrow().get(&k).copied();
        let mut keys = self.keys.borrow_mut();
        if let Some(i) = known {
            if exact {
                keys[i].name = name.to_string();
                keys[i].exact = true;
            }
            if !handle.is_invalid() {
                if keys[i].handle.is_invalid() {
                    keys[i].handle = handle;
                } else {
                    let _ = unsafe { ORCloseKey(handle) };
                }
            }
            return Node(i);
        }
        keys.push(Entry {
            handle,
            parent: Some(parent),
            name: name.to_string(),
            exact,
            deleted: false,
        });
        self.names.borrow_mut().insert(k, keys.len() - 1);
        Node(keys.len() - 1)
    }

    /// The names of the subkeys as stored, in the order of the hive's index (sorted).
    fn subkey_names(&self, node: Node) -> Result<Vec<String>> {
        let h = self.handle(node)?;
        let mut out = Vec::new();
        let mut buf = [0u16; 512];
        for i in 0.. {
            let mut n = buf.len() as u32;
            let e = unsafe { OREnumKey(h, i, PWSTR(buf.as_mut_ptr()), &mut n, None, None, None) };
            if e == ERROR_NO_MORE_ITEMS {
                break;
            }
            check(e, || "OREnumKey".into())?;
            out.push(String::from_utf16_lossy(&buf[..n as usize]));
        }
        Ok(out)
    }

    /// The key's name as stored (empty for the root key: offreg does not tell its name).
    pub fn name(&self, node: Node) -> Result<String> {
        let (parent, name, exact) = {
            let e = &self.keys.borrow()[node.0];
            (e.parent, e.name.clone(), e.exact)
        };
        if exact {
            return Ok(name);
        }
        let parent = parent.expect("the root key's name is exact");
        let upper = name.to_uppercase();
        for n in self.subkey_names(Node(parent))? {
            if n.to_uppercase() == upper {
                let mut keys = self.keys.borrow_mut();
                keys[node.0].name = n.clone();
                keys[node.0].exact = true;
                return Ok(n);
            }
        }
        Ok(name)
    }

    /// The subkeys, in the order of the hive's index (sorted by name).
    pub fn children(&self, node: Node) -> Result<Vec<Node>> {
        Ok(self
            .subkey_names(node)?
            .iter()
            .map(|n| self.entry(node.0, n, ORHKEY::default(), true))
            .collect())
    }

    pub fn parent(&self, node: Node) -> Option<Node> {
        self.keys.borrow()[node.0].parent.map(Node)
    }

    /// The subkey `name` of `node`, compared case-insensitively.
    pub fn child(&self, node: Node, name: &str) -> Result<Option<Node>> {
        if name.is_empty() || name.contains('\\') {
            return Ok(None);
        }
        let known = self
            .names
            .borrow()
            .get(&(node.0, name.to_uppercase()))
            .copied();
        if let Some(i) = known {
            return Ok(Some(Node(i)));
        }
        let p = self.handle(node)?;
        let mut h = ORHKEY::default();
        let n = wstr(name);
        match unsafe { OROpenKey(p, PCWSTR(n.as_ptr()), &mut h) } {
            ERROR_FILE_NOT_FOUND => Ok(None),
            e => {
                check(e, || format!("OROpenKey {name}"))?;
                Ok(Some(self.entry(node.0, name, h, false)))
            }
        }
    }

    /// Adds the subkey `name`, which must not exist yet. With no security descriptor given, offreg
    /// gives it the parent's inheritable entries, as Windows does.
    pub(crate) fn add_child(&mut self, node: Node, name: &str) -> Result<Node> {
        let p = self.handle(node)?;
        let (mut h, mut disposition) = (ORHKEY::default(), 0u32);
        let n = wstr(name);
        check(
            unsafe {
                ORCreateKey(
                    p,
                    PCWSTR(n.as_ptr()),
                    PCWSTR::null(),
                    None,
                    None,
                    &mut h,
                    Some(&mut disposition),
                )
            },
            || format!("ORCreateKey {name}"),
        )?;
        // 1: created (with this name), 2: an existing key opened.
        Ok(self.entry(node.0, name, h, disposition == 1))
    }

    /// Deletes a key and everything below it.
    pub fn delete(&mut self, node: Node) -> Result<()> {
        let Some(parent) = self.parent(node) else {
            return Err(Error("refusing to delete the root key".into()));
        };
        for c in self.children(node)? {
            self.delete(c)?;
        }
        let name = self.name(node)?;
        let p = self.handle(parent)?;
        let h = std::mem::take(&mut self.keys.borrow_mut()[node.0].handle);
        if !h.is_invalid() {
            let _ = unsafe { ORCloseKey(h) };
        }
        let n = wstr(&name);
        check(unsafe { ORDeleteKey(p, PCWSTR(n.as_ptr())) }, || {
            format!("ORDeleteKey {name}")
        })?;
        self.keys.borrow_mut()[node.0].deleted = true;
        self.names
            .borrow_mut()
            .remove(&(parent.0, name.to_uppercase()));
        Ok(())
    }

    /// The values of a key, in the order they are stored.
    pub fn values(&self, node: Node) -> Result<Vec<Value>> {
        let h = self.handle(node)?;
        let (mut count, mut max_name, mut max_data) = (0u32, 0u32, 0u32);
        check(
            unsafe {
                ORQueryInfoKey(
                    h,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(&mut count),
                    Some(&mut max_name),
                    Some(&mut max_data),
                    None,
                    None,
                )
            },
            || "ORQueryInfoKey".into(),
        )?;
        let mut name = vec![0u16; max_name as usize + 1];
        let mut data = vec![0u8; (max_data as usize).max(1)];
        let mut out = Vec::with_capacity(count as usize);
        let mut i = 0;
        loop {
            let (mut n, mut ty, mut len) = (name.len() as u32, 0u32, data.len() as u32);
            let e = unsafe {
                OREnumValue(
                    h,
                    i,
                    PWSTR(name.as_mut_ptr()),
                    &mut n,
                    Some(&mut ty),
                    Some(data.as_mut_ptr()),
                    Some(&mut len),
                )
            };
            match e {
                ERROR_NO_MORE_ITEMS => break,
                ERROR_MORE_DATA => {
                    name.resize(name.len().max(n as usize + 1) * 2, 0);
                    data.resize(data.len().max(len as usize) * 2, 0);
                    continue;
                }
                e => check(e, || "OREnumValue".into())?,
            }
            out.push(Value {
                name: String::from_utf16_lossy(&name[..n as usize]),
                ty,
                data: data[..len as usize].to_vec(),
            });
            i += 1;
        }
        Ok(out)
    }

    /// Creates or replaces one value (a replaced value keeps its place and its name's case).
    pub fn set(&mut self, node: Node, v: &Value) -> Result<()> {
        let h = self.handle(node)?;
        if u32::try_from(v.data.len()).is_err() {
            return Err(Error(format!("{:?}: too long", v.name)));
        }
        let n = wstr(&v.name);
        check(
            unsafe { ORSetValue(h, PCWSTR(n.as_ptr()), v.ty, Some(&v.data)) },
            || format!("ORSetValue {:?}", v.name),
        )
    }

    /// Deletes one value; returns whether it existed.
    pub fn delete_value(&mut self, node: Node, name: &str) -> Result<bool> {
        let h = self.handle(node)?;
        let n = wstr(name);
        match unsafe { ORDeleteValue(h, PCWSTR(n.as_ptr())) } {
            ERROR_FILE_NOT_FOUND => Ok(false),
            e => check(e, || format!("ORDeleteValue {name:?}")).map(|()| true),
        }
    }

    /// Writes the hive to `path`, or back to the file it was opened from, in the format of the
    /// version [`Hive::set_os_version`] gave.
    pub fn commit(&mut self, path: Option<&Path>) -> Result<()> {
        if !self.writable {
            return Err(Error("the hive was opened read-only".into()));
        }
        let target = path.unwrap_or(&self.path).to_path_buf();
        let mut tmp = target.clone().into_os_string();
        tmp.push(".hvkit-new");
        let tmp = PathBuf::from(tmp);
        match std::fs::remove_file(&tmp) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(Error(format!("{}: {e}", tmp.display())));
            }
            _ => {}
        }
        let root = self.keys.borrow()[0].handle;
        let t = wide(tmp.as_os_str());
        check(
            unsafe { ORSaveHive(root, PCWSTR(t.as_ptr()), self.os.0, self.os.1) },
            || format!("{}: ORSaveHive", tmp.display()),
        )?;
        std::fs::rename(&tmp, &target).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            Error(format!("{}: {e}", target.display()))
        })
    }
}

impl Drop for Hive {
    fn drop(&mut self) {
        let keys = self.keys.get_mut();
        for e in keys.iter().skip(1) {
            if !e.handle.is_invalid() {
                let _ = unsafe { ORCloseKey(e.handle) };
            }
        }
        let _ = unsafe { ORCloseHive(keys[0].handle) };
    }
}
