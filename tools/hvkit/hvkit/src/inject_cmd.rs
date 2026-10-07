//! `hvkit inject`: the components of media::components put into an installed system offline (see
//! media::inject).

use clap::Args;
use media::components::Components;
use media::inject::Inject;
use std::path::PathBuf;

#[derive(Args)]
pub struct InjectArgs {
    /// The disk image (raw or VHDX; a differencing VHDX is written only itself), IMAGE:N for
    /// partition N; default: the FAT partition with \WINDOWS\system32\config\system. The VM must be
    /// off.
    image: String,
    /// mdlex.sys (Dynamic Memory) and vmbaud.inf, vmbaud.sys (--vmbaud)
    #[arg(long)]
    files: Option<PathBuf>,
    /// The Integration Services packages, for dmvsc\dmvsc.sys and dmvsc\dmvscres.dll (default: the
    /// volume's Program Files\Hyper-V Integration Services)
    #[arg(long)]
    ic: Option<PathBuf>,
    /// XP: leave out Dynamic Memory
    #[arg(long)]
    no_dynamic_memory: bool,
    /// XP: leave out the VSS service (production checkpoints)
    #[arg(long)]
    no_vss: bool,
    /// XP: leave out the Guest Service Interface (Copy-VMFile)
    #[arg(long)]
    no_gsi: bool,
    /// Leave SynthVid at 16 bpp and its six modes
    #[arg(long)]
    no_synthvid: bool,
    /// Add the vmbaud sound card (for when the host runs vmbaud-host.ps1)
    #[arg(long)]
    vmbaud: bool,
    /// Change hives whose logs were not written back (after an unclean shutdown)
    #[arg(long)]
    force: bool,
}

pub fn run(a: InjectArgs) -> Result<(), String> {
    let (image, partition) = match a.image.rsplit_once(':') {
        Some((p, n)) if !p.is_empty() && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            (PathBuf::from(p), n.parse().ok())
        }
        _ => (PathBuf::from(&a.image), None),
    };
    let c = Inject {
        image,
        partition,
        files: a.files,
        ic: a.ic,
        leave_out: Components {
            dynamic_memory: a.no_dynamic_memory,
            vss: a.no_vss,
            gsi: a.no_gsi,
            synthvid: a.no_synthvid,
            ..Default::default()
        },
        opt_in: Components {
            vmbaud: a.vmbaud,
            ..Default::default()
        },
        force: a.force,
    };
    media::inject::inject(&c, &mut |l| println!("{l}")).map_err(|e| e.to_string())
}
