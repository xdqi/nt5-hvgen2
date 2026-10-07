//! Windows 98 VxDs (VPICD, VTD, VKD): redirect their port I/O to the legacy PIC, PIT, port 61h and
//! the i8042 into the GEN2LEG shim VxD, which emulates them. Port of the CSMWrap testbed's
//! w98/patch-io.py.
//!
//! Hyper-V Generation 2 VMs have no 8259 PICs, no 8254 PIT, nothing at port 61h and no i8042. Each
//! `in`/`out` to those ports in the 32-bit code objects is replaced in place by a 2-byte `int vv`; the
//! shim owns one IDT vector per (port, direction), since the instruction encodes nothing else:
//!
//! ```text
//! in  al, imm8   E4 pp   ->  CD vv
//! out imm8, al   E6 pp   ->  CD vv
//! ```
//!
//! The vectors come from the shim's build (w9x/gen2leg/vectors.txt), in the order of [`OPS`], then the
//! three "port in DX at run time" operations, the four i8042 ones and the two plain `in al,dx` /
//! `out dx,al` ones that SYSDETMG.DLL's port helper uses ([`crate::win98_sysdetmg`]). Sites are found by a linear sweep of
//! each code object. VxD code calls services as `int 20h` followed by a dword service id, so the sweep
//! skips the 4 bytes after each `int 20h`. A few sites whose port is in DX at run time were found by
//! hand ([`SPECIAL`]), and VPICD's init object is left alone ([`SKIP`]).

use crate::{Error, Outcome, Result, State};
use formats::bail;
use formats::le::{Fixup, Le};
use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, NasmFormatter};
use std::collections::HashSet;

pub const PORTS: [u8; 11] = [
    0x20, 0x21, 0xa0, 0xa1, 0x40, 0x41, 0x42, 0x43, 0x60, 0x61, 0x64,
];
/// The shim's operations with a vector each, in the order of vectors.txt: (port, out).
pub const OPS: [(u8, bool); 13] = [
    (0x20, false),
    (0x20, true),
    (0x21, false),
    (0x21, true),
    (0xa0, false),
    (0xa0, true),
    (0xa1, false),
    (0xa1, true),
    (0x40, false),
    (0x40, true),
    (0x43, true),
    (0x61, false),
    (0x61, true),
];
/// Operations whose port is in DX at run time: `in al, dx; ret`, `out dx, al; ret`, `out dx, al; jmp $+2;
/// in al, dx`.
const IDX_IN_DX_RET: usize = 13;
const IDX_OUT_DX_RET: usize = 14;
const IDX_OUT_IN_DX: usize = 15;
/// VKD's i8042 accesses.
const OPS_I8042: [((u8, bool), usize); 4] = [
    ((0x60, false), 16),
    ((0x60, true), 17),
    ((0x64, false), 18),
    ((0x64, true), 19),
];
/// `in al, dx` and `out dx, al` with the port in DX, carrying on with the next instruction.
pub const IDX_IN_DX: usize = 20;
pub const IDX_OUT_DX: usize = 21;
pub const VECTORS: usize = 22;

/// Objects left alone: VPICD's discardable init code only writes the 8259 init sequence (ICW1-4) and
/// reads the old masks; two of its ICW writes sit under bogus-looking relocation records, so none of it
/// is patched and the shim starts from the state the sequence would leave. (VTD's init code reads the
/// PIT counter and has to be patched.)
const SKIP: [(&str, usize); 1] = [("VPICD", 4)];

