//! dmvsc.sys, the Dynamic Memory VSC of the Hyper-V Integration Services, x86 and x64: rebind the
//! ntoskrnl imports the target kernel cannot satisfy to the companion driver mdlex.sys (drivers/mdlex;
//! its x64 build mdlex64.sys is installed as mdlex.sys). Known builds:
//! IC 6.3.9600.16384 (the vmguest.iso one) and 6.3.9600.17903 (KB3063109), plus Windows 7 SP1's own
//! 6.1.7601.17514.
//!
//! - MmAllocatePagesForMdlEx (x86 only): XP does not export it, so dmvsc.sys would not load. mdlex
//!   implements it over XP's MmAllocatePagesForMdl.
//! - MmAddPhysicalMemory: XP exports it but cannot hot-add memory. dmvsc probes it with one present
//!   page and advertises hot-add only if that succeeds; without hot-add the host rejects the
//!   capabilities (0xC000A013) and does not balloon. mdlex lets the probe succeed and refuses real
//!   requests, so dmvsc stays balloon-only.
//!
//! XP Professional x64 has the Server 2003 x64 kernel (5.2), whose MmAddPhysicalMemory fails the
//! probe too (STATUS_NOT_SUPPORTED), but whose own MmAllocatePagesForMdlEx serves the x64 dmvsc.sys
//! as it is: before Windows 7 dmvsc passes it only MM_DONT_ZERO_ALLOCATION (the contiguity flags are
//! behind a version check), which 5.2's routine takes (it returns NULL for any flag but that one and
//! MM_ALLOCATE_FROM_LOCAL_NODE_ONLY). It is the routine dmvsc was built against for Server 2003 x64,
//! while mdlex's goes through MmAllocatePagesForMdl, which on 5.2 zeroes the pages dmvsc asked not
//! to have zeroed. So on x64 only MmAddPhysicalMemory is rebound. The probe, and the answer to a
//! real hot-add (STATUS_INVALID_PARAMETER_1 taken as "no pages added"), are the same in both builds.
//!
//! Only the import table changes, no code: a new section `.dmx` holds the original import
//! descriptors verbatim plus one mdlex.sys descriptor per rebound import, whose FirstThunk is the IAT
//! slot the code already calls. The rebound ntoskrnl INT entries point at an import that stays
//! (MmFreePagesFromMdl), so ntoskrnl's thunk still resolves; the mdlex descriptors, processed later,
//! overwrite those slots. Thunks are 4 bytes in the x86 file and 8 in the x64 one (PE32+). The
//! inputs are identified by their SHA-256, so the fixed geometry always fits.

use crate::{Outcome, Result, State, sha256_hex};
use formats::pe::{self, IMAGE_DIRECTORY_ENTRY_IMPORT, Pe};
use formats::{align_up, bail, put_u16, put_u32, u32_at};

pub const SUMMARY: &str = "dmvsc.sys (IC 6.3.9600.16384/17903 or Win7 6.1.7601, x86 or x64): rebind hot-add probe imports to mdlex.sys";

/// A dmvsc.sys this recipe knows.
struct Build {
    /// What it is, for the log.
    name: &'static str,
    stock_sha: &'static str,
    /// What this recipe makes of it.
    patched_sha: &'static str,
    /// Import descriptors of the stock file, ntoskrnl.exe's first.
    descriptors: usize,
    /// The ntoskrnl imports rebound to mdlex.sys.
    redirects: &'static [&'static str],
}

