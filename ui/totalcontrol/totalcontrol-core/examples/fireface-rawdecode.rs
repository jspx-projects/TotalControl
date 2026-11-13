//! Replay a raw `0x85` capture through the UC meter parsers.
//! fireface-rawdecode <capture-file>
//!
//! The capture format is the `fireface-meterdump` output: one
//! `<seq> <hex>` line per event.  Only 324-byte USB messages decode here.

use totalcontrol_core::fireface::protocol::{UcMeterPacket, UcRmsPacket};

#[cfg(feature = "alsa")]
fn main() {
    let path = std::env::args().nth(1).expect("usage: fireface-rawdecode <capture>");
    let text = std::fs::read_to_string(path).expect("read");
    let mut data = 0usize;
    let mut rms = 0usize;
    let mut rejected = 0usize;
    for line in text.lines() {
        let Some((seq_str, payload)) = line.split_once(' ') else { continue };
        let Ok(seq) = seq_str.parse::<u32>() else { continue };
        if payload.len() != UcMeterPacket::LEN * 2 {
            continue;
        }
        let mut buf = [0u8; UcMeterPacket::LEN];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = u8::from_str_radix(&payload[i * 2..i * 2 + 2], 16)
                .expect("hex digit");
        }
        if let Some(p) = UcMeterPacket::parse(&buf) {
            data += 1;
            println!("data seq {seq} play1 {:08x} play5 {:08x} out7 {:08x} out9 {:08x}",
                p.playback(0), p.playback(4), p.output(6), p.output(8));
            continue;
        }
        if let Some(p) = UcRmsPacket::parse(&buf) {
            rms += 1;
            let act: Vec<String> = (0..18)
                .filter(|i| p.playback(*i) != 0)
                .map(|i| format!("pb{i}={}", p.playback(i)))
                .collect();
            if !act.is_empty() {
                println!("rms  seq {seq} {}", act.join(" "));
            }
            continue;
        }
        rejected += 1;
    }
    eprintln!("data={data} rms={rms} rejected={rejected}");
}