enum New {
    Bytes(&'static [u8]),
    /// `int` to the vector of operation n, then NOPs to the length of the old bytes.
    Int(usize),
}

/// (module, object, offset, expected bytes, replacement)
const SPECIAL: [(&str, usize, u32, &[u8], New); 5] = [
    (
        "VPICD",
        1,
        0x17db,
        &[0xee, 0xeb, 0x00, 0xec],
        New::Int(IDX_OUT_IN_DX),
    ),
    // Init code that switches the local APIC off (Win9x has no APIC support): rdmsr 1Bh; and eax,
    // 0xfffff7ff; wrmsr. Without the wrmsr the APIC stays enabled, and Hyper-V's synthetic timer can
    // deliver IRQ 0's vector through it.
    ("VPICD", 4, 0x67, &[0x0f, 0x30], New::Bytes(&[0x90, 0x90])),
    // The same in the 8259 re-initialisation.
    ("VPICD", 1, 0x5e2, &[0x0f, 0x30], New::Bytes(&[0x90, 0x90])),
    ("VTD", 6, 0x40, &[0xee, 0xc3], New::Int(IDX_OUT_DX_RET)),
    ("VTD", 6, 0x76, &[0xec, 0xc3], New::Int(IDX_IN_DX_RET)),
];

/// Parses vectors.txt: the shim's 22 vectors, separated by white space, decimal or 0x-hex.
pub fn parse_vectors(text: &str) -> Result<[u8; VECTORS]> {
    let v: Vec<u8> = text
        .split_whitespace()
        .map(|x| {
            let n = match x.strip_prefix("0x") {
                Some(h) => u8::from_str_radix(h, 16),
                None => x.parse(),
            };
            n.map_err(|_| Error(format!("vectors: bad number {x:?}")))
        })
        .collect::<Result<_>>()?;
    if v.len() != VECTORS
        || v.iter().collect::<HashSet<_>>().len() != VECTORS
        || v.iter().any(|&x| x >= 0x60)
    {
        bail!(
            "vectors: expected {VECTORS} distinct vectors below 0x60 (VMM's IDT has 96 gates), got {}",
            v.len()
        );
    }
    Ok(v.try_into().unwrap())
}

/// The vector of the shim's operation for a fixed port and direction.
pub fn vector(vectors: &[u8; VECTORS], port: u8, out: bool) -> Result<u8> {
    if let Some(i) = OPS.iter().position(|&o| o == (port, out)) {
        return Ok(vectors[i]);
    }
    match OPS_I8042.iter().find(|(o, _)| *o == (port, out)) {
        Some(&(_, i)) => Ok(vectors[i]),
        None => bail!(
            "no shim operation for {} port {port:#04x}",
            if out { "out" } else { "in" }
        ),
    }
}

/// A decoded instruction of the sweep.
#[derive(Debug, Clone)]
pub struct Insn {
    pub offset: u32,
    pub bytes: Vec<u8>,
    pub text: String,
}

/// Linear sweep of a code object. Bytes that do not decode are listed one at a time, as `db`.
pub fn disassemble(code: &[u8], bits: u32) -> Vec<Insn> {
    let mut out = Vec::new();
    let mut fmt = NasmFormatter::new();
    let mut pos = 0usize;
    let mut text = String::new();
    while pos < code.len() {
        let mut d = Decoder::with_ip(bits, &code[pos..], pos as u64, DecoderOptions::NONE);
        let mut ins = Instruction::default();
        d.decode_out(&mut ins);
        let len = if ins.is_invalid() { 1 } else { ins.len() };
        text.clear();
        if ins.is_invalid() {
            text.push_str(&format!("db 0x{:02x}", code[pos]));
        } else {
            fmt.format(&ins, &mut text);
        }
        out.push(Insn {
            offset: pos as u32,
            bytes: code[pos..pos + len].to_vec(),
            text: text.clone(),
        });
        pos += len;
        // A service call: int 20h, then the service id, which is not code.
        if code[pos - len..pos] == [0xcd, 0x20] {
            pos = (pos + 4).min(code.len());
        }
    }
    out
}

/// A port I/O site.
#[derive(Debug, Clone)]
pub struct Site {
    pub object: usize,
    pub offset: u32,
    pub len: u32,
    /// None: the port is in DX at run time and could not be told from the instruction before.
    pub port: Option<u8>,
    pub out: bool,
    pub dx: bool,
    pub bits: u32,
    pub text: String,
    pub overlaps_fixup: bool,
}

fn fixed_bytes(fixups: &[Fixup]) -> HashSet<(usize, u32)> {
    fixups
        .iter()
        .flat_map(|f| (0..f.field_len()).map(move |i| (f.object, f.offset + i)))
        .collect()
}

/// The port I/O sites of the VxD's code objects.
pub fn scan(b: &[u8], le: &Le) -> Result<Vec<Site>> {
    let fixed = fixed_bytes(&le.fixups(b)?);
    let mut res = Vec::new();
    for o in le.objects.iter().filter(|o| o.is_code()) {
        let bits = if o.is_32bit() { 32 } else { 16 };
        let ins = disassemble(&le.object_bytes(b, o.number)?, bits);
        for (k, i) in ins.iter().enumerate() {
            let r = &i.bytes;
            let mut site = None;
            if r.len() == 2 && (r[0] == 0xe4 || r[0] == 0xe6) && PORTS.contains(&r[1]) {
                site = Some((i.offset, 2, r[1], r[0] == 0xe6, false, i.text.clone()));
            } else if r[..] == [0xec] || r[..] == [0xee] {
                let prev = k.checked_sub(1).map(|p| &ins[p]);
                let port = prev.and_then(|p| {
                    let pr = &p.bytes;
                    if pr.len() == 4 && pr[..2] == [0x66, 0xba] {
                        Some(u16::from_le_bytes([pr[2], pr[3]]))
                    } else if bits == 16 && pr.len() == 3 && pr[0] == 0xba {
                        Some(u16::from_le_bytes([pr[1], pr[2]]))
                    } else {
                        None
                    }
                });
                match (port, prev) {
                    (Some(port), Some(p)) if port < 0x100 && PORTS.contains(&(port as u8)) => {
                        site = Some((
                            p.offset,
                            p.bytes.len() as u32 + 1,
                            port as u8,
                            r[0] == 0xee,
                            true,
                            i.text.clone(),
                        ));
                    }
                    (None, _) => res.push(Site {
                        object: o.number,
                        offset: i.offset,
                        len: 1,
                        port: None,
                        out: r[0] == 0xee,
                        dx: true,
                        bits,
                        text: format!(
                            "{}   ; port in DX unknown: {}",
                            i.text,
                            prev.map_or("", |p| p.text.as_str())
                        ),
                        overlaps_fixup: false,
                    }),
                    _ => {}
                }
            }
            if let Some((offset, len, port, out, dx, text)) = site {
                let overlaps_fixup = (0..len).any(|j| fixed.contains(&(o.number, offset + j)));
                res.push(Site {
                    object: o.number,
                    offset,
                    len,
                    port: Some(port),
                    out,
                    dx,
                    bits,
                    text,
                    overlaps_fixup,
                });
            }
        }
    }
    Ok(res)
}

/// Patches the VxD (an LE file, or one inside a bigger file at `header`).
pub fn patch(input: &[u8], header: Option<usize>, vectors: &[u8; VECTORS]) -> Result<Outcome> {
    let mut b = input.to_vec();
    let le = Le::parse(&b, header)?;
    let name = le.ddb(&b)?.name;
    let fixups = le.fixups(&b)?;
    let mut log = Vec::new();
    let mut n = 0;
    for s in scan(&b, &le)? {
        let Some(port) = s.port else { continue };
        if s.bits != 32 || SKIP.contains(&(name.as_str(), s.object)) {
            continue;
        }
        if s.overlaps_fixup {
            bail!(
                "{name} object {} +{:#x}: the site overlaps a fixup",
                s.object,
                s.offset
            );
        }
        if s.dx {
            bail!(
                "{name} object {} +{:#x}: mov dx, imm16 forms are not handled (VPICD, VTD and VKD have none)",
                s.object,
                s.offset
            );
        }
        let v = vector(vectors, port, s.out)?;
        let fo = le.file_offset(&b, s.object, s.offset)?.ok_or_else(|| {
            Error(format!(
                "object {} +{:#x} has no file data",
                s.object, s.offset
            ))
        })?;
        if le.file_offset(&b, s.object, s.offset + s.len - 1)? != Some(fo + s.len as usize - 1) {
            bail!(
                "object {} +{:#x}: the site crosses a page",
                s.object,
                s.offset
            );
        }
        b[fo] = 0xcd;
        b[fo + 1] = v;
        b[fo + 2..fo + s.len as usize].fill(0x90);
        n += 1;
    }
    // Sites whose port is in DX at run time, found by hand.
    for (module, obj, off, old, new) in SPECIAL.iter().filter(|s| s.0 == name) {
        let fo = le
            .file_offset(&b, *obj, *off)?
            .ok_or_else(|| Error(format!("{module} object {obj} +{off:#x} has no file data")))?;
        let bytes = match new {
            New::Bytes(x) => x.to_vec(),
            New::Int(i) => {
                let mut v = vec![0xcd, vectors[*i]];
                v.resize(old.len(), 0x90);
                v
            }
        };
        if b.get(fo..fo + bytes.len()) == Some(&bytes[..]) {
            continue; // patched before
        }
        if b.get(fo..fo + old.len()) != Some(*old) {
            bail!("{module} object {obj} +{off:#x}: unexpected bytes (another build of {module}?)");
        }
        if fixups
            .iter()
            .any(|f| f.object == *obj && *off <= f.offset && f.offset < off + old.len() as u32)
        {
            bail!("{module} object {obj} +{off:#x}: a fixup lies in the site");
        }
        b[fo..fo + bytes.len()].copy_from_slice(&bytes);
        n += 1;
    }
    if n == 0 {
        return Ok(Outcome {
            state: State::Patched,
            bytes: b,
            log: vec![format!("{name}: nothing left to patch")],
        });
    }
    log.push(format!("{name}: {n} sites"));
    Ok(Outcome {
        state: State::Untested,
        bytes: b,
        log,
    })
}
