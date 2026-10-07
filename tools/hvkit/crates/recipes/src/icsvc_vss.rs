//! icsvc.dll of the Hyper-V Integration Services 6.3.9600.16384: make the VSS (Backup) integration
//! service work on Windows XP, so that production checkpoints take the guest's VSS path. Port of
//! nt5-hvgen2/migrate/Patch-IcSvcVss.ps1.
//!
//! icsvc.dll already has an XP code path for VSS: its IVssBackupComponents adapter switches to XP's
//! vtable layout when GetVersionEx reports 5.1, and its vssapi.dll delay-load hook falls back from the
//! Vista exports to the 2003 ones XP has. What still stops it is all in code only the VSS service
//! reaches:
//!
//! - the OS gate (ICVssCheckOsVersionForHotBackup) accepts 5.2 and up only;
//! - XP's IVssBackupComponents::SetContext always returns E_NOTIMPL, which VssClientBase::Initialize
//!   takes as fatal; XP knows only the default context, so the XP branch skips the call;
//! - XP's IVssAsync::Wait has no timeout parameter, so the INFINITE pushed for it stays on the stack
//!   and is popped as the caller's saved EDI (a `this` of -1 a moment later);
//! - the VSS provider (CHyperVICProvider) answers only the 2003 SP1 IIDs of
//!   IVssSoftwareSnapshotProvider and IVssProviderCreateSnapshotSet, whose vtables differ from XP's,
//!   and its BeginPrepareSnapshot takes the extra argument of 2003 SP1, which XP does not pass.
//!
//! The result is meant to be a separate file, icsvcvss.dll, that only the vmicvss service loads: the
//! stub for XP's MakeSnapshotReadWrite overwrites the start of a KVP function, which the other
//! services keep running from the stock icsvc.dll. Like the script, the recipe takes only the stock
//! file, identified by its SHA-256 (an icsvc.dll with the Guest Service Interface patch is refused),
//! so the sites are fixed file offsets, all in .text (RVA = offset + 0xC00, image base 0x10000000).
//! The PE checksum is recomputed.

use crate::{Outcome, Result, State, expect_bytes, sha256_hex};
use formats::pe::{self, Pe};
use formats::{bail, u32_at};

pub const SUMMARY: &str =
    "icsvc.dll (IC 6.3.9600.16384): VSS (Backup) service on XP, as icsvcvss.dll";

/// IC 6.3.9600.16384 icsvc.dll (x86, 2013-08-21), the only supported input.
const STOCK_SHA: &str = "CEF218418F65513DDC91215D82ECAE6624A259013F4C84EA0229465266EB07AF";
/// What this recipe (and the script) makes of it.
const PATCHED_SHA: &str = "5561A98276578B674F5F2C3F23AB5A23FF892111FEAE0CE29C0EB1AB28426AB7";

