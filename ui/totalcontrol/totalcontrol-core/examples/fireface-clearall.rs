//! Clear every UC matrix crosspoint.
//! fireface-clearall <card>
#[cfg(feature = "alsa")]
fn main() {
    use totalcontrol_core::fireface::hwdep::Direction;
    use totalcontrol_core::fireface::protocol::uc;
    use totalcontrol_core::fireface::Hwdep;
    let card:i32=std::env::args().nth(1).and_then(|s|s.parse().ok()).unwrap_or(2);
    let hw=Hwdep::open(card).expect("open");
    for source in 0..36u16 { for block in 0..18u16 {
        let (v,i)=uc::matrix_word(0.0, block*uc::MIX_STRIDE+source);
        hw.vendor(0x12, Direction::Out, v, i, &[]).expect("clear");
    }}
    println!("matrix cleared");
}
#[cfg(not(feature = "alsa"))]
fn main() {}
