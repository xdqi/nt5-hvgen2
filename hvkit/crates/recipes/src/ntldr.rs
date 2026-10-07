//! NTLDR and SETUPLDR.BIN of Windows XP and Server 2003: make the highlighted boot.ini / F8 menu entry
//! and text-mode setup's status bar visible when VGA mode 12h is a single bit plane in RAM (Hyper-V
//! Generation 2 through CSMWrap, SeaVGABIOS's planar approximation). Port of
//! nt5-hvgen2/migrate/Patch-Ntldr.ps1, which explains the mechanism in full.
//!
//! In short: with BOOTFONT.BIN present (Chinese, Japanese, Korean versions) the loaders draw in mode
//! 12h and let the VGA latches and set/reset produce the colours, so in a plain bit plane reverse
//! video (attribute 0x70) looks like normal text. For attributes with a light background the patch
//! hands TextGrSetCurrentAttribute the attribute with foreground and background swapped and inverts
//! every byte stored while it is current (glyph bits, fill bytes, cleared screen). On real VGA that
//! draws the same pixels; in the single plane the highlight becomes a white bar.
//!
//! The image (osloader) sits behind a 16-bit startup module. The five routines to hook are found by
//! their code, not by address, so the same recipe covers NTLDR and SETUPLDR.BIN of XP SP3 and Server
//! 2003 SP2 in any language. 111 bytes of new code go into the zero padding at the end of the first
//! executable section with room, whose VirtualSize is raised to its raw size (the loader copies only
//! min(VirtualSize, SizeOfRawData)). The PE checksum of the image is updated.

use crate::{Outcome, Result, State, expect_bytes, sha256_hex};
use formats::pattern::Pattern;
use formats::pe::{self, IMAGE_SCN_MEM_EXECUTE, Pe};
use formats::{bail, put_u32, u32_at};

pub const SUMMARY: &str =
    "NTLDR / SETUPLDR.BIN (XP, 2003): visible menu highlight in single-plane mode 12h";

// The new code. Its address-dependent operands are zero here and filled in per file (FIXUPS);
// CURATTR, GLYPHROWS, SETATTR, GLYPH and CURSOR stand for the attribute byte set last, the number of
// rows of a BOOTFONT.BIN glyph, the instructions after the prologues of TextGrSetCurrentAttribute and
// of the glyph writer, and the routine that ends the glyph writer. The glyph writer is stdcall
// (bits, width 8 or 16, top fill, bottom fill).
#[rustfmt::skip]
const CAVE: [u8; 111] = [
    // Drawn inverted: attributes with a light background (bit 6 of the background; in practice only
    // 0x70, reverse video).
    // jmp from TextGrSetCurrentAttribute: the VGA latches get the foreground colour
    0xF6,0x44,0x24,0x04,0x40,       // 00  test byte [esp+4], 0x40
    0x74,0x05,                      // 05  jz .1
    0xC0,0x44,0x24,0x04,0x04,       // 07  rol byte [esp+4], 4
    0x55,                           // 0C  .1: push ebp
    0x89,0xE5,                      // 0D  mov ebp, esp
    0xE9,0x00,0x00,0x00,0x00,       // 0F  jmp SETATTR
    // jmp from the glyph writer's entry
    0x55,                           // 14  push ebp
    0x89,0xE5,                      // 15  mov ebp, esp
    0xE8,0x0F,0x00,0x00,0x00,       // 17  call flip
    0xE9,0x00,0x00,0x00,0x00,       // 1C  jmp GLYPH
    // call from the glyph writer's last call: puts the font back
    0xE8,0x05,0x00,0x00,0x00,       // 21  call flip
    0xE9,0x00,0x00,0x00,0x00,       // 26  jmp CURSOR
    // flip: invert the fill bytes and the glyph's bits (in the font) of the glyph writer whose
    // frame is ebp, if the current attribute is drawn inverted.
    0xF6,0x05,0x00,0x00,0x00,0x00,0x40, // 2B  test byte [CURATTR], 0x40
    0x74,0x22,                      // 32  jz .r
    0xF7,0x55,0x10,                 // 34  not dword [ebp+0x10]
    0xF7,0x55,0x14,                 // 37  not dword [ebp+0x14]
    0x8B,0x55,0x08,                 // 3A  mov edx, [ebp+8]
    0x85,0xD2,                      // 3D  test edx, edx
    0x74,0x15,                      // 3F  jz .r
    0x8B,0x0D,0x00,0x00,0x00,0x00,  // 41  mov ecx, [GLYPHROWS]
    0xE3,0x0D,                      // 47  jecxz .r
    0x83,0x7D,0x0C,0x10,            // 49  cmp dword [ebp+0xc], 16
    0x75,0x02,                      // 4D  jne .a
    0x01,0xC9,                      // 4F  add ecx, ecx
    0xF6,0x12,                      // 51  .a: not byte [edx]
    0x42,                           // 53  inc edx
    0xE2,0xFB,                      // 54  loop .a
    0xC3,                           // 56  .r: ret
    // calls from the clear screen and the clear to end of screen routines, in place of
    // mov ecx,0x2580; xor eax,eax; mov edi,0xa0000  and  lea edi,[eax+0xa0000]; xor eax,eax
    // Out: edi, eax = what to store (0, or all ones for an inverted attribute)
    0xB9,0x80,0x25,0x00,0x00,       // 57  mov ecx, 0x2580
    0x31,0xC0,                      // 5C  xor eax, eax
    0x8D,0xB8,0x00,0x00,0x0A,0x00,  // 5E  lea edi, [eax+0xa0000]
    0xA0,0x00,0x00,0x00,0x00,       // 64  mov al, [CURATTR]
    0xC0,0xE0,0x02,                 // 69  shl al, 2
    0x19,0xC0,                      // 6C  sbb eax, eax
    0xC3,                           // 6E  ret
];

