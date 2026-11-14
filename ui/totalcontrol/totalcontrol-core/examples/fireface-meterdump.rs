//! Dump raw 0x85 meter packets from the hwdep node.
//! fireface-meterdump <card> <seconds> [delay-ms]
//!
//! Each line is `<absolute-micros> <seq> <hex>`; the absolute micros are
//! the process's monotonic clock delta at read time.  This makes the
//! inter-page timing visible for the heartbeat analysis.

#[cfg(feature = "alsa")]
fn main() {
    use std::time::{Duration, Instant};
    use totalcontrol_core::fireface::Hwdep;

    let mut args = std::env::args().skip(1);
    let card: i32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(2);
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let delay_ms: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let hw = Hwdep::open(card).expect("open hwdep");
    let start = Instant::now();
    let deadline = start + Duration::from_secs(seconds);
    let mut n = 0usize;
    while Instant::now() < deadline {
        match hw.wait(Duration::from_millis(200)) {
            Ok(true) => match hw.read_event() {
                Ok(e) => {
                    let micros = start.elapsed().as_micros();
                    let hex: String = e.data.iter().map(|b| format!("{b:02x}")).collect();
                    println!("{micros} evt kind {} seq {} flags {}", e.kind, e.seq, e.flags);
                    println!("{micros} data {}", hex);
                    n += 1;
                    if delay_ms > 0 {
                        std::thread::sleep(Duration::from_millis(delay_ms));
                    }
                }
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
    if let Ok(c) = hw.counters() {
        eprintln!(
            "events_dropped={} meter_urbs={} errors={}",
            c.events_dropped, c.meter_urbs, c.urb_errors
        );
    }
}

#[cfg(not(feature = "alsa"))]
fn main() {}
