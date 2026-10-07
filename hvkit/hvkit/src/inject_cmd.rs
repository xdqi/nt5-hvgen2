//! `hvkit inject` and `hvkit migrate`: an installed system changed offline (see media::inject and
//! media::migrate).

use clap::Args;
use media::components::Components;
use media::inject::Inject;
use media::migrate::Migrate;
use std::path::PathBuf;

/// The components both commands install.
#[derive(Args)]
pub struct ComponentArgs {
    /// Leave out Dynamic Memory
    #[arg(long)]
    no_dynamic_memory: bool,
    /// XP: leave out the VSS service (production checkpoints)
    #[arg(long)]
    no_vss: bool,
    /// Leave out the Guest Service Interface (Copy-VMFile)
    #[arg(long)]
    no_gsi: bool,
    /// Add the vmbaud sound card (for when the host runs vmbaud-host.ps1)
    #[arg(long)]
    vmbaud: bool,
}

impl ComponentArgs {
    /// (leave out, opt in)
    fn split(&self) -> (Components, Components) {
        (
            Components {
                dynamic_memory: self.no_dynamic_memory,
                vss: self.no_vss,
                gsi: self.no_gsi,
                ..Default::default()
            },
            Components {
                vmbaud: self.vmbaud,
                ..Default::default()
            },
        )
    }
}

#[derive(Args)]
pub struct InjectArgs {
    /// The disk image (raw or VHDX; a differencing VHDX is written only itself), IMAGE:N for
    /// partition N; default: the FAT partition with \WINDOWS\system32\config\system. The VM must be
    /// off.
    image: String,
    /// Where mdlex.sys (Dynamic Memory), vmbaud.inf and vmbaud.sys (--vmbaud) and, for an old
    /// bootwait.sys installed, bootwait.sys are (repeatable: the first that has a file wins)
    #[arg(long)]
    files: Vec<PathBuf>,
    /// The Integration Services packages, for dmvsc\dmvsc.sys and dmvsc\dmvscres.dll (default:
    /// --files, then the volume's Program Files\Hyper-V Integration Services)
    #[arg(long)]
    ic: Option<PathBuf>,
    #[command(flatten)]
    components: ComponentArgs,
    /// Leave SynthVid at 16 bpp and its six modes
    #[arg(long)]
    no_synthvid: bool,
    /// Change hives whose logs were not written back (after an unclean shutdown)
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
pub struct MigrateArgs {
    /// A copy of the Generation 1 VM's disk (raw or VHDX), IMAGE:N for partition N; default: the
    /// FAT partition with \WINDOWS\system32\config\system. Without IMAGE, --check checks the files
    /// alone.
    #[arg(required_unless_present = "check")]
    image: Option<String>,
    /// Where the files are (repeatable: the first that has a file wins): csmwrap.efi dsdt.aml
    /// hvfb.sys bootwait.sys bootvid.dll storport.sys diskdump.sys, mdlex.sys (Dynamic Memory),
    /// vmbaud.inf vmbaud.sys (--vmbaud); storvsc.sys dmvsc.sys dmvscres.dll unless from the
    /// Integration Services on the volume
    #[arg(long)]
    files: Vec<PathBuf>,
    /// CSMWrap (default: csmwrap.efi from --files)
    #[arg(long)]
    efi: Option<PathBuf>,
    /// The Integration Services packages, for storvsc\storvsc.sys, dmvsc\dmvsc.sys and
    /// dmvsc\dmvscres.dll (default: --files, then the volume's Program Files\Hyper-V Integration
    /// Services)
    #[arg(long)]
    ic: Option<PathBuf>,
    #[command(flatten)]
    components: ComponentArgs,
    /// CSMWrap's log on COM1, a default boot.ini entry with the kernel debugger on COM2, bug
    /// checks kept on the screen
    #[arg(long)]
    debug: bool,
    /// Leave Memory Management's DisablePagingExecutive as it is (default: 1, which keeps XP's
    /// Msfs.sys from a bug check on the first boot; drivers under test may want paging)
    #[arg(long)]
    keep_paging_executive: bool,
    /// Accept storvsc.sys, storport.sys, diskdump.sys and dmvscres.dll of other versions than the
    /// tested ones, and hives whose logs were not written back
    #[arg(long)]
    force: bool,
    /// Check the files (and the system on IMAGE), write nothing
    #[arg(long)]
    check: bool,
}

/// Splits IMAGE:N.
fn image_part(s: &str) -> (PathBuf, Option<usize>) {
    match s.rsplit_once(':') {
        Some((p, n)) if !p.is_empty() && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            (PathBuf::from(p), n.parse().ok())
        }
        _ => (PathBuf::from(s), None),
    }
}

pub fn run(a: InjectArgs) -> Result<(), String> {
    let (image, partition) = image_part(&a.image);
    let (mut leave_out, opt_in) = a.components.split();
    leave_out.synthvid = a.no_synthvid;
    let c = Inject {
        image,
        partition,
        files: a.files,
        ic: a.ic,
        leave_out,
        opt_in,
        force: a.force,
    };
    media::inject::inject(&c, &mut |l| println!("{l}")).map_err(|e| e.to_string())
}

pub fn run_migrate(a: MigrateArgs) -> Result<(), String> {
    let (image, partition) = match a.image.as_deref().map(image_part) {
        Some((i, p)) => (Some(i), p),
        None => (None, None),
    };
    let (leave_out, opt_in) = a.components.split();
    let c = Migrate {
        image,
        partition,
        files: a.files,
        efi: a.efi,
        ic: a.ic,
        leave_out,
        opt_in,
        debug: a.debug,
        keep_paging_executive: a.keep_paging_executive,
        force: a.force,
        check: a.check,
    };
    media::migrate::migrate(&c, &mut |l| println!("{l}")).map_err(|e| e.to_string())
}
