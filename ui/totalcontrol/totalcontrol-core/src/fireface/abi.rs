//! Userspace ABI for the kernel driver's `snd-hwdep` control node.
//!
//! This file mirrors `kernel/ffuc-uapi.h` and must be updated with it.
//! The kernel owns USB and PCM; this ABI carries raw vendor requests,
//! IF1 register words and raw meter/state/status events.  All parsing and
//! device policy lives in this crate.

use std::mem::size_of;

pub const ABI_VERSION: u32 = 7;
pub const HWDEP_MAX_DATA: usize = 64;
pub const HWDEP_MAX_WORDS: usize = 63;
pub const HWDEP_MAX_RATES: usize = 16;
pub const EVENT_MAX: usize = 704;

pub const MODEL_UNKNOWN: u32 = 0;
pub const MODEL_UC: u32 = 1;
pub const MODEL_UCX: u32 = 2;
pub const MODEL_BABYFACE_CLASSIC: u32 = 3;

pub const XPORT_INTERRUPT: u32 = 0;
pub const XPORT_ISO: u32 = 1;

pub const CAP_VENDOR: u32 = 1 << 0;
pub const CAP_WORDS: u32 = 1 << 1;
pub const CAP_METERS: u32 = 1 << 2;
pub const CAP_STATUS: u32 = 1 << 3;
pub const CAP_PCM: u32 = 1 << 4;

pub const EV_METER: u32 = 1;
pub const EV_STATE: u32 = 2;
pub const EV_STATUS: u32 = 3;
pub const EV_LATCH: u32 = 4;

