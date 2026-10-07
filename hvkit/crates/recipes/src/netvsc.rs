//! netvsc50.sys of the Hyper-V Integration Services 6.3.9600.16384: survive the hot removal of a
//! network adapter (NdisDevicePnPEventSurpriseRemoved).
//!
//! `MiniportPnPEventNotify` (RVA 0x380a) calls a host-supplied callback through
//! `call [adapter+0x57C]`.  That pointer is always NULL in this build: `DriverEntry` never fills
//! entry +0x24 of its callback table, and the framework copies the table to +0x57C of the adapter.
//! Generation 1 never hot-plugs a network adapter, so the path is dead there; on Generation 2 the
//! host can remove a NIC under a running guest and the call goes to address 0 (STOP 0x7E).
//!
//! The fix is to drop the two argument pushes and the call, keeping the adapter-state store between
//! them: 14 bytes of NOPs at three sites.  Nothing else in the routine changes.  The recipe is also
//! the data of bootwait's load-time patch table (see `media::bootwait_patch`), which is how a
//! converted disk keeps the fix after Plug and Play copies the stock driver over
//! `system32\drivers` again.
//!
//! The input is identified by its SHA-256 (the file recipe) and, for the load-time table, by the
//! TimeDateStamp and image size of the same file.

use crate::{Outcome, Result, State, expect_bytes, sha256_hex};

pub const SUMMARY: &str = "netvsc50.sys (IC 6.3.9600.16384): survive hot removal of a NIC";

/// IC 6.3.9600.16384 netvsc50.sys (x86), the only supported input.
const STOCK_SHA: &str = "A60FE7E324A2B5599C90B41975FAB35A844E034F60160008F1E6A4A6E478F5F3";
/// What this recipe makes of it.
const PATCHED_SHA: &str = "2F14E1DCC4B0E0B5CBE247C282E794FE15E1D499FED48557335D50DE55DA97B8";

/// `TimeDateStamp` of the PE header, for the load-time table (the file is not readable there).
pub const TIMESTAMP: u32 = 0x5215_8EB2;
/// Size of the file, for the same check.
pub const SIZE: u32 = 0x8C00;
/// File name the load-time table matches on (the driver PnP reinstalls).
pub const IMAGE: &str = "netvsc50.sys";

/// The sites, in address order: (RVA, original bytes, new bytes, what).  Every section of this
/// image has the same virtual and raw offset, so the file offset of a site equals its RVA.
pub const SITES: &[(usize, &[u8], &[u8], &str)] = &[
    (
        0x3819,
        &[0x6a, 0x01],
        &[0x90, 0x90],
        "surprise-remove: drop the first callback argument (push 1)",
    ),
    (
        0x381b,
        &[0xff, 0xb6, 0x80, 0x05, 0x00, 0x00],
        &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
        "surprise-remove: drop the second callback argument (push [esi+0x580])",
    ),
    (
        0x3828,
        &[0xff, 0x96, 0x7c, 0x05, 0x00, 0x00],
        &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
        "surprise-remove: drop the call of the never-filled callback (call [esi+0x57C])",
    ),
];
/// `mov [esi+0x5F4], 1` at 0x3821 sits between the sites and stays: it records the new state.
const KEPT: (usize, &[u8]) = (0x3821, &[0xc6, 0x86, 0xf4, 0x05, 0x00, 0x00, 0x01]);

pub fn apply(input: &[u8]) -> Result<Outcome> {
    let mut log = Vec::new();
    let sha = sha256_hex(input);
    let (state, bytes) = if sha == STOCK_SHA {
        log.push(format!("netvsc50.sys: dropping the surprise-remove callback call ({} NOP bytes)", SITES.iter().map(|s| s.1.len()).sum::<usize>()));
        let mut b = input.to_vec();
        expect_bytes(&b, KEPT.0, KEPT.1, "the adapter-state store")?;
        for &(at, old, new, what) in SITES {
            expect_bytes(&b, at, old, what)?;
            b[at..at + new.len()].copy_from_slice(new);
            log.push(format!("  0x{at:05X}: {what}"));
        }
        (State::Known("IC 6.3.9600.16384"), b)
    } else if sha == PATCHED_SHA {
        (State::Patched, input.to_vec())
    } else {
        return Err(formats::Error(format!(
            "netvsc50.sys: SHA-256 {sha} is neither the stock nor the patched file; refusing to patch."
        )));
    };
    Ok(Outcome { state, bytes, log })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_do_not_overlap_the_kept_store() {
        let mut spans: Vec<(usize, usize)> = SITES.iter().map(|s| (s.0, s.0 + s.1.len())).collect();
        spans.push((KEPT.0, KEPT.0 + KEPT.1.len()));
        spans.sort();
        for w in spans.windows(2) {
            assert!(w[0].1 <= w[1].0, "overlap at 0x{:X}", w[1].0);
        }
    }

    #[test]
    fn expected_and_written_lengths_match() {
        for &(_, old, new, _) in SITES {
            assert_eq!(old.len(), new.len());
            assert!(new.iter().all(|&b| b == 0x90), "only NOPs in this recipe");
        }
    }
}
