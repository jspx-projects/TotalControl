//! Route one stereo playback source pair to one Matrix output pair on the UC.
//! Usage: uc-route-out <source> <matrix_channel> [gain_db]
//!   matrix channels: AN1..8 = 0..7, SPDIF = 8, ADAT 1..8 = 10..17.
//! Example: uc-route-out 18 8   (playback 1/2 -> SPDIF at 0 dB)
//! Example: uc-route-out 18 4 -6 (AN5/6, cells and masters at -6 dB)
#[cfg(feature = "alsa")]
fn main() {
    use totalcontrol_core::fireface::protocol;
    use totalcontrol_core::fireface::protocol::uc;
    use totalcontrol_core::fireface::{Direction, Hwdep};
    use totalcontrol_core::{ChannelId, Fireface, RmeDevice};

    let mut args = std::env::args().skip(1);
    let source: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(18);
    let out: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);
    let gain_db: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let linear = 10f64.powf(gain_db / 20.0) as f32;

    let hw = Hwdep::open(2).expect("hwdep");
    for o in [out, out + 1] {
        for src in 0..uc::MIX_STRIDE {
            let (value, index) = uc::matrix_word(0.0, o * uc::MIX_STRIDE + src);
            hw.vendor(protocol::req::MIXER, Direction::Out, value, index, &[])
                .expect("clear matrix");
        }
    }
    for o in [out, out + 1] {
        let src = source + (o - out);
        let (value, index) = uc::matrix_word(linear, o * uc::MIX_STRIDE + src);
        hw.vendor(protocol::req::MIXER, Direction::Out, value, index, &[])
            .expect("set matrix");
        // The master is what the unit latches at engine start; repeat it in
        // all four submix banks like TotalMix does.
        for bank in [0x0000u16, 0x4000, 0x8000, 0xc000] {
            let (value, index) = uc::matrix_word(linear, uc::master_address(o as usize));
            hw.vendor(protocol::req::MIXER, Direction::Out, value, index | bank, &[])
                .expect("set master");
        }
    }
    hw.vendor(protocol::req::UNMUTE, Direction::Out, 0xffff, 0xc000, &[])
        .expect("global unmute");

    let mut dev = Fireface::open_uc().expect("UC hwdep");
    let order = totalcontrol_core::profiles::uc::PROFILE.output_order;
    let pair = (out / 2) as usize;
    let Some(internal_pair) = order.iter().position(|&p| p == pair) else {
        panic!("matrix channel {out} has no internal pair");
    };
    for side in 0..2 {
        let idx = internal_pair * 2 + side;
        dev.set_volume(ChannelId::Output(idx), idx, 1.0)
            .expect("output volume");
    }

        println!(
            "routed playback {}/{} to matrix {} / {} at {} dB",
            source,
            source + 1,
            out,
            out + 1,
            gain_db
        );
}
#[cfg(not(feature = "alsa"))]
fn main() {}
