//! icsvc.dll of the Hyper-V Integration Services 6.3.9600.16384: make the Guest Service Interface
//! (`Copy-VMFile -FileSource Host`) work on Windows XP, Server 2003 and XP Professional x64.
//!
//! For every file the host copies in, the service logs on NT AUTHORITY\SYSTEM (LogonUserExW, empty
//! password, LOGON32_LOGON_SERVICE) to impersonate it. That logon exists from Vista on; on XP it fails
//! with ERROR_LOGON_FAILURE, which the host reports as 0x8007052E ("The user name or password is
//! incorrect"). The service runs as LocalSystem, so the patch makes the call return a
//! token of its own identity instead (ImpersonateSelf, OpenThreadToken, RevertToSelf).
//!
//! The x86 and the x64 file (the two packages of the same release) are told apart by the PE machine.
//! In both the call through `__imp_LogonUserExW` becomes a relative call to a stub in the unused tail
//! of .text (whose VirtualSize grows to take it in, within the section's file data), and the
//! stub reaches its imports relative to its own position, so it needs no base relocations. The patch
//! is recognised by the bytes it changes, not by a hash of the whole file, so it combines with
//! patches to other parts of icsvc.dll.
//!
//! x86: the call is at RVA 0x32959; the base relocation of its old operand is neutralised. The PE
//! checksum is left as it is.
//!
//! x64: the call is at RVA 0x3b9d1 and RIP-relative (no base relocation). It becomes `nop` + `call`,
//! so the return address stays where it was for the caller's unwind and C++ exception tables. The
//! stub has a frame (a non-leaf function), so it gets a RUNTIME_FUNCTION at the end of .pdata, still
//! in address order, and UNWIND_INFO after its code; without them an exception or a stack walk
//! through it would take its frame for a leaf's. The PE checksum is recomputed.

use crate::{Outcome, Result, State};
use formats::pe::{
    IMAGE_DIRECTORY_ENTRY_EXCEPTION, IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_I386, Pe,
};
use formats::{bail, put_u16, put_u32, u32_at};

pub const SUMMARY: &str =
    "icsvc.dll (IC 6.3.9600.16384, x86 or x64): Guest Service Interface (Copy-VMFile) on XP/2003";

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

/// The x64 file: `call [rip+__imp_LogonUserExW]` and the 8-byte import slots.
const SITE_64: u32 = 0x3b9d1;
const IMPERSONATE_SELF_64: u32 = 0x860e0;
const OPEN_THREAD_TOKEN_64: u32 = 0x860f0;
const REVERT_TO_SELF_64: u32 = 0x860d0;
const LOGON_USER_EX_W_64: u32 = 0x860f8;
const SLOTS_64: [(&str, u32); 4] = [
    ("ImpersonateSelf", IMPERSONATE_SELF_64),
    ("OpenThreadToken", OPEN_THREAD_TOKEN_64),
    ("RevertToSelf", REVERT_TO_SELF_64),
    ("LogonUserExW", LOGON_USER_EX_W_64),
];

pub fn apply(input: &[u8]) -> Result<Outcome> {
    let pe = Pe::parse(input, 0)?;
    let machine = pe.machine(input)?;
    if machine != IMAGE_FILE_MACHINE_I386 && machine != IMAGE_FILE_MACHINE_AMD64 {
        bail!("neither an x86 nor an x64 PE file");
    }
    let fv = pe.file_version_string(input)?.unwrap_or_default();
    let version_ok = fv
        .strip_prefix("6.3.9600.16384")
        .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'));
    if !version_ok {
        bail!(
            "file version '{fv}', the patch is for 6.3.9600.16384 (Integration Services of Windows Server 2012 R2)"
        );
    }
    if machine == IMAGE_FILE_MACHINE_AMD64 {
        apply_x64(input, pe)
    } else {
        apply_x86(input, pe)
    }
}

