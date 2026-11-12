//! Write one UC loopback channel into the 30-entry loopback bank.
//! uc-loopback <card> <index 0..29> <0|1> [more indices...]
//! Every call writes the complete bank: value 1 for the listed indices,
//! 0 elsewhere.  The layout follows uc_write_loopback_bank.

#[cfg(feature = "alsa")]
fn main() {
    let mut a = std::env::args().skip(1);
    let card: i32 = a.next().and_then(|s| s.parse().ok()).unwrap_or(2);
    let indices: Vec<usize> = a.take(32).filter_map(|s| s.parse().ok()).collect();
    let hw = totalcontrol_core::fireface::Hwdep::open(card).expect("open");
    let mut values = [0u16; 30];
    for &i in &indices {
        values[i.min(29)] = 1;
    }
    for (i, v) in values.iter().enumerate() {
        hw.vendor(
            0x15,
            totalcontrol_core::fireface::hwdep::Direction::Out,
            *v,
            i as u16,
            &[],
        )
        .expect("loopback");
    }
    println!("loopback indices {:?}", indices);
}

#[cfg(not(feature = "alsa"))]
fn main() {}
