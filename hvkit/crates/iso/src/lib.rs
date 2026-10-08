//! Mastering ISO 9660 images with libisofs (linked dynamically), for Windows NT 5.x setup CDs and
//! for CSMWrap boot CDs.

use std::ffi::{CString, c_char, c_int};
use std::fmt;
use std::path::Path;

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// How to master an image. Paths are inside the tree, absolute ("/boot.img").
#[derive(Debug, Clone)]
pub struct Options {
    pub volume_id: String,
    /// A no-emulation BIOS boot image and how many 512-byte sectors the BIOS loads.
    pub bios_boot: Option<(String, u16)>,
    /// An EFI System Partition image (FAT) for an El Torito entry with platform 0xEF.
    pub efi_boot: Option<String>,
    /// Where the boot catalog goes; it is hidden from the ISO 9660 tree.
    pub catalog: String,
    /// Files hidden from the ISO 9660 tree (Joliet still lists them).
    pub hide: Vec<String>,
    pub iso_level: u8,
    pub rock_ridge: bool,
    pub joliet: bool,
    /// Add "." to names without an extension ("WIN51." instead of "WIN51").
    pub force_dots: bool,
}

impl Options {
    /// A Windows NT 5.x setup CD, as Microsoft's are: ISO 9660 level 2 names without ";1" and without
    /// a forced ".", 7-bit names ("$OEM$"), deep directories, Joliet with long names, El Torito
    /// no-emulation boot loading 4 sectors, the boot image hidden from the ISO 9660 tree. No Rock
    /// Ridge: it makes the directory records bigger, and the CD boot sector (etfsboot) reads only the
    /// first 128 sectors of \I386 when it looks for SETUPLDR.BIN.
    pub fn nt5_setup(volume_id: &str, boot_image: &str) -> Options {
        Options {
            volume_id: volume_id.to_string(),
            bios_boot: Some((boot_image.to_string(), 4)),
            efi_boot: None,
            catalog: "/boot.catalog".into(),
            hide: vec![boot_image.to_string()],
            iso_level: 2,
            rock_ridge: false,
            joliet: true,
            force_dots: false,
        }
    }

    /// A Windows NT 6.x setup DVD (Vista and later), as Microsoft's are: ISO 9660 level 3 names
    /// without ";1" and without a forced ".", 7-bit names, deep directories, Joliet with long names,
    /// El Torito no-emulation boot loading all of etfsboot.com (8 sectors of 512: it is 4 KiB, twice
    /// the NT 5 one), the boot image hidden from the ISO 9660 tree. No Rock Ridge. The boot sector
    /// (etfsboot) looks for BOOTMGR in the root by name, so the names must keep neither the ";1"
    /// version suffix nor the forced ".".
    pub fn nt6_setup(volume_id: &str, boot_image: &str) -> Options {
        Options {
            volume_id: volume_id.to_string(),
            bios_boot: Some((boot_image.to_string(), 8)),
            efi_boot: None,
            catalog: "/boot.catalog".into(),
            hide: vec![boot_image.to_string()],
            iso_level: 3,
            rock_ridge: false,
            joliet: true,
            force_dots: false,
        }
    }
}

#[repr(C)]
struct CopyOpts {
    volume_id: *const c_char,
    bios_boot: *const c_char,
    boot_load_size: c_int,
    efi_boot: *const c_char,
    catalog: *const c_char,
    hide: *const *const c_char,
    iso_level: c_int,
    rock_ridge: c_int,
    joliet: c_int,
    force_dots: c_int,
}

unsafe extern "C" {
    fn hvkit_iso_build(
        root: *const c_char,
        out: *const c_char,
        o: *const CopyOpts,
        err: *mut c_char,
        errlen: usize,
    ) -> c_int;
}

fn cstr(s: &str) -> Result<CString, Error> {
    CString::new(s).map_err(|_| Error(format!("NUL in {s:?}")))
}

/// Masters the directory `root` into `out`. An existing `out` is truncated and rewritten, not
/// replaced, so it keeps its ACL.
pub fn build(root: &Path, out: &Path, o: &Options) -> Result<(), Error> {
    // libisofs does not find a relative root ("The file does not exist in the filesystem").
    let root = std::path::absolute(root).map_err(|e| Error(format!("{}: {e}", root.display())))?;
    let root_c = cstr(&root.to_string_lossy())?;
    let out_c = cstr(&out.to_string_lossy())?;
    let vol = cstr(&o.volume_id)?;
    let bios = o.bios_boot.as_ref().map(|(p, _)| cstr(p)).transpose()?;
    let efi = o.efi_boot.as_deref().map(cstr).transpose()?;
    let cat = cstr(&o.catalog)?;
    let hide: Vec<CString> = o.hide.iter().map(|h| cstr(h)).collect::<Result<_, _>>()?;
    let mut hide_ptrs: Vec<*const c_char> = hide.iter().map(|h| h.as_ptr()).collect();
    hide_ptrs.push(std::ptr::null());
    let opts = CopyOpts {
        volume_id: vol.as_ptr(),
        bios_boot: bios.as_ref().map_or(std::ptr::null(), |b| b.as_ptr()),
        boot_load_size: o.bios_boot.as_ref().map_or(0, |(_, n)| c_int::from(*n)),
        efi_boot: efi.as_ref().map_or(std::ptr::null(), |e| e.as_ptr()),
        catalog: cat.as_ptr(),
        hide: hide_ptrs.as_ptr(),
        iso_level: c_int::from(o.iso_level),
        rock_ridge: c_int::from(o.rock_ridge),
        joliet: c_int::from(o.joliet),
        force_dots: c_int::from(o.force_dots),
    };
    let mut err = vec![0u8; 512];
    let r = unsafe {
        hvkit_iso_build(
            root_c.as_ptr(),
            out_c.as_ptr(),
            &opts,
            err.as_mut_ptr().cast(),
            err.len(),
        )
    };
    if r != 0 {
        let end = err.iter().position(|&c| c == 0).unwrap_or(err.len());
        return Err(Error(String::from_utf8_lossy(&err[..end]).into_owned()));
    }
    Ok(())
}