/// The only supported inputs (SHA-256). A build whose target kernel exports MmAllocatePagesForMdlEx
/// keeps its own: the Windows 7 and later dmvsc pass it the contiguity flags, which mdlex (built for
/// XP) drops. Only the hot-add probe goes to mdlex everywhere, because no kernel this works on lets a
/// one-page MmAddPhysicalMemory succeed on its own.
const BUILDS: [Build; 4] = [
    Build {
        name: "IC 6.3.9600.16384 dmvsc.sys",
        stock_sha: "E23B6657E1126603D195145BED77AA239625057A28378AF535E5A3A7A4D1F36D",
        patched_sha: "AD7E7E24F9C4861A21C1C9F3D7846BB819505A57462C8AC5EE09603A5CD5D05B",
        descriptors: 4,
        redirects: &["MmAllocatePagesForMdlEx", "MmAddPhysicalMemory"],
    },
    Build {
        name: "IC 6.3.9600.16384 dmvsc.sys (x64)",
        stock_sha: "0DD2A97F5E1B38D1F7C0D44E50F09EA222B18B3B074CC9C8CD25A7526CB1A112",
        patched_sha: "43D5B2AC6AEB1E22BB474ACD3FA8494308684EEFFFCB09C43B16B572C89C08EF",
        descriptors: 3,
        redirects: &["MmAddPhysicalMemory"],
    },
    Build {
        name: "IC 6.3.9600.17903 dmvsc.sys (KB3063109)",
        stock_sha: "D351F0539D4D4CFDD7A98DB5B2BE02BC5827C75B55A52495B7E5AA06B2C284E2",
        patched_sha: "77041ACA593C7B185541B19238D0E2B40A4523A00D9CA899F549BF13300981D9",
        descriptors: 4,
        redirects: &["MmAddPhysicalMemory"],
    },
    Build {
        name: "Windows 7 SP1 dmvsc.sys 6.1.7601",
        stock_sha: "C83511685EE1CE85A5ADF9B5BE96C375A521601F66024BDC3EE044C0B6E85D69",
        patched_sha: "AB99BFE565AC317FB43645FFFD65F3A16A4C525B043D1156E17C6684922A54FD",
        descriptors: 3,
        redirects: &["MmAddPhysicalMemory"],
    },
];
/// An ntoskrnl import that stays, used as the placeholder of the rebound INT entries.
const PLACEHOLDER: &str = "MmFreePagesFromMdl";
const HELPER: &str = "mdlex.sys";

pub fn apply(input: &[u8]) -> Result<Outcome> {
    let sha = sha256_hex(input);
    if BUILDS.iter().any(|b| b.patched_sha == sha) {
        return Ok(Outcome {
            state: State::Patched,
            bytes: input.to_vec(),
            log: vec!["already patched".into()],
        });
    }
    let Some(build) = BUILDS.iter().find(|b| b.stock_sha == sha) else {
        let known = BUILDS
            .iter()
            .map(|b| format!("{} ({})", b.name, b.stock_sha))
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "input SHA-256 {sha} is none of the known dmvsc.sys builds: {known}. Refusing to patch."
        );
    };
    rebind(input, build)
}

