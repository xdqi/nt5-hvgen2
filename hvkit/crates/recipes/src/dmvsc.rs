//! dmvsc.sys of the Hyper-V Integration Services 6.3.9600.16384 (the Dynamic Memory VSC, built for
//! Server 2003 SP1): rebind the two ntoskrnl imports Windows XP cannot satisfy to the companion
//! driver mdlex.sys. Port of nt5-hvgen2/migrate/Patch-Dmvsc.ps1.
//!
//! - MmAllocatePagesForMdlEx: XP does not export it, so dmvsc.sys would not load. mdlex implements it
//!   over XP's MmAllocatePagesForMdl.
//! - MmAddPhysicalMemory: XP exports it but cannot hot-add memory; dmvsc's probe of it would make it
//!   advertise hot-add, which the host rejects. mdlex refuses real requests, so dmvsc stays
//!   balloon-only.
//!
//! Only the import table changes, no code: a new section `.dmx` holds the four original import
//! descriptors verbatim plus one mdlex.sys descriptor per rebound import, whose FirstThunk is the IAT
//! slot the code already calls. The rebound ntoskrnl INT entries point at an import that stays
//! (MmFreePagesFromMdl), so ntoskrnl's thunk still resolves; the mdlex descriptors, processed later,
//! overwrite those slots. The input is identified by its SHA-256, so the fixed geometry always fits.

use crate::{Outcome, Result, State, sha256_hex};
use formats::pe::{self, IMAGE_DIRECTORY_ENTRY_IMPORT, Pe};
use formats::{align_up, bail, put_u16, put_u32, u32_at};

pub const SUMMARY: &str = "dmvsc.sys (IC 6.3.9600.16384): rebind XP-missing imports to mdlex.sys";

/// IC 6.3.9600.16384 dmvsc.sys (x86), the only supported input.
const STOCK_SHA: &str = "E23B6657E1126603D195145BED77AA239625057A28378AF535E5A3A7A4D1F36D";
/// What this recipe makes of it.
const PATCHED_SHA: &str = "AD7E7E24F9C4861A21C1C9F3D7846BB819505A57462C8AC5EE09603A5CD5D05B";
const REDIRECTS: [&str; 2] = ["MmAllocatePagesForMdlEx", "MmAddPhysicalMemory"];
/// An ntoskrnl import that stays, used as the placeholder of the rebound INT entries.
const PLACEHOLDER: &str = "MmFreePagesFromMdl";
const HELPER: &str = "mdlex.sys";

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
            "input SHA-256 {sha} does not match the IC 6.3.9600.16384 dmvsc.sys ({STOCK_SHA}). Refusing to patch."
        );
    }
    let mut d = input.to_vec();
    let pe = Pe::parse(&d, 0)?;

    let descs = pe.imports(&d)?;
    if descs.len() != 4 {
        bail!("expected 4 import descriptors, found {}", descs.len());
    }
    // Descriptor 0 is ntoskrnl.exe: for every rebound import its INT entry and IAT slot, and the
    // placeholder's IMAGE_IMPORT_BY_NAME.
    let nt = &descs[0];
    if !nt.dll.starts_with("ntoskrnl") {
        bail!("descriptor 0 is not ntoskrnl");
    }
    let int_off = pe.rva_to_offset(nt.original_first_thunk)?;
    let mut redir: Vec<Option<(usize, u32)>> = vec![None; REDIRECTS.len()]; // (INT entry offset, IAT slot RVA)
    let mut placeholder = 0;
    for j in 0.. {
        let t = u32_at(&d, int_off + 4 * j)?;
        if t == 0 {
            break;
        }
        if let Some(name) = pe.thunk_name(&d, t)? {
            if let Some(k) = REDIRECTS.iter().position(|r| *r == name) {
                redir[k] = Some((int_off + 4 * j, nt.first_thunk + 4 * j as u32));
            }
            if name == PLACEHOLDER {
                placeholder = t;
            }
        }
    }
    let redir: Vec<(usize, u32)> = redir
        .iter()
        .zip(REDIRECTS)
        .map(|(r, name)| {
            r.ok_or_else(|| formats::Error(format!("ntoskrnl import {name} not found")))
        })
        .collect::<Result<_>>()?;
    if placeholder == 0 {
        bail!("placeholder import {PLACEHOLDER} not found");
    }

    // The new section: descriptors (4 original, one per rebound import, terminator), then per rebound
    // import an INT (two dwords), then the IMAGE_IMPORT_BY_NAMEs (padded to even), then the module
    // name.
    let end = pe
        .sections
        .iter()
        .map(|s| s.rva + s.virtual_size)
        .max()
        .unwrap_or(0);
    let new_va = align_up(end, pe.section_alignment);
    let new_raw = align_up(d.len() as u32, pe.file_alignment);
    let n_desc = descs.len() + REDIRECTS.len() + 1;
    let mut cursor = new_va + 20 * n_desc as u32;
    let int_rva: Vec<u32> = (0..REDIRECTS.len() as u32)
        .map(|k| cursor + 8 * k)
        .collect();
    cursor += 8 * REDIRECTS.len() as u32;
    let byname_rva: Vec<u32> = REDIRECTS
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
    for &by in &byname_rva {
        blob.extend_from_slice(&by.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
    }
    for (k, r) in REDIRECTS.iter().enumerate() {
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
            REDIRECTS.join(", ")
        ),
    ];
    Ok(Outcome {
        state: State::Known("IC 6.3.9600.16384 dmvsc.sys"),
        bytes: d,
        log,
    })
}
