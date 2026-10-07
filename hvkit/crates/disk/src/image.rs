//! A disk image, raw or VHDX, read and written through a page cache.
//!
//! VHDX images are read through their whole differencing chain (parents found by the locator's
//! relative path, next to the child) and written only to the image itself. The cache collects writes
//! and stores them in runs on flush: vhdx-rs syncs on every write, and the file system code writes in
//! small pieces. Pages that were zero and still are are not written, so that an image stays sparse and
//! a VHDX gets no blocks for them.

use crate::{Error, Result};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use vhdx::{LogReplayPolicy, Medium};

const PAGE: u64 = 64 * 1024;
/// Dirty pages held before a flush.
const MAX_DIRTY: usize = 1024;
/// Pages held before clean ones are dropped.
const MAX_PAGES: usize = 4096;
pub const SECTOR: u64 = 512;

enum Backing {
    Raw(File),
    Vhdx(Box<Medium<File>>),
}

struct Page {
    data: Vec<u8>,
    dirty: bool,
    was_zero: bool,
}

pub struct Image {
    path: PathBuf,
    backing: Backing,
    size: u64,
    writable: bool,
    pages: BTreeMap<u64, Page>,
    dirty: usize,
}

fn io_err(p: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error(format!("{}: {e}", p.display()))
}

fn vhdx_err(p: &Path) -> impl Fn(vhdx::Error) -> Error + '_ {
    move |e| Error(format!("{}: {e}", p.display()))
}

fn is_vhdx(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("vhdx") || e.eq_ignore_ascii_case("avhdx"))
}

/// Opens a VHDX and, recursively, its parents (read only), by the locator's relative_path or, without
/// one, the file name of its absolute path, next to the child.
fn open_vhdx(path: &Path, writable: bool) -> Result<Medium<File>> {
    let file = File::options()
        .read(true)
        .write(writable)
        .open(path)
        .map_err(io_err(path))?;
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let resolver = move |req: vhdx::ParentRequest<'_>| -> vhdx::Result<Medium<File>> {
        let loc = req.locator();
        let data = loc.key_value_data();
        let mut rel = None;
        let mut abs = None;
        for e in loc.entries() {
            match e.key(data)?.as_str() {
                "relative_path" => rel = Some(e.value(data)?),
                "absolute_win32_path" => abs = Some(e.value(data)?),
                _ => {}
            }
        }
        let name = rel.or(abs).unwrap_or_default();
        let name = name.trim_start_matches(".\\").replace('\\', "/");
        let p = if name.contains(':') {
            dir.join(name.rsplit('/').next().unwrap_or(&name))
        } else {
            dir.join(&name)
        };
        open_vhdx(&p, false).map_err(|e| vhdx::Error::InvalidParentLocator(e.0))
    };
    let m = if writable {
        Medium::open(file)
            .write()
            .log_replay(LogReplayPolicy::Auto)
            .with_parent_resolver(resolver)
            .finish()
    } else {
        Medium::open(file)
            .log_replay(LogReplayPolicy::Auto)
            .with_parent_resolver(resolver)
            .finish()
    };
    m.map_err(vhdx_err(path))
}

impl Image {
    /// Opens an image; the format is taken from the extension (.vhdx, .avhdx; anything else is raw).
    pub fn open(path: &Path, writable: bool) -> Result<Image> {
        let (backing, size) = if is_vhdx(path) {
            let m = open_vhdx(path, writable)?;
            let size = m
                .sections()
                .and_then(|s| s.metadata()?.items().virtual_disk_size())
                .map_err(vhdx_err(path))?;
            (Backing::Vhdx(Box::new(m)), size)
        } else {
            let f = File::options()
                .read(true)
                .write(writable)
                .open(path)
                .map_err(io_err(path))?;
            let size = f.metadata().map_err(io_err(path))?.len();
            (Backing::Raw(f), size)
        };
        Ok(Image {
            path: path.to_path_buf(),
            backing,
            size,
            writable,
            pages: BTreeMap::new(),
            dirty: 0,
        })
    }

