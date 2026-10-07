//! KEYBOARD.DRV of Windows 98 setup's mini-Windows (MINI.CAB, standard mode): take the scancode
//! from the BDA at 40:16h instead of i8042 port 60h. Port of the CSMWrap testbed's
//! w98/patch-kbd.py.
//!
//! Hyper-V Generation 2 VMs have no i8042. SeaBIOS (hyperv-gen2-win9x, hvkbd.c) raises int 09h for
//! the synthetic keyboard's codes and leaves the scancode at 40:16h. The driver's int 09h handler
//! starts with ES = 40h (the BDA) and reads
//!
//! ```text
//! E4 60            in   al, 60h          ; scancode
//! 26 8A 26 17 00   mov  ah, es:[17h]     ; shift flags
//! ```
//!
//! which becomes one load of both bytes, since 40:17h follows 40:16h:
//!
//! ```text
//! 26 A1 16 00      mov  ax, es:[16h]
//! 90 90 90         nop
//! ```

use crate::{Outcome, Result, State, sha256_hex};
use formats::{bail, ne};

pub const SUMMARY: &str =
    "KEYBOARD.DRV of Windows 98 setup: scancode from the BDA (40:16h), not port 60h";

const ORIG: [u8; 7] = [0xe4, 0x60, 0x26, 0x8a, 0x26, 0x17, 0x00];
const NEW: [u8; 7] = [0x26, 0xa1, 0x16, 0x00, 0x90, 0x90, 0x90];
/// KEYBOARD.DRV of the zh-hans Windows 98 SE OEM CD's MINI.CAB.
const KNOWN: [(&str, &str); 1] = [(
    "Windows 98 SE setup KEYBOARD.DRV (zh-hans OEM)",
    "629B3CF54676BB06B38316E0CEA730BC53B79A36C9B8B2D492431B7C118DC1E7",
)];

/// (segment, offset in it, file offset) of each occurrence of `pat` in the code segments.
fn find(b: &[u8], pat: &[u8]) -> Result<Vec<(usize, usize, usize)>> {
    let mut hits = Vec::new();
    for s in ne::segments(b)?
        .into_iter()
        .filter(|s| s.is_code() && s.offset != 0)
    {
        let end = (s.offset + s.len).min(b.len());
        let Some(data) = b.get(s.offset..end) else {
            continue;
        };
        for (i, w) in data.windows(pat.len()).enumerate() {
            if w == pat {
                hits.push((s.number, i, s.offset + i));
            }
        }
    }
    Ok(hits)
}

pub fn apply(input: &[u8]) -> Result<Outcome> {
    let orig = find(input, &ORIG)?;
    let new = find(input, &NEW)?;
    if orig.is_empty() && new.len() == 1 {
        return Ok(Outcome {
            state: State::Patched,
            bytes: input.to_vec(),
            log: vec!["already patched".into()],
        });
    }
    if orig.len() != 1 {
        bail!(
            "expected one int 09h scancode read (in al, 60h; mov ah, es:[17h]), found {}",
            orig.len()
        );
    }
    let (seg, off, at) = orig[0];
    let mut b = input.to_vec();
    b[at..at + NEW.len()].copy_from_slice(&NEW);
    let sha = sha256_hex(input);
    let state = KNOWN
        .iter()
        .find(|(_, h)| *h == sha)
        .map_or(State::Untested, |(n, _)| State::Known(n));
    Ok(Outcome {
        state,
        bytes: b,
        log: vec![format!(
            "patched segment {seg} offset {off:#x} (file {at:#x})"
        )],
    })
}
