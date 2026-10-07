//! `hvkit vxd`: Windows 9x VxDs (LE files).

use clap::Subcommand;
use formats::le::{self, Le};
use recipes::vxd_portio;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum VxdCommand {
    /// Dump the header, objects, entries, DDB and fixup summary (FILE or FILE@0xOFFSET for an LE
    /// inside a bigger file, e.g. VMM in a W3 VMM32.VXD)
    Info { vxd: String },
    /// List the port I/O sites of the code objects (to the PIC, PIT, port 61h, i8042)
    Scan {
        #[arg(required = true)]
        vxds: Vec<String>,
    },
    /// Disassemble part of an object, fixups marked
    Show {
        vxd: String,
        object: usize,
        /// First offset (hex)
        from: String,
        /// Last offset (hex)
        to: String,
    },
    /// Redirect VPICD/VTD/VKD port I/O to the GEN2LEG shim's vectors
    PatchIo {
        input: String,
        output: PathBuf,
        /// The shim's vectors.txt (20 vectors, in the shim's operation order)
        #[arg(long)]
        vectors: PathBuf,
    },
    /// Make the DDB export of a VxD linked by Open Watcom's wlink a type 3 entry (in place)
    FixEntry { vxd: PathBuf },
}

/// FILE or FILE@0xOFFSET.
fn open(spec: &str) -> Result<(Vec<u8>, Option<usize>, PathBuf), String> {
    let (path, off) = match spec.rsplit_once('@') {
        Some((p, o)) if o.starts_with("0x") => (
            p,
            Some(usize::from_str_radix(&o[2..], 16).map_err(|_| format!("bad offset {o}"))?),
        ),
        _ => (spec, None),
    };
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    Ok((data, off, PathBuf::from(path)))
}

fn parse(spec: &str) -> Result<(Vec<u8>, Le), String> {
    let (b, off, _) = open(spec)?;
    let le = Le::parse(&b, off).map_err(|e| format!("{spec}: {e}"))?;
    Ok((b, le))
}

fn e(spec: &str) -> impl Fn(formats::Error) -> String + '_ {
    move |err| format!("{spec}: {err}")
}

pub fn run(cmd: VxdCommand) -> Result<(), String> {
    match cmd {
        VxdCommand::Info { vxd } => {
            let (b, le) = parse(&vxd)?;
            let f = |o| le.field(&b, o).map_err(e(&vxd));
            println!(
                "header @{:#x}  pages {} (page {}, last {})  module flags {:#x}",
                le.header,
                le.page_count,
                le.page_size,
                le.last_page,
                f(0x10)?
            );
            for o in &le.objects {
                println!(
                    "  object {} vsize {:#x} base {:#x} flags {:#06x} pages {}+{}",
                    o.number, o.virtual_size, o.base, o.flags, o.first_page, o.pages
                );
            }
            println!("  names {:?}", le.resident_names(&b).map_err(e(&vxd))?);
            for en in le.entries(&b).map_err(e(&vxd))? {
                println!(
                    "  entry {} object {} offset {:#x} flags {:#x}",
                    en.ordinal, en.object, en.offset, en.flags
                );
            }
            let d = le.ddb(&b).map_err(e(&vxd))?;
            println!(
                "  DDB {} id {:#x} v{}.{} init order {:#x} control {:#x} sdk {:#x} services {}",
                d.name,
                d.device_id,
                d.version.0,
                d.version.1,
                d.init_order,
                d.control_proc,
                d.sdk_version,
                d.service_count
            );
            let fx = le.fixups(&b).map_err(e(&vxd))?;
            let mut types: Vec<u8> = fx.iter().map(|f| f.source).collect();
            types.sort();
            types.dedup();
            println!("  fixups {} source types {types:?}", fx.len());
            Ok(())
        }
        VxdCommand::Scan { vxds } => {
            for v in &vxds {
                let (b, le) = parse(v)?;
                let sites = vxd_portio::scan(&b, &le).map_err(e(v))?;
                println!("== {v}: {} sites", sites.len());
                for s in sites {
                    println!(
                        "  obj {} ({}-bit) +{:05x}  {} {}{}  {}{}",
                        s.object,
                        s.bits,
                        s.offset,
                        if s.out { "out" } else { "in " },
                        s.port.map_or("??".into(), |p| format!("{p:02x}h")),
                        if s.dx { " dx" } else { "   " },
                        s.text,
                        if s.overlaps_fixup {
                            "   ; OVERLAPS FIXUP"
                        } else {
                            ""
                        }
                    );
                }
            }
            Ok(())
        }
        VxdCommand::Show {
            vxd,
            object,
            from,
            to,
        } => {
            let (b, le) = parse(&vxd)?;
            let hex = |s: &str| {
                u32::from_str_radix(s.trim_start_matches("0x"), 16)
                    .map_err(|_| format!("bad offset {s}"))
            };
            let (lo, hi) = (hex(&from)?, hex(&to)?);
            let o = le
                .objects
                .get(object.wrapping_sub(1))
                .ok_or_else(|| format!("{vxd}: no object {object}"))?;
            let fx: Vec<u32> = le
                .fixups(&b)
                .map_err(e(&vxd))?
                .into_iter()
                .filter(|f| f.object == object)
                .map(|f| f.offset)
                .collect();
            let code = le.object_bytes(&b, object).map_err(e(&vxd))?;
            for i in vxd_portio::disassemble(&code, if o.is_32bit() { 32 } else { 16 }) {
                if (lo..=hi).contains(&i.offset) {
                    let mark = fx
                        .iter()
                        .any(|&x| i.offset <= x && x < i.offset + i.bytes.len() as u32);
                    let hexb: String = i.bytes.iter().map(|x| format!("{x:02x}")).collect();
                    println!(
                        "  {:05x}  {hexb:18} {}{}",
                        i.offset,
                        i.text,
                        if mark { "   <fixup>" } else { "" }
                    );
                }
            }
            Ok(())
        }
        VxdCommand::PatchIo {
            input,
            output,
            vectors,
        } => {
            let text = std::fs::read_to_string(&vectors)
                .map_err(|err| format!("{}: {err}", vectors.display()))?;
            let v = vxd_portio::parse_vectors(&text)
                .map_err(|err| format!("{}: {err}", vectors.display()))?;
            let (b, off, _) = open(&input)?;
            let out = vxd_portio::patch(&b, off, &v).map_err(e(&input))?;
            std::fs::write(&output, &out.bytes)
                .map_err(|err| format!("{}: {err}", output.display()))?;
            println!("{input} -> {}: {}", output.display(), out.log.join("; "));
            Ok(())
        }
        VxdCommand::FixEntry { vxd } => {
            let p: &Path = &vxd;
            let mut b = std::fs::read(p).map_err(|err| format!("{}: {err}", p.display()))?;
            if le::fix_ddb_entry(&mut b).map_err(|err| format!("{}: {err}", p.display()))? {
                std::fs::write(p, &b).map_err(|err| format!("{}: {err}", p.display()))?;
                println!("{}: entry 1 changed from type 2 to type 3", p.display());
            }
            Ok(())
        }
    }
}
