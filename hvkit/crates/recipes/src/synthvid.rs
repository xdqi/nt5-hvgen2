//! VMBusVideoM.sys and VMBusVideoD.dll of the Hyper-V Integration Services 6.3.9600.16384 (the
//! SynthVid miniport and its display driver), x86 and x64: 32 bpp, and 56 modes up to 4096x2160
//! instead of six at 16 bpp. Port of `vid32/patch.py --table` in the CSMWrap testbed, and of
//! `vid32/patch64.py` for the x64 files.
//!
//! The host (SynthVid 3.3) takes 32 bpp and any mode that fits the VRAM; the 16 bpp limit and the
//! six modes come only from the guest drivers. One recipe patches both files, which have to be
//! installed together; the patched files no longer match the IC catalog's signature.
//!
//! - VMBusVideoD.dll: the dead 24 bpp branch of the mode filter, of GDIINFO and of DEVINFO becomes a
//!   32 bpp branch (5 bytes). The 16 bpp branch stays.
//! - VMBusVideoM.sys: the mode table moves into a new section `.modes`. When the section table
//!   leaves no room for its header (x86), the NT headers move over the Rich header, to 0x80. The
//!   section must not be discardable: MmFreeDriverInitialization stops merging discardable sections
//!   when it reaches it, and frees only whole pages. Seven code sites that hard-code the 6-entry
//!   table get its new size, count, address and last index. The miniport still disables the modes
//!   that need more than the VRAM, and it has to: its synchronous VMBus send waits forever when the
//!   host rejects a mode.
//!
//! The x64 files are the same source built for AMD64 and get the same changes, at other places:
//! the display driver's mode filter tests `(bpp - 16) & ~8` (16 or 24), which becomes `& ~16`; the
//! miniport copies the table from a RIP-relative address (no base relocation), and three of its
//! loops count modes instead of bytes. Its section header fits after the section table.
//!
//! The modes: 1024x768x32 first (the display driver falls back to the first mode when the
//! registry's doesn't match), the other resolutions at 32 bpp, then all of them at 16 bpp; at most
//! 64, the size of the display driver's static copy. The inputs are identified by their SHA-256.
//! Sites are given by RVA; in the x86 files that is the file offset too (section and file
//! alignment 0x80).

use crate::{Outcome, Result, State, expect_bytes, sha256_hex};
use formats::pe::{self, Pe};
use formats::{align_up, bail, put_u16, put_u32, u32_at};

pub const SUMMARY: &str =
    "VMBusVideoM.sys + VMBusVideoD.dll (IC 6.3.9600.16384, x86/x64): SynthVid at 32 bpp, 56 modes";

/// One build of VMBusVideoD.dll: what it is, and (RVA, original instructions, new instructions,
/// what) of each change.
struct Display {
    name: &'static str,
    sha: &'static str,
    patched_sha: &'static str,
    patches: &'static [(u32, &'static [u8], &'static [u8], &'static str)],
}