#[derive(Clone, Copy)]
enum Sym {
    SetAttr,
    Glyph,
    Cursor,
    CurAttr,
    GlyphRows,
}

enum Kind {
    /// The 4 bytes are the address.
    Abs,
    /// A rel32 to the address.
    Rel,
}

const FIXUPS: [(usize, Kind, Sym); 6] = [
    (0x10, Kind::Rel, Sym::SetAttr),
    (0x1D, Kind::Rel, Sym::Glyph),
    (0x27, Kind::Rel, Sym::Cursor),
    (0x2D, Kind::Abs, Sym::CurAttr),
    (0x43, Kind::Abs, Sym::GlyphRows),
    (0x65, Kind::Abs, Sym::CurAttr),
];

#[derive(Clone, Copy)]
enum Site {
    SetAttr,
    GlyphEntry,
    GlyphExit,
    Cls,
    Eos,
}

/// A jump (0xE9) or call (0xE8) into the cave at `to`, over the bytes `from` (whole instructions) at
/// the site, with `pad` more bytes replaced by NOPs.
struct Hook {
    name: &'static str,
    site: Site,
    op: u8,
    to: u32,
    pad: usize,
    from: &'static [u8],
}

const HOOKS: [Hook; 5] = [
    Hook {
        name: "TextGrSetCurrentAttribute",
        site: Site::SetAttr,
        op: 0xE9,
        to: 0x00,
        pad: 0,
        from: &[0x8B, 0xFF, 0x55, 0x8B, 0xEC],
    },
    Hook {
        name: "glyph writer, entry",
        site: Site::GlyphEntry,
        op: 0xE9,
        to: 0x14,
        pad: 0,
        from: &[0x8B, 0xFF, 0x55, 0x8B, 0xEC],
    },
    Hook {
        name: "glyph writer, last call",
        site: Site::GlyphExit,
        op: 0xE8,
        to: 0x21,
        pad: 0,
        from: &[0xE8],
    },
    Hook {
        name: "clear screen",
        site: Site::Cls,
        op: 0xE8,
        to: 0x57,
        pad: 7,
        from: &[
            0xB9, 0x80, 0x25, 0x00, 0x00, 0x33, 0xC0, 0xBF, 0x00, 0x00, 0x0A, 0x00,
        ],
    },
    Hook {
        name: "clear to end of screen",
        site: Site::Eos,
        op: 0xE8,
        to: 0x5E,
        pad: 3,
        from: &[0x8D, 0xB8, 0x00, 0x00, 0x0A, 0x00, 0x33, 0xC0],
    },
];