    /// Creates an empty image of `size` bytes: a sparse raw file, or a dynamic VHDX with 512-byte
    /// sectors and `block_size` blocks for a .vhdx path. An existing file is truncated and rewritten in
    /// place, which keeps its ACL.
    pub fn create(path: &Path, size: u64, block_size: u32) -> Result<Image> {
        if !size.is_multiple_of(SECTOR) {
            return Err(Error(format!(
                "{}: size {size} is not a multiple of 512",
                path.display()
            )));
        }
        let f = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(io_err(path))?;
        let backing = if is_vhdx(path) {
            let m = Medium::create(f)
                .size(size)
                .block_size(block_size)
                .logical_sector_size(SECTOR as u32)
                .physical_sector_size(SECTOR as u32)
                .finish()
                .map_err(vhdx_err(path))?;
            Backing::Vhdx(Box::new(m))
        } else {
            f.set_len(size).map_err(io_err(path))?;
            Backing::Raw(f)
        };
        Ok(Image {
            path: path.to_path_buf(),
            backing,
            size,
            writable: true,
            pages: BTreeMap::new(),
            dirty: 0,
        })
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_raw(&mut self, off: u64, buf: &mut [u8]) -> Result<()> {
        let path = self.path.clone();
        match &mut self.backing {
            Backing::Raw(f) => {
                f.seek(SeekFrom::Start(off)).map_err(io_err(&path))?;
                f.read_exact(buf).map_err(io_err(&path))
            }
            Backing::Vhdx(m) => {
                let mut io = m.io().map_err(vhdx_err(&path))?;
                let mut s = io
                    .sector(off / SECTOR, buf.len() as u64 / SECTOR)
                    .map_err(vhdx_err(&path))?;
                s.read_exact(buf).map_err(io_err(&path))
            }
        }
    }

    fn write_raw(&mut self, off: u64, data: &[u8]) -> Result<()> {
        let path = self.path.clone();
        match &mut self.backing {
            Backing::Raw(f) => {
                f.seek(SeekFrom::Start(off)).map_err(io_err(&path))?;
                f.write_all(data).map_err(io_err(&path))
            }
            Backing::Vhdx(m) => {
                let mut io = m.io().map_err(vhdx_err(&path))?;
                let mut s = io
                    .sector(off / SECTOR, data.len() as u64 / SECTOR)
                    .map_err(vhdx_err(&path))?;
                s.write_all(data).map_err(io_err(&path))
            }
        }
    }

    fn page(&mut self, n: u64) -> Result<&mut Page> {
        if !self.pages.contains_key(&n) {
            if self.pages.len() >= MAX_PAGES {
                self.pages.retain(|_, p| p.dirty);
            }
            let len = PAGE.min(self.size - n * PAGE) as usize;
            let mut data = vec![0; len];
            self.read_raw(n * PAGE, &mut data)?;
            let was_zero = data.iter().all(|&b| b == 0);
            self.pages.insert(
                n,
                Page {
                    data,
                    dirty: false,
                    was_zero,
                },
            );
        }
        Ok(self.pages.get_mut(&n).unwrap())
    }

    fn check(&self, off: u64, len: usize) -> Result<()> {
        if off
            .checked_add(len as u64)
            .is_none_or(|end| end > self.size)
        {
            return Err(Error(format!(
                "{}: access past the end of the disk ({off} + {len})",
                self.path.display()
            )));
        }
        Ok(())
    }

    pub fn read_at(&mut self, mut off: u64, mut buf: &mut [u8]) -> Result<()> {
        self.check(off, buf.len())?;
        while !buf.is_empty() {
            let (n, in_page) = (off / PAGE, (off % PAGE) as usize);
            let p = self.page(n)?;
            let k = buf.len().min(p.data.len() - in_page);
            buf[..k].copy_from_slice(&p.data[in_page..in_page + k]);
            buf = &mut buf[k..];
            off += k as u64;
        }
        Ok(())
    }

    pub fn write_at(&mut self, mut off: u64, mut data: &[u8]) -> Result<()> {
        if !self.writable {
            return Err(Error(format!("{}: opened read-only", self.path.display())));
        }
        self.check(off, data.len())?;
        while !data.is_empty() {
            let (n, in_page) = (off / PAGE, (off % PAGE) as usize);
            let p = self.page(n)?;
            let k = data.len().min(p.data.len() - in_page);
            p.data[in_page..in_page + k].copy_from_slice(&data[..k]);
            if !p.dirty {
                p.dirty = true;
                self.dirty += 1;
            }
            data = &data[k..];
            off += k as u64;
        }
        if self.dirty >= MAX_DIRTY {
            self.flush()?;
        }
        Ok(())
    }

    /// Writes the dirty pages, contiguous ones in one write.
    pub fn flush(&mut self) -> Result<()> {
        self.dirty = 0;
        let mut runs: Vec<(u64, Vec<u8>)> = Vec::new();
        for (&n, p) in self.pages.iter_mut().filter(|(_, p)| p.dirty) {
            p.dirty = false;
            if p.was_zero && p.data.iter().all(|&b| b == 0) {
                continue;
            }
            p.was_zero = false;
            match runs.last_mut() {
                Some((start, data)) if *start + data.len() as u64 == n * PAGE => {
                    data.extend_from_slice(&p.data)
                }
                _ => runs.push((n * PAGE, p.data.clone())),
            }
        }
        for (off, data) in runs {
            self.write_raw(off, &data)?;
        }
        if let Backing::Raw(f) = &mut self.backing {
            f.flush().map_err(io_err(&self.path))?;
        }
        Ok(())
    }

    /// A window on part of the image (e.g. a partition), usable as a file.
    pub fn window(&mut self, start: u64, len: u64) -> Window<'_> {
        Window {
            img: self,
            start,
            len,
            pos: 0,
        }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        if self.dirty > 0
            && let Err(e) = self.flush()
        {
            eprintln!("{e}");
        }
    }
}

/// Bytes `start..start + len` of an image as a Read + Write + Seek stream.
pub struct Window<'a> {
    img: &'a mut Image,
    start: u64,
    len: u64,
    pos: u64,
}

fn to_io(e: Error) -> std::io::Error {
    std::io::Error::other(e.0)
}

impl Read for Window<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = (buf.len() as u64).min(self.len.saturating_sub(self.pos)) as usize;
        self.img
            .read_at(self.start + self.pos, &mut buf[..n])
            .map_err(to_io)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Write for Window<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let n = (data.len() as u64).min(self.len.saturating_sub(self.pos)) as usize;
        if n == 0 && !data.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "write past the end of the partition",
            ));
        }
        self.img
            .write_at(self.start + self.pos, &data[..n])
            .map_err(to_io)?;
        self.pos += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.img.flush().map_err(to_io)
    }
}

impl Seek for Window<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
        };
        match p {
            Some(p) => {
                self.pos = p;
                Ok(p)
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before the start",
            )),
        }
    }
}