/// Checks that the import slots are the functions the stub assumes (8 bytes each in a PE32+).
fn check_slots(pe: &Pe, b: &[u8], slots: &[(&str, u32)]) -> Result<()> {
    for &(name, slot) in slots {
        let o = pe.rva_to_offset(slot)?;
        let thunk = u32_at(b, o)?;
        if pe.pe32_plus && u32_at(b, o + 4)? != 0 {
            bail!("import slot of {name} is by ordinal");
        }
        match pe.thunk_name(b, thunk)? {
            None => bail!("import slot of {name} is by ordinal"),
            Some(n) if n != name => bail!("import slot 0x{slot:x} is '{n}', expected '{name}'"),
            Some(_) => {}
        }
    }
    Ok(())
}

fn already_patched(b: Vec<u8>) -> Result<Outcome> {
    Ok(Outcome {
        state: State::Patched,
        bytes: b,
        log: vec!["Guest Service Interface patch is already applied".into()],
    })
}

fn apply_x86(input: &[u8], pe: Pe) -> Result<Outcome> {
    let mut b = input.to_vec();
    let Some(text) = pe.section(".text").cloned() else {
        bail!("no .text section")
    };
    check_slots(&pe, &b, &SLOTS)?;

    let so = pe.rva_to_offset(SITE)?;
    if b[so] == 0xe8 {
        return already_patched(b);
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

fn apply_x64(input: &[u8], pe: Pe) -> Result<Outcome> {
    let mut b = input.to_vec();
    let (Some(text), Some(pdata)) = (pe.section(".text").cloned(), pe.section(".pdata").cloned())
    else {
        bail!("no .text or .pdata section")
    };
    check_slots(&pe, &b, &SLOTS_64)?;

    let so = pe.rva_to_offset(SITE_64)?;
    if b[so..so + 2] == [0x90, 0xe8] {
        return already_patched(b);
    }
    let mut orig = vec![0xff, 0x15];
    orig.extend_from_slice(&LOGON_USER_EX_W_64.wrapping_sub(SITE_64 + 6).to_le_bytes());
    if b[so..so + 6] != orig[..] {
        bail!(
            "unexpected code at the LogonUserExW call site ({:02x?})",
            &b[so..so + 6]
        );
    }
    // (No 8-byte base relocation may overlap the 6 bytes either.)
    for r in SITE_64 - 7..SITE_64 + 6 {
        if !pe.base_relocations_at(&b, r)?.is_empty() {
            bail!("a base relocation applies to the LogonUserExW call");
        }
    }

    // Microsoft x64 convention with the 10 arguments of LogonUserExW: phToken, the 6th, is at
    // [rsp+30h] on entry, [rsp+58h] after the prologue. 28h bytes of frame: the callees' 20h of home
    // space and 8 at [rsp+20h] for OpenThreadToken's result, and rsp 16-byte aligned at the calls.
    // Only volatile registers change, as in any call.
    let stub_rva = (text.rva + text.virtual_size + 15) & !15;
    let mut code: Vec<u8> = Vec::new();
    let rip_rel = |code: &mut Vec<u8>, slot: u32| {
        let next = stub_rva + code.len() as u32 + 4;
        code.extend_from_slice(&slot.wrapping_sub(next).to_le_bytes());
    };
    code.extend_from_slice(&[0x48, 0x83, 0xec, 0x28]); //  sub rsp,28h
    let prolog = code.len() as u8;
    code.extend_from_slice(&[0xb9, 0x02, 0, 0, 0]); //     mov ecx,2 (SecurityImpersonation)
    code.extend_from_slice(&[0xff, 0x15]); //              call [rip+ImpersonateSelf]
    rip_rel(&mut code, IMPERSONATE_SELF_64);
    code.extend_from_slice(&[0x85, 0xc0]); //              test eax,eax
    code.extend_from_slice(&[0x74, 0x00]); //              jz out
    let jz = code.len() - 1;
    code.extend_from_slice(&[0x4c, 0x8b, 0x4c, 0x24, 0x58]); // mov r9,[rsp+58h]   ; phToken
    code.extend_from_slice(&[0x41, 0xb8, 0x01, 0, 0, 0]); // mov r8d,1 (OpenAsSelf)
    code.push(0xba); //                                    mov edx,TOKEN_ALL_ACCESS
    code.extend_from_slice(&0xF01FFu32.to_le_bytes());
    code.extend_from_slice(&[0x48, 0xc7, 0xc1, 0xfe, 0xff, 0xff, 0xff]); // mov rcx,-2 (current thread)
    code.extend_from_slice(&[0xff, 0x15]); //              call [rip+OpenThreadToken]
    rip_rel(&mut code, OPEN_THREAD_TOKEN_64);
    code.extend_from_slice(&[0x89, 0x44, 0x24, 0x20]); //  mov [rsp+20h],eax
    code.extend_from_slice(&[0xff, 0x15]); //              call [rip+RevertToSelf]
    rip_rel(&mut code, REVERT_TO_SELF_64);
    code.extend_from_slice(&[0x8b, 0x44, 0x24, 0x20]); //  mov eax,[rsp+20h]
    code[jz] = (code.len() - jz - 1) as u8;
    code.extend_from_slice(&[0x48, 0x83, 0xc4, 0x28]); //  out: add rsp,28h
    code.push(0xc3); //                                    ret
    let stub_end = stub_rva + code.len() as u32;

    // UNWIND_INFO: version 1, no handler, the prologue's one UWOP_ALLOC_SMALL of 28h (padded to an
    // even number of codes). The epilogue is the standard `add rsp; ret` the unwinder recognises.
    let unwind_rva = (stub_end + 3) & !3;
    let unwind = [1, prolog, 1, 0, prolog, 0x02 | ((0x28 / 8 - 1) << 4), 0, 0];
    let end = unwind_rva + unwind.len() as u32;
    if end - text.rva > text.raw_size {
        bail!("no room for the stub after .text");
    }
    let stub_off = pe.rva_to_offset(stub_rva)?;
    let tail = &mut b[stub_off..stub_off + (end - stub_rva) as usize];
    if tail.iter().any(|&x| x != 0) {
        bail!("the tail of .text is not empty");
    }
    tail[..code.len()].copy_from_slice(&code);
    tail[(unwind_rva - stub_rva) as usize..].copy_from_slice(&unwind);
    put_u32(&mut b, text.header + 8, end - text.rva);

    // The RUNTIME_FUNCTION goes after the last one, which must end before the stub (the table is
    // searched by address).
    let (dir_rva, dir_size) = pe.data_directory(&b, IMAGE_DIRECTORY_ENTRY_EXCEPTION)?;
    if dir_rva != pdata.rva || dir_size % 12 != 0 || dir_size < 12 {
        bail!("unexpected exception directory (RVA 0x{dir_rva:x}, {dir_size} bytes)");
    }
    if u32_at(&b, pe.rva_to_offset(dir_rva + dir_size - 8)?)? > stub_rva {
        bail!("a function after the end of .text has unwind data");
    }
    if dir_size + 12 > pdata.raw_size {
        bail!("no room for the stub's unwind data in .pdata");
    }
    let rf = pe.rva_to_offset(dir_rva + dir_size)?;
    if b[rf..rf + 12].iter().any(|&x| x != 0) {
        bail!("the tail of .pdata is not empty");
    }
    put_u32(&mut b, rf, stub_rva);
    put_u32(&mut b, rf + 4, stub_end);
    put_u32(&mut b, rf + 8, unwind_rva);
    put_u32(
        &mut b,
        pe.data_directory_offset(IMAGE_DIRECTORY_ENTRY_EXCEPTION) + 4,
        dir_size + 12,
    );
    put_u32(
        &mut b,
        pdata.header + 8,
        pdata.virtual_size.max(dir_size + 12),
    );

    b[so] = 0x90;
    b[so + 1] = 0xe8;
    put_u32(&mut b, so + 2, stub_rva.wrapping_sub(SITE_64 + 6));
    formats::pe::update_checksum(&mut b, &pe);

    let log = vec![format!(
        "Guest Service Interface patch applied (x64 stub at RVA 0x{stub_rva:x}, {} bytes, with unwind data)",
        code.len()
    )];
    Ok(Outcome {
        state: State::Known("IC 6.3.9600.16384 icsvc.dll x64"),
        bytes: b,
        log,
    })
}