const DISPLAYS: [Display; 2] = [
    Display {
        name: "IC 6.3.9600.16384 VMBusVideoD.dll",
        sha: "A635674F74081A202F36FCC4131E9B0F8CC11918A8F01F0926FE0DBF73A81098",
        patched_sha: "085E2ED70317D070CB400FAD32CB8005CC696F5075CA4083584DB46103009BE6",
        patches: &[
            (
                0x59c,
                &[0x83, 0xf9, 0x18],
                &[0x83, 0xf9, 0x20],
                "mode filter: accept 32 bpp instead of 24",
            ),
            (
                0x88a,
                &[0x83, 0xf8, 0x18],
                &[0x83, 0xf8, 0x20],
                "GDIINFO: bpp == 32",
            ),
            (
                0x88f,
                &[0xc7, 0x83, 0xf0, 0, 0, 0, 0x06, 0, 0, 0],
                &[0xc7, 0x83, 0xf0, 0, 0, 0, 0x07, 0, 0, 0],
                "GDIINFO: ulHTOutputFormat = HT_FORMAT_32BPP",
            ),
            (
                0x8e0,
                &[0x83, 0xf9, 0x18],
                &[0x83, 0xf9, 0x20],
                "DEVINFO: bpp == 32",
            ),
            (
                0x8e5,
                &[0x6a, 0x05],
                &[0x6a, 0x06],
                "DEVINFO: surface format BMF_32BPP",
            ),
        ],
    },
    Display {
        name: "IC 6.3.9600.16384 x64 VMBusVideoD.dll",
        sha: "144E2BC194EAF648AE6DEF5AAC0EE8A22DD924BE6516C8C49C3D957FE6EB9F1A",
        patched_sha: "AB9BB13D3C7B8337914ECEAD67CF32180B6D105A49948EA5DD8B1A705FC87AA2",
        patches: &[
            (
                0x1181,
                &[0x83, 0xe9, 0x10, 0xf7, 0xc1, 0xf7, 0xff, 0xff, 0xff],
                &[0x83, 0xe9, 0x10, 0xf7, 0xc1, 0xef, 0xff, 0xff, 0xff],
                "mode filter: (bpp - 16) & ~16, 16 or 32 bpp instead of 16 or 24",
            ),
            (
                0x1605,
                &[0x83, 0xf8, 0x18],
                &[0x83, 0xf8, 0x20],
                "GDIINFO: bpp == 32",
            ),
            (
                0x160a,
                &[0xc7, 0x83, 0xf0, 0, 0, 0, 0x06, 0, 0, 0],
                &[0xc7, 0x83, 0xf0, 0, 0, 0, 0x07, 0, 0, 0],
                "GDIINFO: ulHTOutputFormat = HT_FORMAT_32BPP",
            ),
            (
                0x16fa,
                &[0x83, 0xf8, 0x18],
                &[0x83, 0xf8, 0x20],
                "DEVINFO: bpp == 32",
            ),
            (
                0x16ff,
                &[0xb8, 0x05, 0, 0, 0],
                &[0xb8, 0x06, 0, 0, 0],
                "DEVINFO: surface format BMF_32BPP",
            ),
        ],
    },
];

/// The VMware vmwgfx builtin list (Linux v6.6) plus QXL's 1366x768, 1600x900, 2048x1536 and
/// 4096x2160 (qemu hw/display/qxl.c).
const RESOLUTIONS: [(u32, u32); 28] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1152, 864),
    (1280, 720),
    (1280, 768),
    (1280, 800),
    (1280, 960),
    (1280, 1024),
    (1360, 768),
    (1366, 768),
    (1400, 1050),
    (1440, 900),
    (1600, 900),
    (1600, 1200),
    (1680, 1050),
    (1792, 1344),
    (1856, 1392),
    (1920, 1080),
    (1920, 1200),
    (1920, 1440),
    (2048, 1536),
    (2560, 1440),
    (2560, 1600),
    (2880, 1800),
    (3840, 2160),
    (3840, 2400),
    (4096, 2160),
];
const FALLBACK: (u32, u32) = (1024, 768);
/// sizeof(VIDEO_MODE_INFORMATION)
const MODE_SIZE: u32 = 0x50;
/// VMBusVideoD.dll copies the miniport's modes into a static array of this many.
const DISPLAY_MAX_MODES: usize = 64;
/// Where the NT headers move to, over the Rich header.
const NEW_NT: usize = 0x80;
const IMAGE_DIRECTORY_ENTRY_BOUND_IMPORT: usize = 11;

#[derive(Clone, Copy)]
enum Operand {
    /// The table's size in bytes (dword)
    Size,
    /// The number of modes (dword)
    Count,
    /// The number of modes (byte, sign-extended: at most 0x7F)
    Count8,
    /// The table's virtual address (dword, PE32)
    Va,
    /// The table's address relative to the end of the instruction (dword, RIP-relative)
    Rel,
    /// The highest mode index (byte)
    Last,
}

/// One build of VMBusVideoM.sys: what it is, and the sites that hard-code the 6-entry table:
/// (RVA, original instruction, operand offset in it, new operand, what).
struct Miniport {
    name: &'static str,
    sha: &'static str,
    patched_sha: &'static str,
    sites: &'static [(u32, &'static [u8], usize, Operand, &'static str)],
}

