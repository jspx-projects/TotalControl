//! Dump raw 0x85 meter packets from the hwdep node.
//! fireface-meterdump <card> <seconds>

#[cfg(feature = "alsa")]
fn main() {
    use std::time::{Duration, Instant};
    use totalcontrol_core::fireface::Hwdep;

    let mut args = std::env::args().skip(1);
    let card: i32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(2);
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let hw = Hwdep::open(card).expect("open hwdep");
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut n = 0usize;
    while Instant::now() < deadline {
        match hw.wait(Duration::from_millis(200)) {
            Ok(true) => match hw.read_event() {
                Ok(e) if e.kind == 1 => {
                    let hex: String = e.data.iter().map(|b| format!("{b:02x}")).collect();
                    println!("{} {}", e.seq, hex);
                    n += 1;
                }
                Ok(_) => {}
                Err(e) => {
                    // Concurrent readers (TotalControl GUI) can interleave
                    // and leave nothing to hand back on a wake; retry.
                    if e.to_string().contains("Resource temporarily unavailable") {
                        continue;
                    }
                    eprintln!("read: {e}");
                    break;
                }
            },
            Ok(false) => {}
            Err(e) => { eprintln!("poll: {e}"); break; }
        }
    }
    eprintln!("packets={n}");
}

#[cfg(not(feature = "alsa"))]
fn main() {}