/// SHA-256 of the .text section of the files this was tested with.
const KNOWN: [(&str, &str); 4] = [
    (
        "NTLDR, XP SP3 (5.1.2600.5512)",
        "4D42E725B44BFAC18A7BD5FC38388838A5D05AD64EE6220197BAD10E7F0C8452",
    ),
    (
        "SETUPLDR.BIN, XP SP3 (5.1.2600.5512)",
        "7B8CA5ECE163DF0F27E3925DEDAD300B2522A8D07CD3E81A9EF60AA9D48FEE9C",
    ),
    (
        "NTLDR, Server 2003 SP2 (5.2.3790.3959)",
        "AED4089FA7CCBBC3DE3BB67C610CC030DAF163B59B23BB9E82E6840310B2FF52",
    ),
    (
        "SETUPLDR.BIN, Server 2003 SP2 (5.2.3790.3959)",
        "FC63713FAB8746590D168C582C0A856702EDD3BD65445D5C4D2E48DC72E204A3",
    ),
];

const NOT_A_LOADER: &str = "this does not look like an NTLDR or SETUPLDR.BIN with the 640x480x16 text routines. Nothing was changed.";

pub fn apply(input: &[u8]) -> Result<Outcome> {
    let mut b = input.to_vec();
    let mut log = Vec::new();

    // Patched before? The new code's first 15 bytes contain no fixup.
    if b.windows(15).any(|w| w == &CAVE[..15]) {
        log.push("already patched".into());
        return Ok(Outcome {
            state: State::Patched,
            bytes: b,
            log,
        });
    }

    let Some(start) = pe::find_embedded_image(&b) else {
        bail!("no PE image found in it; this is not an NTLDR or SETUPLDR.BIN. Nothing was changed.")
    };
    let pe = Pe::parse(&b, start)?;
    let text = match pe.section(".text") {
        Some(t) if start + (t.raw + t.raw_size) as usize <= b.len() => t.clone(),
        _ => bail!("the PE image has no usable .text section. Nothing was changed."),
    };
    let text_off = start + text.raw as usize;
    let text_bytes = &b[text_off..text_off + text.raw_size as usize];

    let text_sha = sha256_hex(text_bytes);
    let state = match KNOWN.iter().find(|(_, sha)| *sha == text_sha) {
        Some((name, _)) => {
            log.push(format!("this is the {name}"));
            State::Known(name)
        }
        None => {
            log.push("warning: not one of the files this was tested with; patching it anyway, at your own risk".into());
            State::Untested
        }
    };

    // The routines, by their code.
    let find = |hay: &[u8], pat: &str, what: &str| {
        Pattern::new(pat)
            .find_one(hay, what)
            .map_err(|e| formats::Error(format!("{e}; {NOT_A_LOADER}")))
    };
    let tv = pe.image_base + text.rva; // VA of .text
    let sa = find(
        text_bytes,
        "8b ff 55 8b ec 83 ec 0c c6 45 fb 00 c7 45 f4 00 96 0a 00 66 ba ce 03",
        "TextGrSetCurrentAttribute",
    )?;
    let gw = find(
        text_bytes,
        "8b ff 55 8b ec 8b 55 08 33 c9 3b d1 0f 84 ?? ?? ?? ?? 39 0d ?? ?? ?? ?? 56 57 76 ?? 8b 75 0c 8a 45 10 c1 ee 03 83 7d 0c 10",
        "the glyph writer",
    )?;
    let cls = find(
        text_bytes,
        "8b ff 57 b9 80 25 00 00 33 c0 bf 00 00 0a 00 f3 ab 5f c3",
        "the clear screen routine",
    )?;
    let eos = find(
        text_bytes,
        "8d b8 00 00 0a 00 33 c0 f3 ab 8b ca 83 e1 03 f3 aa 5f c3",
        "the clear to end of screen routine",
    )?;
    let dis = find(
        text_bytes,
        "83 3d ?? ?? ?? ?? 00 8b 45 08 a2 (?? ?? ?? ??) 50 74 07 e8 ?? ?? ?? ?? eb 05 e8 ?? ?? ?? ??",
        "the attribute dispatcher",
    )?;
    let gw_body = &text_bytes[gw.start..(gw.start + 0x110).min(text_bytes.len())];
    let rows = find(gw_body, "40 3b 05 (?? ?? ?? ??) 72", "the glyph row loop")?;
    let gx = find(
        gw_body,
        "e8 (?? ?? ?? ??) 5d c2 10 00",
        "the end of the glyph writer",
    )?;

    let at = |o: usize| u32_at(&b, text_off + o);
    let gx_va = tv + (gw.start + gx.start) as u32;
    let site_va = |s: Site| match s {
        Site::SetAttr => tv + sa.start as u32,
        Site::GlyphEntry => tv + gw.start as u32,
        Site::GlyphExit => gx_va,
        Site::Cls => tv + cls.start as u32 + 3,
        Site::Eos => tv + eos.start as u32,
    };
    let cur_attr = at(dis.captures[0])?;
    let glyph_rows = at(gw.start + rows.captures[0])?;
    let cursor = gx_va
        .wrapping_add(5)
        .wrapping_add(at(gw.start + gx.captures[0])?);
    let sym = |s: Sym| match s {
        Sym::CurAttr => cur_attr,
        Sym::GlyphRows => glyph_rows,
        Sym::SetAttr => tv + sa.start as u32 + 5,
        Sym::Glyph => tv + gw.start as u32 + 5,
        Sym::Cursor => cursor,
    };

    // Where the new code goes: the first executable section with room after its virtual size
    // (16-aligned), room to grow its virtual size to its raw size before the next section, and
    // zeros there.
    let mut cave = None;
    for s in &pe.sections {
        if s.characteristics & IMAGE_SCN_MEM_EXECUTE == 0 || s.raw == 0 {
            continue;
        }
        let first = (s.virtual_size as usize + 15) & !15;
        let file_off = start + s.raw as usize;
        if first + CAVE.len() > s.raw_size as usize || file_off + s.raw_size as usize > b.len() {
            continue;
        }
        let next = pe
            .sections
            .get(s.index + 1)
            .map_or(pe.size_of_image, |n| n.rva);
        if s.rva + s.raw_size > next {
            continue;
        }
        if b[file_off + first..file_off + first + CAVE.len()]
            .iter()
            .all(|&x| x == 0)
        {
            cave = Some((s.clone(), first));
            break;
        }
    }
    let Some((cs, first)) = cave else {
        bail!("no executable section has room for the new code. Nothing was changed.")
    };
    let cave_va = pe.image_base + cs.rva + first as u32;
    let cave_off = start + cs.raw as usize + first;
    log.push(format!(
        "new code: {} bytes at 0x{cave_va:X} in {}",
        CAVE.len(),
        cs.name
    ));

    b[cave_off..cave_off + CAVE.len()].copy_from_slice(&CAVE);
    for (at, kind, s) in FIXUPS {
        let v = match kind {
            Kind::Abs => sym(s),
            Kind::Rel => sym(s).wrapping_sub(cave_va + at as u32 + 4),
        };
        put_u32(&mut b, cave_off + at, v);
    }

    for h in &HOOKS {
        let va = site_va(h.site);
        let o = pe.va_to_offset(va)?;
        expect_bytes(&b, o, h.from, h.name)?;
        b[o] = h.op;
        put_u32(&mut b, o + 1, (cave_va + h.to).wrapping_sub(va + 5));
        b[o + 5..o + 5 + h.pad].fill(0x90);
        log.push(format!(
            "{}: patched {} byte(s) at 0x{o:X}",
            h.name,
            5 + h.pad
        ));
    }
    put_u32(&mut b, cs.header + 8, cs.raw_size); // VirtualSize

    // The image runs to the end of the file.
    pe::update_checksum(&mut b, &pe);
    Ok(Outcome {
        state,
        bytes: b,
        log,
    })
}