const MINIPORTS: [Miniport; 2] = [
    Miniport {
        name: "IC 6.3.9600.16384 VMBusVideoM.sys",
        sha: "A1143AC4E8D75C7F958F7ADCA69800C7C9134B36807A5188D752272BE79FD810",
        patched_sha: "1ACAB6A9699CD72733D764293FD6FCAE2DFEF8432AC12B943BA56D1C941545C9",
        sites: &[
            (
                0x1204,
                &[0xbb, 0xe0, 0x01, 0, 0],
                1,
                Operand::Size,
                "Initialize: VideoPortAllocateBuffer and copy length",
            ),
            (
                0x121f,
                &[0x68, 0x88, 0x0b, 0x01, 0],
                1,
                Operand::Va,
                "Initialize: copy source (base-relocated)",
            ),
            (
                0x1277,
                &[0x81, 0xfb, 0xe0, 0x01, 0, 0],
                2,
                Operand::Size,
                "Initialize: loop bound of the VRAM filter",
            ),
            (
                0x16bd,
                &[0x81, 0x7f, 0x14, 0xe0, 0x01, 0, 0],
                3,
                Operand::Size,
                "QueryAvailableModes: output buffer size check",
            ),
            (
                0x1710,
                &[0x81, 0xfa, 0xe0, 0x01, 0, 0],
                2,
                Operand::Size,
                "QueryAvailableModes: loop bound",
            ),
            (
                0x1805,
                &[0x81, 0xf9, 0xe0, 0x01, 0, 0],
                2,
                Operand::Size,
                "QueryNumAvailableModes: loop bound",
            ),
            (
                0x1880,
                &[0x83, 0xf8, 0x06],
                2,
                Operand::Last,
                "SetCurrentMode: highest valid index (6 was one past the table)",
            ),
        ],
    },
    Miniport {
        name: "IC 6.3.9600.16384 x64 VMBusVideoM.sys",
        sha: "31CB890D248FB86DFECBEF5D03810750034510EDC1F60B7BB1970375FAAD2006",
        patched_sha: "3C0DE3250634863A175E5B22F50DFDDBFCA609F0FA65B888539E3AD0DA8F6D0B",
        sites: &[
            (
                0x548b,
                &[0xbb, 0xe0, 0x01, 0, 0],
                1,
                Operand::Size,
                "Initialize: VideoPortAllocateBuffer and copy length",
            ),
            (
                0x54a9,
                &[0x48, 0x8d, 0x15, 0x60, 0xdb, 0xff, 0xff],
                3,
                Operand::Rel,
                "Initialize: copy source (lea rdx, [rip+...])",
            ),
            (
                0x5500,
                &[0x83, 0xff, 0x06],
                2,
                Operand::Count8,
                "Initialize: mode count of the VRAM filter",
            ),
            (
                0x5a27,
                &[0x81, 0x7e, 0x28, 0xe0, 0x01, 0, 0],
                3,
                Operand::Size,
                "QueryAvailableModes: output buffer size check",
            ),
            (
                0x5a55,
                &[0x41, 0xb8, 0x06, 0, 0, 0],
                2,
                Operand::Count,
                "QueryAvailableModes: mode count",
            ),
            (
                0x5ccf,
                &[0xba, 0x06, 0, 0, 0],
                1,
                Operand::Count,
                "QueryNumAvailableModes: mode count",
            ),
            (
                0x5d7d,
                &[0x83, 0xff, 0x06],
                2,
                Operand::Last,
                "SetCurrentMode: highest valid index (6 was one past the table)",
            ),
        ],
    },
];

pub fn apply(input: &[u8]) -> Result<Outcome> {
    let sha = sha256_hex(input);
    if let Some(m) = MINIPORTS.iter().find(|m| m.sha == sha) {
        return miniport(input, m);
    }
    if let Some(d) = DISPLAYS.iter().find(|d| d.sha == sha) {
        return display(input, d);
    }
    if MINIPORTS.iter().any(|m| m.patched_sha == sha)
        || DISPLAYS.iter().any(|d| d.patched_sha == sha)
    {
        return Ok(Outcome {
            state: State::Patched,
            bytes: input.to_vec(),
            log: vec!["already patched".into()],
        });
    }
    let known: Vec<String> = MINIPORTS
        .iter()
        .map(|m| (m.name, m.sha))
        .chain(DISPLAYS.iter().map(|d| (d.name, d.sha)))
        .map(|(name, sha)| format!("{name} ({sha})"))
        .collect();
    bail!(
        "input SHA-256 {sha} is none of {}. Refusing to patch.",
        known.join(", ")
    )
}

