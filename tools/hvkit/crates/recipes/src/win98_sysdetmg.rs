//! SYSDETMG.DLL, Windows 98's hardware detection manager: send its port I/O through the GEN2LEG shim
//! VxD instead of `in`/`out`. Port of the CSMWrap testbed's w98/patch-sysdetmg.py.
//!
//! Detection busy-waits for the 8254 PIT's channel 0 counter to wrap, with interrupts disabled.
//! Hyper-V Generation 2 VMs have no PIT, port 40h reads FFh for ever, and setup hangs at "detecting
//! hardware". GEN2LEG emulates the PIT for the VxDs whose port I/O `vxd patch-io` redirects, but this
//! DLL does its I/O from 16-bit ring-0 code, which the I/O permission bitmap cannot trap. So the four
//! places it does port I/O from become `int` to the shim's vectors:
//!
//! ```text
//! seg 1 +0x32  the port helper every access goes through:
//!              in al,dx ; xor ah,ah                ->  int <in dx> ; nop    (the shim clears AH)
//! seg 1 +0x3E  jz +3 ; out dx,al ; jmp +3 ; out dx,ax ; jmp +0
//!                                                  ->  jz +4 ; int <out dx> ; jmp +2 ; out dx,ax ; nop
//!              (16-bit OUTs stay: detection does not use them)
//! seg 4 +0x56E3, seg 6 +0x8349  two PIT readers (latch counter 0, read it low byte first) whose
//!              callers wait for the count to move: the out 43h / in 40h pairs become the shim's
//!              fixed-port operations
//! ```
//!
//! The vectors are the shim's (w9x/gen2leg/vectors.txt), so the patch depends on that file.

use crate::vxd_portio::{IDX_IN_DX, IDX_OUT_DX, VECTORS, vector};
use crate::{Outcome, Result, State, sha256_hex};
use formats::{bail, ne};

pub const SUMMARY: &str = "SYSDETMG.DLL of Windows 98: port I/O through the GEN2LEG shim (hardware detection's PIT waits)";

/// A byte of new code: a literal, or the vector of one of the shim's operations.
#[derive(Clone, Copy)]
enum B {
    X(u8),
    InDx,
    OutDx,
    In40,
    Out43,
}
use B::*;

/// (segment, offset in it, original bytes, new code). The new code may be shorter than the original;
/// the original bytes after it stay (they come after a `ret` and are not reached).
const SITES: [(usize, usize, &[u8], &[B]); 4] = [
    (1, 0x32, &[0xec, 0x32, 0xe4], &[X(0xcd), InDx, X(0x90)]),
    (
        1,
        0x3e,
        &[0x74, 0x03, 0xee, 0xeb, 0x03, 0xef, 0xeb, 0x00],
        &[
            X(0x74),
            X(0x04),
            X(0xcd),
            OutDx,
            X(0xeb),
            X(0x02),
            X(0xef),
            X(0x90),
        ],
    ),
    // push dx; mov al,0; out 43h,al; mov dx,40h; in al,dx; mov ah,al; in al,dx; xchg al,ah; pop dx; retf
    (
        4,
        0x56e3,
        &[
            0x52, 0xb0, 0x00, 0xe6, 0x43, 0xba, 0x40, 0x00, 0xec, 0x8a, 0xe0, 0xec, 0x86, 0xc4,
            0x5a, 0xcb,
        ],
        &[
            X(0x52),
            X(0xb0),
            X(0x00),
            X(0xcd),
            Out43,
            X(0xcd),
            In40,
            X(0x88),
            X(0xc4),
            X(0xcd),
            In40,
            X(0x86),
            X(0xc4),
            X(0x5a),
            X(0xcb),
            X(0x90),
        ],
    ),
    // push dx; mov dx,43h; xor al,al; out dx,al; mov dx,40h; in al,dx; mov ah,al; in al,dx;
    // xchg al,ah; pop dx; ret
    (
        6,
        0x8349,
        &[
            0x52, 0xba, 0x43, 0x00, 0x32, 0xc0, 0xee, 0xba, 0x40, 0x00, 0xec, 0x8a, 0xe0, 0xec,
            0x86, 0xc4, 0x5a, 0xc3,
        ],
        &[
            X(0x52),
            X(0x32),
            X(0xc0),
            X(0xcd),
            Out43,
            X(0xcd),
            In40,
            X(0x88),
            X(0xc4),
            X(0xcd),
            In40,
            X(0x86),
            X(0xc4),
            X(0x5a),
            X(0xc3),
            X(0x90),
            X(0x90),
        ],
    ),
];

/// SYSDETMG.DLL of the zh-hans Windows 98 SE OEM CD (PRECOPY2.CAB).
const KNOWN: [(&str, &str); 1] = [(
    "Windows 98 SE SYSDETMG.DLL 4.10.2222 (zh-hans OEM)",
    "C1E1B6108E2770992A5DE0ECF5E054A491194BC65CCF78990F87CED5D71F97FB",
)];

/// The new code of a site with the vectors filled in.
fn code(new: &[B], vectors: &[u8; VECTORS]) -> Result<Vec<u8>> {
    new.iter()
        .map(|b| {
            Ok(match *b {
                X(x) => x,
                InDx => vectors[IDX_IN_DX],
                OutDx => vectors[IDX_OUT_DX],
                In40 => vector(vectors, 0x40, false)?,
                Out43 => vector(vectors, 0x43, true)?,
            })
        })
        .collect()
}

pub fn apply(input: &[u8], vectors: &[u8; VECTORS]) -> Result<Outcome> {
    let segs = ne::segments(input)?;
    let mut b = input.to_vec();
    let (mut stock, mut patched) = (0, 0);
    let mut log = Vec::new();
    for (seg, off, old, new) in SITES {
        let Some(s) = segs.iter().find(|s| s.number == seg && s.offset != 0) else {
            bail!("no segment {seg} with data (not Windows 98's SYSDETMG.DLL?)");
        };
        if off + old.len() > s.len {
            bail!("segment {seg} is too short for the site at {off:#x}");
        }
        let at = s.offset + off;
        let new = code(new, vectors)?;
        let mut want = old.to_vec();
        want[..new.len()].copy_from_slice(&new);
        let got = &input[at..at + old.len()];
        if got == old {
            stock += 1;
            b[at..at + old.len()].copy_from_slice(&want);
            log.push(format!(
                "patched segment {seg} offset {off:#x} (file {at:#x})"
            ));
        } else if got == want.as_slice() {
            patched += 1;
        } else {
            bail!(
                "segment {seg} offset {off:#x}: unexpected bytes {} (another SYSDETMG.DLL, or patched \
                 with other vectors); refusing to patch",
                got.iter().map(|x| format!("{x:02x}")).collect::<String>()
            );
        }
    }
    if patched == SITES.len() {
        return Ok(Outcome {
            state: State::Patched,
            bytes: input.to_vec(),
            log: vec!["already patched".into()],
        });
    }
    if patched != 0 {
        bail!(
            "{patched} of the {} sites are patched, the others not",
            SITES.len()
        );
    }
    debug_assert_eq!(stock, SITES.len());
    let sha = sha256_hex(input);
    let state = KNOWN
        .iter()
        .find(|(_, h)| *h == sha)
        .map_or(State::Untested, |(n, _)| State::Known(n));
    Ok(Outcome {
        state,
        bytes: b,
        log,
    })
}