/// Meter event flag: first heartbeat of a session whose phase is not anchored.
pub const EV_FLAG_PHASE_LOST: u32 = 1;
/// Meter event flag: payload is one complete heartbeat (RMS then peak).
pub const EV_FLAG_METER_PAIR: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HwdepInfo {
    pub abi: u32,
    pub model: u32,
    pub transport: u32,
    pub caps: u32,
    pub channels: u32,
    pub inputs: u32,
    pub outputs: u32,
    pub playback: u32,
    pub matrix_sources: u32,
    pub meter_bytes: u32,
    pub state_ep: u32,
    pub meter_ep: u32,
    pub status_ep: u32,
    pub rate: u32,
    pub firmware: u32,
    pub card: u32,
    pub pcm_device: u32,
    pub name: [u8; 32],
    pub serial: [u8; 32],
    pub reserved: [u32; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HwdepVendor {
    pub request: u8,
    pub direction: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
    pub flags: u16,
    pub data: [u8; HWDEP_MAX_DATA],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HwdepWord {
    pub addr: u16,
    pub value: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HwdepWords {
    pub count: u32,
    pub flags: u32,
    pub words: [HwdepWord; HWDEP_MAX_WORDS],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HwdepEventHdr {
    pub kind: u32,
    pub size: u32,
    pub seq: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HwdepCounters {
    pub out_urbs: u32,
    pub out_gaps: u32,
    pub out_shorts: u32,
    pub out_status_errors: u32,
    pub urb_errors: u32,
    pub recoveries: u32,
    pub events_dropped: u32,
    pub meter_urbs: u32,
    pub state_urbs: u32,
    pub status_urbs: u32,
    pub in_urbs: u32,
    pub in_gaps: u32,
    pub in_shorts: u32,
    pub in_status_errors: u32,
    pub out_underruns: u32,
    pub in_overruns: u32,
    pub out_capacity_err: u32,
    pub out_pkt_len_err: u32,
    pub out_pkt_status: u32,
    pub in_status_err: u32,
    pub in_short: u32,
    pub in_misaligned: u32,
    pub in_bad_length: u32,
    pub out_bad_length: u32,
    pub in_pos_gap: u32,
    pub in_frames_mismatch: u32,
    pub rate_mismatch: u32,
    pub out_fb_ahead: u32,
    pub out_fb_stale: u32,
    pub hw_ptr_mismatch: u32,
    pub delta_zero: u32,
    /// Engine-run epoch; increments on every engine start. Per-run detail
    /// counters reset with it; cumulative counters do not.
    pub run_seq: u32,
}

/// One supported rate and the ALSA geometry the driver accepts at it.
/// `period_floor` is one URB (the smallest period and, with disable-tsched,
/// the smallest graph quantum); `buffer_min` is the smallest ring. The driver
/// computes these with the same functions that refine `hw_params`, so it is
/// the single owner of the achievable envelope.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HwdepRateEnv {
    pub rate: u32,
    pub period_floor: u32,
    pub buffer_min: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HwdepEnvelope {
    pub count: u32,
    pub urb_frames: u32,
    pub nurbs: u32,
    pub iso: u32,
    pub rates: [HwdepRateEnv; HWDEP_MAX_RATES],
}

/// Linux `_IOC` encoding (the generic asm-generic layout used on x86_64,
/// arm64 and most modern architectures).
const fn ioc(dir: u32, ty: u8, nr: u8, size: usize) -> u64 {
    ((dir as u64) << 30)
        | ((ty as u64) << 8)
        | (nr as u64)
        | ((size as u64) << 16)
}

const IOC_READ: u32 = 2;
const IOC_WRITE: u32 = 1;

pub const IOCTL_INFO: u64 = ioc(IOC_READ, b'F', 0x10, size_of::<HwdepInfo>());
pub const IOCTL_VENDOR: u64 = ioc(IOC_READ | IOC_WRITE, b'F', 0x11, size_of::<HwdepVendor>());
pub const IOCTL_WRITE_WORDS: u64 = ioc(IOC_WRITE, b'F', 0x12, size_of::<HwdepWords>());
pub const IOCTL_COUNTERS: u64 = ioc(IOC_READ, b'F', 0x13, size_of::<HwdepCounters>());
pub const IOCTL_ENVELOPE: u64 = ioc(IOC_READ, b'F', 0x14, size_of::<HwdepEnvelope>());

impl HwdepInfo {
    pub fn name_str(&self) -> String {
        cstr(&self.name)
    }

    pub fn serial_str(&self) -> String {
        cstr(&self.serial)
    }

    /// Human-readable transport/personality identity reported by the driver.
    pub fn personality_label(&self) -> &'static str {
        match (self.model, self.transport) {
            (MODEL_UC, XPORT_INTERRUPT) => "UC PC (interrupt)",
            (MODEL_UC, XPORT_ISO) => "UC AP (ISO)",
            (MODEL_UCX, XPORT_ISO) => "UCX ISO",
            (MODEL_UCX, XPORT_INTERRUPT) => "UCX IF5 (interrupt)",
            (MODEL_BABYFACE_CLASSIC, XPORT_ISO) => "Babyface classic (ISO)",
            _ => "unknown",
        }
    }
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_sizes_match_the_c_header() {
        // ffuc_hwdep_info: 17*4 + 32 + 32 + 16*4 = 196
        assert_eq!(size_of::<HwdepInfo>(), 196);
        // ffuc_hwdep_vendor: 1 + 1 + 2 + 2 + 2 + 2 + 64 = 74, aligned to 2
        assert_eq!(size_of::<HwdepVendor>(), 74);
        assert_eq!(size_of::<HwdepWords>(), 4 + 4 + 63 * 4);
        // ffuc_hwdep_counters: 32 * 4 = 128 (31 counters + run_seq)
        assert_eq!(size_of::<HwdepCounters>(), 128);
        // ffuc_hwdep_rate_env: 3 * 4 = 12; envelope: 4 * 4 + 16 * 12 = 208
        assert_eq!(size_of::<HwdepRateEnv>(), 12);
        assert_eq!(size_of::<HwdepEnvelope>(), 208);
    }

    #[test]
    fn ioc_number_is_stable() {
        // _IOR('F', 0x10, 196) = 0x80c44610
        assert_eq!(IOCTL_INFO, 0x80c4_4610);
        // _IOR('F', 0x14, 208) = 0x80d04614
        assert_eq!(IOCTL_ENVELOPE, 0x80d0_4614);
    }
}