fn rebind(input: &[u8], build: &Build) -> Result<Outcome> {
    let redirects = build.redirects;
    let mut d = input.to_vec();
    let pe = Pe::parse(&d, 0)?;
    // A PE32+ thunk that imports by name has its IMAGE_IMPORT_BY_NAME's RVA in the low dword and
    // zero in the high one; one that imports by ordinal has bit 63 set.
    let thunk = if pe.pe32_plus { 8 } else { 4 };

    let descs = pe.imports(&d)?;
    if descs.len() != build.descriptors {
        bail!(
            "expected {} import descriptors, found {}",
            build.descriptors,
            descs.len()
        );
    }
    // Descriptor 0 is ntoskrnl.exe: for every rebound import its INT entry and IAT slot, and the
    // placeholder's IMAGE_IMPORT_BY_NAME.
    let nt = &descs[0];
    if !nt.dll.starts_with("ntoskrnl") {
        bail!("descriptor 0 is not ntoskrnl");
    }
    let int_off = pe.rva_to_offset(nt.original_first_thunk)?;
    let mut redir: Vec<Option<(usize, u32)>> = vec![None; redirects.len()]; // (INT entry offset, IAT slot RVA)
    let mut placeholder = 0;
    for j in 0.. {
        let at = int_off + thunk * j;
        let t = u32_at(&d, at)?;
        let high = if pe.pe32_plus { u32_at(&d, at + 4)? } else { 0 };
        if t == 0 && high == 0 {
            break;
        }
        if high != 0 {
            continue; // by ordinal
        }
        if let Some(name) = pe.thunk_name(&d, t)? {
            if let Some(k) = redirects.iter().position(|r| *r == name) {
                redir[k] = Some((at, nt.first_thunk + (thunk * j) as u32));
            }
            if name == PLACEHOLDER {
                placeholder = t;
            }
        }
    }
    let redir: Vec<(usize, u32)> = redir
        .iter()
        .zip(redirects)
        .map(|(r, name)| {
            r.ok_or_else(|| formats::Error(format!("ntoskrnl import {name} not found")))
        })
        .collect::<Result<_>>()?;
    if placeholder == 0 {
        bail!("placeholder import {PLACEHOLDER} not found");
    }

    // The new section: descriptors (the original ones, one per rebound import, terminator), then
    // per rebound import an INT (two thunks, starting thunk-aligned), then the IMAGE_IMPORT_BY_NAMEs
    // (padded to even), then the module name.
    let end = pe
        .sections
        .iter()
        .map(|s| s.rva + s.virtual_size)
        .max()
        .unwrap_or(0);
    let new_va = align_up(end, pe.section_alignment);
    let new_raw = align_up(d.len() as u32, pe.file_alignment);
    let n_desc = descs.len() + redirects.len() + 1;
    let ints_rva = align_up(new_va + 20 * n_desc as u32, thunk as u32);
    let int_size = 2 * thunk as u32;
    let int_rva: Vec<u32> = (0..redirects.len() as u32)
        .map(|k| ints_rva + int_size * k)
        .collect();
    let mut cursor = ints_rva + int_size * redirects.len() as u32;
    let byname_rva: Vec<u32> = redirects
        .iter()
        .map(|r| {
            let at = cursor;
            cursor += (2 + r.len() as u32 + 1).next_multiple_of(2);
            at
        })
        .collect();
    let dll_name_rva = cursor;

    let mut blob = Vec::new();
    for desc in &descs {
        blob.extend_from_slice(&d[desc.offset..desc.offset + 20]);
    }
    for (k, (_, slot)) in redir.iter().enumerate() {
        for v in [int_rva[k], 0, 0, dll_name_rva, *slot] {
            blob.extend_from_slice(&v.to_le_bytes());
        }
    }
    blob.extend_from_slice(&[0; 20]);
    blob.resize((ints_rva - new_va) as usize, 0);
    for &by in &byname_rva {
        blob.extend_from_slice(&by.to_le_bytes());
        blob.resize(blob.len() + 2 * thunk - 4, 0);
    }
    for (k, r) in redirects.iter().enumerate() {
        blob.resize((byname_rva[k] - new_va) as usize, 0);
        blob.extend_from_slice(&0u16.to_le_bytes()); // hint
        blob.extend_from_slice(r.as_bytes());
        blob.push(0);
    }
    blob.resize((dll_name_rva - new_va) as usize, 0);
    blob.extend_from_slice(HELPER.as_bytes());
    blob.push(0);
    let vs_new = blob.len() as u32;
    let raw_new = align_up(vs_new, pe.file_alignment);

    d.resize(new_raw as usize, 0);
    d.extend_from_slice(&blob);
    d.resize((new_raw + raw_new) as usize, 0);

    // (An INT entry by name has a zero high dword in PE32+, so the low one is all that changes.)
    for (entry, _) in &redir {
        put_u32(&mut d, *entry, placeholder);
    }

    let hdr = pe.section_table_end();
    if hdr + 40 > pe.size_of_headers as usize {
        bail!("no room in the header for a new section");
    }
    d[hdr..hdr + 40].fill(0);
    d[hdr..hdr + 4].copy_from_slice(b".dmx");
    put_u32(&mut d, hdr + 8, vs_new);
    put_u32(&mut d, hdr + 12, new_va);
    put_u32(&mut d, hdr + 16, raw_new);
    put_u32(&mut d, hdr + 20, new_raw);
    put_u32(&mut d, hdr + 36, 0x4000_0040); // CNT_INITIALIZED_DATA | MEM_READ
    put_u16(&mut d, pe.nt + 6, pe.number_of_sections + 1);
    put_u32(
        &mut d,
        pe.opt + 56,
        align_up(new_va + vs_new, pe.section_alignment),
    ); // SizeOfImage
    let dir = pe.data_directory_offset(IMAGE_DIRECTORY_ENTRY_IMPORT);
    put_u32(&mut d, dir, new_va);
    put_u32(&mut d, dir + 4, 20 * n_desc as u32);
    pe::update_checksum(&mut d, &pe);

    let log = vec![
        format!(
            "added section .dmx at RVA 0x{new_va:X}, {} import descriptors, checksum 0x{:X}",
            n_desc - 1,
            u32_at(&d, pe.checksum_offset())?
        ),
        format!(
            "rebound to {HELPER}: {}; place both in system32\\drivers",
            redirects.join(", ")
        ),
    ];
    Ok(Outcome {
        state: State::Known(build.name),
        bytes: d,
        log,
    })
}
