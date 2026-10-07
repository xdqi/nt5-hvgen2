//! icsvc.dll of the Hyper-V Integration Services 6.3.9600.16384: make the Guest Service Interface
//! (`Copy-VMFile -FileSource Host`) work on Windows XP. Port of
//! nt5-hvgen2/migrate/IcSvcGuestInterface.ps1.
//!
//! For every file the host copies in, the service logs on NT AUTHORITY\SYSTEM (LogonUserExW, empty
//! password, LOGON32_LOGON_SERVICE) to impersonate it. That logon exists from Vista on; on XP it fails
//! with ERROR_LOGON_FAILURE. The service runs as LocalSystem, so the patch makes the call return a
//! token of its own identity instead (ImpersonateSelf, OpenThreadToken, RevertToSelf).
//!
//! The `call [__imp_LogonUserExW]` at RVA 0x32959 becomes a relative call to a stub in the unused
//! tail of .text (whose VirtualSize grows by the stub's length, within the section's last page). The
//! stub reaches its imports relative to its own position, so it needs no base relocations; the base
//! relocation of the old call operand is neutralised. The patch is recognised by the bytes it
//! changes, not by a hash of the whole file, so it combines with patches to other parts of icsvc.dll.
//! Like the script, it leaves the PE checksum as it is.

use crate::{Outcome, Result, State};
use formats::pe::{IMAGE_FILE_MACHINE_I386, Pe};
use formats::{bail, put_u16, put_u32, u32_at};

pub const SUMMARY: &str =
    "icsvc.dll (IC 6.3.9600.16384): Guest Service Interface (Copy-VMFile) on XP";

/// `call [__imp_LogonUserExW]`
const SITE: u32 = 0x32959;
const IMPERSONATE_SELF: u32 = 0x64070;
const OPEN_THREAD_TOKEN: u32 = 0x64078;
const REVERT_TO_SELF: u32 = 0x64068;
const LOGON_USER_EX_W: u32 = 0x6407c;
const SLOTS: [(&str, u32); 4] = [
    ("ImpersonateSelf", IMPERSONATE_SELF),
    ("OpenThreadToken", OPEN_THREAD_TOKEN),
    ("RevertToSelf", REVERT_TO_SELF),
    ("LogonUserExW", LOGON_USER_EX_W),
];

pub fn apply(input: &[u8]) -> Result<Outcome> {
    let mut b = input.to_vec();
    let pe = Pe::parse(&b, 0)?;
    if pe.machine(&b)? != IMAGE_FILE_MACHINE_I386 {
        bail!("not an x86 PE file");
    }
    let fv = pe.file_version_string(&b)?.unwrap_or_default();
    let version_ok = fv
        .strip_prefix("6.3.9600.16384")
        .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'));
    if !version_ok {
        bail!(
            "file version '{fv}', the patch is for 6.3.9600.16384 (Integration Services of Windows Server 2012 R2)"
        );
    }
    let Some(text) = pe.section(".text").cloned() else {
        bail!("no .text section")
    };

    // The import slots must be the functions the stub assumes.
    for (name, slot) in SLOTS {
        let thunk = u32_at(&b, pe.rva_to_offset(slot)?)?;
        match pe.thunk_name(&b, thunk)? {
            None => bail!("import slot of {name} is by ordinal"),
            Some(n) if n != name => bail!("import slot 0x{slot:x} is '{n}', expected '{name}'"),
            Some(_) => {}
        }
    }

    let so = pe.rva_to_offset(SITE)?;
    if b[so] == 0xe8 {
        return Ok(Outcome {
            state: State::Patched,
            bytes: b,
            log: vec!["Guest Service Interface patch is already applied".into()],
        });
    }
    let mut orig = vec![0xff, 0x15];
    orig.extend_from_slice(&(pe.image_base + LOGON_USER_EX_W).to_le_bytes());
    if b[so..so + 6] != orig[..] {
        bail!(
            "unexpected code at the LogonUserExW call site ({:02x?})",
            &b[so..so + 6]
        );
    }

    // The stub is stdcall with the 10 arguments of LogonUserExW (40 bytes); phToken is the 6th.
    let stub_rva = (text.rva + text.virtual_size + 15) & !15;
    let l1 = stub_rva + 6;
    let disp = |slot: u32| slot.wrapping_sub(l1).to_le_bytes();
    let mut code: Vec<u8> = Vec::new();
    code.push(0x53); //                                  push ebx
    code.extend_from_slice(&[0xe8, 0, 0, 0, 0]); //      call L1
    code.push(0x5b); //                                  L1: pop ebx
    code.extend_from_slice(&[0x6a, 0x02]); //            push SecurityImpersonation
    code.extend_from_slice(&[0xff, 0x93]); //            call [ebx+ImpersonateSelf]
    code.extend_from_slice(&disp(IMPERSONATE_SELF));
    code.extend_from_slice(&[0x85, 0xc0]); //            test eax,eax
    code.extend_from_slice(&[0x74, 0x00]); //            jz out
    let jz = code.len() - 1;
    code.extend_from_slice(&[0x8b, 0x4c, 0x24, 0x1c]); // mov ecx,[esp+1Ch]   ; phToken
    code.push(0x51); //                                  push ecx
    code.extend_from_slice(&[0x6a, 0x01]); //            push TRUE (OpenAsSelf)
    code.push(0x68); //                                  push TOKEN_ALL_ACCESS
    code.extend_from_slice(&0xF01FFu32.to_le_bytes());
    code.extend_from_slice(&[0x6a, 0xfe]); //            push -2 (current thread)
    code.extend_from_slice(&[0xff, 0x93]); //            call [ebx+OpenThreadToken]
    code.extend_from_slice(&disp(OPEN_THREAD_TOKEN));
    code.push(0x50); //                                  push eax
    code.extend_from_slice(&[0xff, 0x93]); //            call [ebx+RevertToSelf]
    code.extend_from_slice(&disp(REVERT_TO_SELF));
    code.push(0x58); //                                  pop eax
    code[jz] = (code.len() - jz - 1) as u8;
    code.push(0x5b); //                                  out: pop ebx
    code.extend_from_slice(&[0xc2, 0x28, 0x00]); //      ret 28h

    if stub_rva + code.len() as u32 - text.rva > text.raw_size {
        bail!("no room for the stub after .text");
    }
    let stub_off = pe.rva_to_offset(stub_rva)?;
    if b[stub_off..stub_off + code.len()].iter().any(|&x| x != 0) {
        bail!("the tail of .text is not empty");
    }
    b[stub_off..stub_off + code.len()].copy_from_slice(&code);
    b[so] = 0xe8;
    put_u32(&mut b, so + 1, stub_rva.wrapping_sub(SITE + 5));
    b[so + 5] = 0x90;
    put_u32(
        &mut b,
        text.header + 8,
        stub_rva + code.len() as u32 - text.rva,
    );

    // The base relocation of the old call operand must not change the new relative call.
    let relocs = pe.base_relocations_at(&b, SITE + 2)?;
    if relocs.is_empty() {
        bail!("no base relocation for the LogonUserExW call operand");
    }
    for q in relocs {
        put_u16(&mut b, q, 0);
    }

    let log = vec![format!(
        "Guest Service Interface patch applied (stub at RVA 0x{stub_rva:x}, {} bytes)",
        code.len()
    )];
    Ok(Outcome {
        state: State::Known("IC 6.3.9600.16384 icsvc.dll"),
        bytes: b,
        log,
    })
}