fn display(input: &[u8], build: &Display) -> Result<Outcome> {
    let mut d = input.to_vec();
    let pe = Pe::parse(&d, 0)?;
    for &(rva, old, new, what) in build.patches {
        let at = pe.rva_to_offset(rva)?;
        expect_bytes(&d, at, old, what)?;
        d[at..at + new.len()].copy_from_slice(new);
    }
    pe::update_checksum(&mut d, &pe);
    Ok(Outcome {
        state: State::Known(build.name),
        log: vec![
            format!(
                "32 bpp instead of 24 in the mode filter, GDIINFO and DEVINFO ({} sites), checksum 0x{:X}",
                build.patches.len(),
                u32_at(&d, pe.checksum_offset())?
            ),
            "install with the patched VMBusVideoM.sys as system32\\VMBusVideoD.dll".into(),
        ],
        bytes: d,
    })
}

/// (width, height, bpp) in table order.
fn modes() -> Vec<(u32, u32, u32)> {
    let mut m = vec![(FALLBACK.0, FALLBACK.1, 32)];
    m.extend(
        RESOLUTIONS
            .iter()
            .filter(|&&r| r != FALLBACK)
            .map(|&(w, h)| (w, h, 32)),
    );
    m.extend(RESOLUTIONS.iter().map(|&(w, h)| (w, h, 16)));
    m
}

