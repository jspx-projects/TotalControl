//! Write the whole 30-index UC loopback bank.
//! fireface-loopall <card> <0|1>
#[cfg(feature = "alsa")]
fn main() {
    use totalcontrol_core::fireface::hwdep::Direction;
    use totalcontrol_core::fireface::Hwdep;
    let mut a=std::env::args().skip(1);
    let card:i32=a.next().and_then(|s|s.parse().ok()).unwrap_or(2);
    let on:bool=a.next().map(|s|s!="0").unwrap_or(true);
    let hw=Hwdep::open(card).expect("open");
    for i in 0..30u16 { hw.vendor(0x15, Direction::Out, on as u16, i, &[]).expect("loopback"); }
    println!("loopall {}", if on {"on"} else {"off"});
}
#[cfg(not(feature = "alsa"))]
fn main() {}
