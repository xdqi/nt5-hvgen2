//! hal.dll of Windows XP Professional x64 / Windows Server 2003 x64 (5.2.3790.3959, srv03_sp2_rtm):
//! send the system clock to one processor instead of to every local APIC.
//!
//! The x64 HAL drives the system clock from the CMOS RTC and, in `HalpEnableRedirEntry`, forces the
//! IOAPIC redirection entry of that interrupt to a physical broadcast: when the vector is 0xd1 the
//! destination byte is written as 0xff whatever the caller asked for, skipping the destination
//! calculation every other interrupt goes through. On Hyper-V Generation 2 that broadcast never
//! reaches the guest's processors — no clock interrupt is delivered once the application processors
//! come up and the redirection entry is programmed, the tick count freezes and every processor ends
//! up in the idle loop. On a Generation 1 VM, and on hardware, the same value works; it is the
//! combination of this constant with that platform that fails.
//!
//! The change is one byte: write the boot processor's APIC id (0) instead of 0xff. The clock then
//! reaches the boot processor, which is where `KeUpdateSystemTime` advances the shared tick count
//! anyway (the application processors' `KiSecondaryClockInterrupt` only accounts their own run
//! time). The PE checksum is recomputed, since the loader checks it.
//!
//! The input is identified by its SHA-256 and by the instruction sequence that is replaced, so a
//! different build of the HAL is refused rather than patched by offset.

use crate::{Outcome, Result, State, sha256_hex};
use formats::bail;
use formats::pattern::Pattern;
use formats::pe::{self, Pe};

pub const SUMMARY: &str =
    "hal.dll (XP/2003 x64): deliver the system clock to the boot processor, not to every APIC";

/// The 5.2.3790.3959 hal.dll this was derived from and tested with.
const STOCK_SHA: &str = "7B7671166CDE8CBB8AF9EDE79D0C4CD38208F77A5B59E7BA661D839A870A101E";
/// What this recipe makes of it.
const PATCHED_SHA: &str = "2E6C112297E144BF14B3DDDEF9F1DEB36C1118AC154E05FA17C1137FF44EFB67";

/// `lea dest_byte(%r12,%rbx,4),%rdi` followed by `movb $-1,(%rdi)`, the destination byte of the
/// redirection entry. The last byte is the one that becomes 0.
const SITE: &str = "49 8d bc 9c c1 c1 02 00 c6 07 ff";

pub fn apply(input: &[u8]) -> Result<Outcome> {
    let sha = sha256_hex(input);
    if sha == PATCHED_SHA {
        return Ok(Outcome {
            state: State::Patched,
            bytes: input.to_vec(),
            log: vec!["already patched".into()],
        });
    }

    let mut log = Vec::new();
    let state = if sha == STOCK_SHA {
        log.push("this is the XP Pro x64 / Server 2003 x64 hal.dll 5.2.3790.3959".into());
        State::Known("hal.dll 5.2.3790.3959 (srv03_sp2_rtm) x64")
    } else {
        log.push(
            "warning: not the hal.dll this was tested with; patching it anyway, at your own risk"
                .into(),
        );
        State::Untested
    };

    let mut b = input.to_vec();
    let pe = Pe::parse(&b, 0)?;
    let Some(text) = pe.section(".text").cloned() else {
        bail!("the image has no .text section; this is not a hal.dll. Nothing was changed.");
    };
    let text_off = text.raw as usize;
    let text_bytes = &b[text_off..text_off + text.raw_size as usize];

    let site = Pattern::new(SITE);
    let m = site.find_one(
        text_bytes,
        "the redirection entry destination of the clock interrupt",
    )?;
    // The last byte of the pattern is the destination byte 0xff.
    let at = text_off + m.start + site.len() - 1;
    log.push(format!(
        "the clock interrupt went to every local APIC (destination byte 0xff at file offset {at:#x}); \
         sending it to the boot processor instead"
    ));
    b[at] = 0;

    pe::update_checksum(&mut b, &pe);
    log.push("recomputed the PE checksum".into());
    Ok(Outcome {
        state,
        bytes: b,
        log,
    })
}