/// (file offset, original bytes, new bytes, what), in the script's order. The script numbers 8
/// changes; the provider's IIDs and its SSP vtable take two sites each.
const SITES: &[(usize, &[u8], &[u8], &str)] = &[
    // 1. ICVssCheckOsVersionForHotBackup: `cmp dword [ebp-118h], 2` on the minor version becomes
    //    `cmp ..., 1`, so 5.1 passes; 5.0 is still rejected, 5.2 and 6.x behave as before.
    (0x362F8, &[0x02], &[0x01], "ICVssCheckOsVersionForHotBackup"),
    // 2. ICVssComponentAdapter::SetContext, the old-interface (XP) branch, taken only when
    //    gm_UseOldInterface is set: `mov eax, [ecx+4]; push eax; mov ecx, [eax];
    //    call [ecx+80h]; ret` becomes `pop eax; xor eax, eax; ret` (drop the context pushed before
    //    the branch, return S_OK) and NOPs.
    (
        0x3FF4D,
        &[
            0x8B, 0x41, 0x04, 0x50, 0x8B, 0x08, 0xFF, 0x91, 0x80, 0x00, 0x00, 0x00, 0xC3,
        ],
        &[
            0x58, 0x33, 0xC0, 0xC3, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90,
        ],
        "ICVssComponentAdapter::SetContext",
    ),
    // 3. VssClientBase::WaitAndCheckForAsyncOperation: `push edi; push -1; push esi;
    //    call [eax+10h]` was built for Wait(this, dwTimeout), ret 8, with the pushed EDI popped by
    //    the epilogue. XP SP3's Wait(this) is ret 4. `push -1` becomes NOPs: the stack balances, EDI
    //    is restored, and XP's Wait waits indefinitely anyway.
    (
        0x3ACEC,
        &[0x6A, 0xFF],
        &[0x90, 0x90],
        "WaitAndCheckForAsyncOperation Wait arity",
    ),
    // 4. CHyperVICProvider::_QueryInterface: the IIDs it answers become XP's, so that XP SP3's
    //    vssvc gets the provider instead of E_NOINTERFACE (event 12292, 0x8004230F).
    //    IVssSoftwareSnapshotProvider: {609E123E-2C5A-44D3-8F01-0B1D9A47D1FF} ->
    //    {FBE2D3E8-4C8C-4464-9FE9-C3B6AE023357}
    (
        0x658,
        &[
            0x3E, 0x12, 0x9E, 0x60, 0x5A, 0x2C, 0xD3, 0x44, 0x8F, 0x01, 0x0B, 0x1D, 0x9A, 0x47,
            0xD1, 0xFF,
        ],
        &[
            0xE8, 0xD3, 0xE2, 0xFB, 0x8C, 0x4C, 0x64, 0x44, 0x9F, 0xE9, 0xC3, 0xB6, 0xAE, 0x02,
            0x33, 0x57,
        ],
        "QueryInterface SSP IID (XP)",
    ),
    //    IVssProviderCreateSnapshotSet: {5F894E5B-1E39-4778-8E23-9ABAD9F0E08C} ->
    //    {C4226E73-2F8B-490D-84E0-F9486B4ED627}
    (
        0x648,
        &[
            0x5B, 0x4E, 0x89, 0x5F, 0x39, 0x1E, 0x78, 0x47, 0x8E, 0x23, 0x9A, 0xBA, 0xD9, 0xF0,
            0xE0, 0x8C,
        ],
        &[
            0x73, 0x6E, 0x22, 0xC4, 0x8B, 0x2F, 0x0D, 0x49, 0x84, 0xE0, 0xF9, 0x48, 0x6B, 0x4E,
            0xD6, 0x27,
        ],
        "QueryInterface CSS IID (XP)",
    ),
    // 5. IVssSoftwareSnapshotProvider vtable: XP has MakeSnapshotReadWrite at +28h and
    //    SetSnapshotProperty at +2Ch; 2003 SP1 dropped the former and moved the latter up.
    //    +28h: SetSnapshotProperty (0x100436EB) -> the MakeSnapshotReadWrite stub (0x1002A312)
    (
        0xB7CC,
        &[0xEB, 0x36, 0x04, 0x10],
        &[0x12, 0xA3, 0x02, 0x10],
        "SSP vtable MakeSnapshotReadWrite",
    ),
    //    +2Ch: RevertToSnapshot (0x10043045) -> SetSnapshotProperty (0x100436EB)
    (
        0xB7D0,
        &[0x45, 0x30, 0x04, 0x10],
        &[0xEB, 0x36, 0x04, 0x10],
        "SSP vtable SetSnapshotProperty",
    ),
    // 6. IVssProviderCreateSnapshotSet vtable: XP has AbortSnapshots at +1Ch, where 2003 SP1 put
    //    PreFinalCommitSnapshots (0x10043235) -> AbortSnapshots (0x10043528)
    (
        0xB728,
        &[0x35, 0x32, 0x04, 0x10],
        &[0x28, 0x35, 0x04, 0x10],
        "CSS vtable AbortSnapshots",
    ),
    // 7. CHyperVICProvider::BeginPrepareSnapshot: XP calls it without the 2003 SP1 LONG argument,
    //    which the body never reads; `ret 2Ch` becomes `ret 28h`.
    (
        0x423CF,
        &[0xC2, 0x2C, 0x00],
        &[0xC2, 0x28, 0x00],
        "BeginPrepareSnapshot ret 0x28",
    ),
    // 8. The MakeSnapshotReadWrite stub at RVA 0x2A312, over the prologue (`push 96Ch`) of a KVP
    //    function: `xor eax, eax; ret 14h`, S_OK for XP's call with this and a GUID.
    (
        0x29712,
        &[0x68, 0x6C, 0x09, 0x00, 0x00],
        &[0x33, 0xC0, 0xC2, 0x14, 0x00],
        "MakeSnapshotReadWrite stub",
    ),
];

pub fn apply(input: &[u8]) -> Result<Outcome> {
    let sha = sha256_hex(input);
    if sha == PATCHED_SHA {
        return Ok(Outcome {
            state: State::Patched,
            bytes: input.to_vec(),
            log: vec!["already patched".into()],
        });
    }
    if sha != STOCK_SHA {
        bail!(
            "input SHA-256 {sha} does not match the IC 6.3.9600.16384 icsvc.dll ({STOCK_SHA}). Refusing to patch."
        );
    }
    let mut d = input.to_vec();
    for &(at, old, new, what) in SITES {
        expect_bytes(&d, at, old, what)?;
        d[at..at + new.len()].copy_from_slice(new);
    }
    let pe = Pe::parse(&d, 0)?;
    pe::update_checksum(&mut d, &pe);
    Ok(Outcome {
        state: State::Known("IC 6.3.9600.16384 icsvc.dll"),
        log: vec![
            format!(
                "XP VSS patch applied ({} sites: OS gate, SetContext, Wait arity, provider IIDs and vtables, BeginPrepareSnapshot, MakeSnapshotReadWrite stub), checksum 0x{:X}",
                SITES.len(),
                u32_at(&d, pe.checksum_offset())?
            ),
            "install as system32\\icsvcvss.dll, the ServiceDll of vmicvss alone; the other services keep icsvc.dll".into(),
        ],
        bytes: d,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_keep_their_length() {
        for &(_, old, new, what) in SITES {
            assert_eq!(old.len(), new.len(), "{what}");
        }
    }

    #[test]
    fn unknown_files_are_refused() {
        assert!(apply(&[0u8; 1024]).is_err());
    }
}
