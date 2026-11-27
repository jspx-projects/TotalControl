//! Userspace ABI for the kernel driver's `snd-hwdep` control node.
//!
//! This file mirrors `kernel/ff-uapi.h` and must be updated with it.
//! The kernel owns USB and PCM; this ABI carries raw vendor requests,
//! register words, raw meter/state/status events, live stream snapshots and
//! diagnostics.  All device models, names and policy live in this crate.

use std::mem::size_of;

pub const ABI_VERSION: u32 = 9;
pub const HWDEP_MAX_DATA: usize = 64;
pub const HWDEP_MAX_WORDS: usize = 63;
pub const HWDEP_MAX_RATES: usize = 16;
pub const EVENT_MAX: usize = 704;

/* USB product ids are hardware facts.  The host maps them to a profile;
 * the kernel does not name a model. */
pub const PRODUCT_UC_PC: u32 = 0x3fc5;
pub const PRODUCT_UC_MAC: u32 = 0x3fc6;
pub const PRODUCT_UC_WIN: u32 = 0x3fce;
pub const PRODUCT_UCX: u32 = 0x3fc9;
pub const PRODUCT_BABYFACE_CLASSIC: u32 = 0x3fc7;

pub fn is_uc(product: u32) -> bool {
    matches!(product, PRODUCT_UC_PC | PRODUCT_UC_MAC | PRODUCT_UC_WIN)
}

pub fn is_ucx(product: u32) -> bool {
    product == PRODUCT_UCX
}

pub fn is_babyface_classic(product: u32) -> bool {
    product == PRODUCT_BABYFACE_CLASSIC
}

pub const XPORT_INTERRUPT: u32 = 0;
pub const XPORT_ISO: u32 = 1;

pub const CAP_VENDOR: u32 = 1 << 0;
pub const CAP_WORDS: u32 = 1 << 1;
pub const CAP_METERS: u32 = 1 << 2;
pub const CAP_STATUS: u32 = 1 << 3;
pub const CAP_PCM: u32 = 1 << 4;
pub const CAP_SNAPSHOT: u32 = 1 << 5;
pub const CAP_DIAGNOSTICS: u32 = 1 << 6;

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
    pub caps: u32,
    pub transport: u32,
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
    pub vendor: u32,
    pub product: u32,
    pub bcd_device: u32,
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

/// Accumulated diagnostics and watermarks.  `run_seq` scopes the per-run
/// values; `urb_errors` is a consecutive-error gauge, not a total.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HwdepDiagnostics {
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
    pub run_seq: u32,
    pub ring_min_filled: u32,
    pub ring_missed: u32,
    pub ring_starved: u32,
    pub ring_max_stall_ns: u64,
    pub gate_deferred: u32,
    pub gate_overdue: u32,
    pub gate_max_pending: u32,
    pub out_lag_min: i32,
    pub out_lag_max: i32,
    pub out_prefills: u32,
    pub out_prefill_timeouts: u32,
    pub meter_paused: u32,
    pub meter_pairs: u32,
    pub meter_realigns: u32,
    pub in_total: u64,
    pub reserved: [u32; 8],
}

/// Historical name kept for the device/diagnostics call sites.
pub type HwdepCounters = HwdepDiagnostics;

/// One supported rate and the ALSA geometry the driver accepts at it.
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
    pub iso_packets: u32,
    pub fb_delay: u32,
    pub out_lead: u32,
    pub lead_bias: u32,
    pub reserved: [u32; 4],
    pub rates: [HwdepRateEnv; HWDEP_MAX_RATES],
}

/// One direction of the live stream snapshot.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HwdepStream {
    pub present: u32,
    pub state: u32,
    pub appl_ptr: u64,
    pub hw_ptr: u64,
    pub fill: u32,
    pub inflight: u32,
    pub delay: u32,
    pub period: u32,
    pub buffer: u32,
    pub urb_frames: u32,
    pub nurbs: u32,
    pub iso_packets: u32,
    pub last_complete_ns: u64,
    pub device_pos: u64,
    pub xruns: u32,
    pub flags: u32,
    pub reserved: [u32; 8],
}

/// Atomic live snapshot: queue state for both directions and the device
/// clock in one sample.  `size` is set by the caller to the struct size;
/// the driver writes the prefix it knows.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HwdepSnapshot {
    pub abi: u32,
    pub size: u32,
    pub run_seq: u32,
    pub rate: u32,
    pub timestamp_ns: u64,
    pub timestamp_pos: u64,
    pub timestamp_phase: u32,
    pub rate_meas: u32,
    pub anchor_ns: u64,
    pub frames: u64,
    pub wrap: u32,
    pub flags: u32,
    pub out: HwdepStream,
    pub input: HwdepStream,
    pub reserved: [u32; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HwdepSetRate {
    /// In: requested rate, Hz.
    pub rate: u32,
    /// In: must be 0.
    pub flags: u32,
    /// Out: the state-word rate after the write, Hz.
    pub confirmed: u32,
    pub reserved: [u32; 5],
}

impl HwdepInfo {
    pub fn serial_str(&self) -> String {
        cstr(&self.serial)
    }
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
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
pub const IOCTL_DIAGNOSTICS: u64 =
    ioc(IOC_READ, b'F', 0x13, size_of::<HwdepDiagnostics>());
pub const IOCTL_ENVELOPE: u64 = ioc(IOC_READ, b'F', 0x14, size_of::<HwdepEnvelope>());
pub const IOCTL_SET_RATE: u64 =
    ioc(IOC_READ | IOC_WRITE, b'F', 0x16, size_of::<HwdepSetRate>());
pub const IOCTL_SNAPSHOT: u64 =
    ioc(IOC_READ | IOC_WRITE, b'F', 0x17, size_of::<HwdepSnapshot>());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_sizes_match_the_c_header() {
        assert_eq!(size_of::<HwdepInfo>(), 172);
        assert_eq!(size_of::<HwdepVendor>(), 74);
        assert_eq!(size_of::<HwdepWords>(), 4 + 4 + 63 * 4);
        assert_eq!(size_of::<HwdepDiagnostics>(), 232);
        assert_eq!(size_of::<HwdepRateEnv>(), 12);
        assert_eq!(size_of::<HwdepEnvelope>(), 240);
        assert_eq!(size_of::<HwdepStream>(), 112);
        assert_eq!(size_of::<HwdepSnapshot>(), 352);
        assert_eq!(size_of::<HwdepSetRate>(), 32);
    }

    #[test]
    fn ioc_number_is_stable() {
        assert_eq!(IOCTL_INFO, 0x80ac_4610);
        assert_eq!(IOCTL_DIAGNOSTICS, 0x80e8_4613);
        assert_eq!(IOCTL_ENVELOPE, 0x80f0_4614);
        assert_eq!(IOCTL_SET_RATE, 0xc020_4616);
        assert_eq!(IOCTL_SNAPSHOT, 0xc160_4617);
    }
}