/// One VIDEO_MODE_INFORMATION.
fn mode_info(index: u32, w: u32, h: u32, bpp: u32) -> Vec<u8> {
    // 16 bpp as in the original table: no bit counts, 5:6:5 masks
    let (bits, masks) = if bpp == 32 {
        ([8, 8, 8], [0xff0000, 0x00ff00, 0x0000ff])
    } else {
        ([0, 0, 0], [0xf800, 0x07e0, 0x001f])
    };
    let fields = [
        MODE_SIZE,
        index,
        w,
        h,
        w * bpp / 8, // stride
        1,           // planes
        bpp,
        60, // Hz
        w / 2,
        h / 2, // mm
        bits[0],
        bits[1],
        bits[2],
        masks[0],
        masks[1],
        masks[2],
        0x23, // VIDEO_MODE_COLOR | VIDEO_MODE_GRAPHICS | VIDEO_MODE_NO_OFF_SCREEN
        w,
        h,
        1, // DriverSpecificAttributeFlags: enabled (Initialize clears it if over the VRAM)
    ];
    fields.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn miniport(input: &[u8], build: &Miniport) -> Result<Outcome> {
    let mut d = input.to_vec();
    let old = Pe::parse(&d, 0)?;

    // Room for one more section header. If the section table fills the headers, move the NT
    // headers (signature, file header, optional header, section table) over the Rich header.
    let moved = old.section_table_end() + 40 > old.size_of_headers as usize;
    if moved {
        let headers = old.section_table_end() - old.nt;
        if old.nt < NEW_NT || NEW_NT + headers + 40 > old.size_of_headers as usize {
            bail!("no room to move the NT headers to 0x{NEW_NT:X}");
        }
        if old.data_directory(&d, IMAGE_DIRECTORY_ENTRY_BOUND_IMPORT)? != (0, 0) {
            bail!("the image has bound imports, which would have to move too");
        }
        let blob = d[old.nt..old.section_table_end()].to_vec();
        d[NEW_NT..old.size_of_headers as usize].fill(0);
        d[NEW_NT..NEW_NT + blob.len()].copy_from_slice(&blob);
        put_u32(&mut d, 0x3c, NEW_NT as u32);
    }
    let pe = Pe::parse(&d, 0)?;
    let h = pe.section_table_end();
    if d[h..h + 40].iter().any(|&b| b != 0) {
        bail!("the space after the section table at 0x{h:X} is in use");
    }

    // The section goes at the end of the image and of the file.
    let modes = modes();
    let n = modes.len();
    if n > DISPLAY_MAX_MODES {
        bail!("{n} modes, the display driver takes at most {DISPLAY_MAX_MODES}");
    }
    let table: Vec<u8> = modes
        .iter()
        .enumerate()
        .flat_map(|(i, &(w, h, bpp))| mode_info(i as u32, w, h, bpp))
        .collect();
    let file_end = pe.sections.iter().map(|s| s.raw + s.raw_size).max();
    if file_end != Some(d.len() as u32) {
        bail!("the file does not end with the last section's data (an embedded signature?)");
    }
    let va = align_up(pe.size_of_image, pe.section_alignment);
    let raw = align_up(d.len() as u32, pe.file_alignment);
    let size = table.len() as u32;
    let raw_size = align_up(size, pe.file_alignment);
    d.resize(raw as usize, 0);
    d.extend_from_slice(&table);
    d.resize((raw + raw_size) as usize, 0);

    d[h..h + 6].copy_from_slice(b".modes");
    put_u32(&mut d, h + 8, size);
    put_u32(&mut d, h + 12, va);
    put_u32(&mut d, h + 16, raw_size);
    put_u32(&mut d, h + 20, raw);
    put_u32(&mut d, h + 36, 0x4800_0040); // initialized data, not paged, read
    put_u16(&mut d, pe.nt + 6, pe.number_of_sections + 1);
    put_u32(
        &mut d,
        pe.opt + 56,
        align_up(va + size, pe.section_alignment),
    ); // SizeOfImage
    let init = u32_at(&d, pe.opt + 8)?;
    put_u32(&mut d, pe.opt + 8, init + raw_size); // SizeOfInitializedData

    for &(rva, old, operand_at, operand, what) in build.sites {
        let at = pe.rva_to_offset(rva)?;
        expect_bytes(&d, at, old, what)?;
        let o = at + operand_at;
        match operand {
            Operand::Size => put_u32(&mut d, o, size),
            Operand::Count => put_u32(&mut d, o, n as u32),
            Operand::Count8 => d[o] = n as u8,
            Operand::Va => put_u32(&mut d, o, pe.image_base + va),
            Operand::Rel => put_u32(&mut d, o, va.wrapping_sub(rva + old.len() as u32)),
            Operand::Last => d[o] = (n - 1) as u8,
        }
    }
    pe::update_checksum(&mut d, &pe);

    let max = RESOLUTIONS
        .iter()
        .map(|&(w, h)| w * h * 4)
        .max()
        .unwrap_or(0);
    Ok(Outcome {
        state: State::Known(build.name),
        log: vec![
            format!(
                "{}{n} modes in section .modes at 0x{:X} (0x{size:X} bytes), {} code sites, checksum 0x{:X}",
                if moved {
                    format!("NT headers moved from 0x{:X} to 0x{NEW_NT:X}; ", old.nt)
                } else {
                    String::new()
                },
                pe.image_base_64(&d)? + u64::from(va),
                build.sites.len(),
                u32_at(&d, pe.checksum_offset())?
            ),
            format!(
                "32 bpp modes need up to {:.1} MiB of VRAM; the miniport disables the ones that don't fit",
                f64::from(max) / 1048576.0
            ),
            "install with the patched VMBusVideoD.dll as system32\\drivers\\VMBusVideoM.sys".into(),
        ],
        bytes: d,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_table() {
        let m = modes();
        assert_eq!(m.len(), 56);
        assert!(m.len() <= DISPLAY_MAX_MODES && m.len() < 0x80); // Count8, Last: one byte
        assert_eq!(m[0], (1024, 768, 32));
        assert_eq!(m.iter().filter(|x| x.2 == 32).count(), RESOLUTIONS.len());
        assert_eq!(mode_info(5, 1366, 768, 16).len(), MODE_SIZE as usize);
        // stride of 1366x768x16 is 2732, as the driver expects (no padding)
        assert_eq!(mode_info(0, 1366, 768, 16)[16..20], 2732u32.to_le_bytes());
    }

    #[test]
    fn unknown_files_are_refused() {
        assert!(apply(&[0u8; 1024]).is_err());
    }
}
