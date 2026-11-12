//! Route every playback source to every output block (diagnostic).
//! fireface-routeall <card> <0|1>
#[cfg(feature = "alsa")]
fn main() {
    use totalcontrol_core::fireface::hwdep::Direction;
    use totalcontrol_core::fireface::protocol::uc;
    use totalcontrol_core::fireface::Hwdep;
    let mut args = std::env::args().skip(1);
    let card: i32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(2);
    let on: bool = args.next().map(|s| s != "0").unwrap_or(true);
    let hw = Hwdep::open(card).expect("open hwdep");
    let linear = if on { 1.0 } else { 0.0 };
    for source in 18..36u16 {
        for block in 0..18u16 {
            let (value, index) = uc::matrix_word(linear, block * uc::MIX_STRIDE + source);
            hw.vendor(0x12, Direction::Out, value, index, &[]).expect("matrix");
        }
    }
    println!("routeall {}", if on { "on" } else { "off" });
}
#[cfg(not(feature = "alsa"))]
fn main() {}
