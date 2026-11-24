//! Scene control for the Fireface loopback measurement (local bench tool).
//! Multi-device: opens whichever Fireface (UC / UCX) is present.
//!
//!   fireface-scene-ctl outputs                          list output indices + names
//!   fireface-scene-ctl inputs                           list input indices + names
//!   fireface-scene-ctl save <scene.json> [words.json]   dump state, capture scene
//!   fireface-scene-ctl route <out|label> <level>        zero other sends to <out>,
//!                                         playback AN1/2 -> <out> at level,
//!                                         unmute/raise <out> if needed;
//!                                         <out> is an index or a name substring
//!   fireface-scene-ctl restore <scene.json>             apply the saved scene
//!
//! Control-only: opens hwdep, never touches the PCM or the graph clock.
//! The routing change is temporary; `restore` recalls the saved scene.

use std::time::Duration;
use totalcontrol_core::channel::ChannelId;
use totalcontrol_core::device::RmeDevice;
use totalcontrol_core::Fireface;
use totalcontrol_core::FirefaceModel;
use totalcontrol_core::Scene;

fn open_dump() -> Fireface {
    let mut dev = Fireface::open().expect("open Fireface hwdep (UC/UCX)");
    if dev.model() == FirefaceModel::Ucx {
        dev.capture_state_dump(Duration::from_secs(8))
            .expect("UCX state dump");
    }
    // UC: the model is populated from read_uc_levels / read_uc_settings at open.
    dev
}

/// Resolve an output selector: a numeric index, or a case-insensitive name
/// substring (e.g. "AN 5"). Device-agnostic across UC / UCX internal ordering.
fn resolve_output(dev: &Fireface, sel: &str) -> usize {
    if let Ok(idx) = sel.parse::<usize>() {
        return idx;
    }
    let needle = sel.to_lowercase();
    dev.outputs()
        .iter()
        .position(|o| o.name.to_lowercase().contains(&needle))
        .unwrap_or_else(|| panic!("no output name matches {sel:?}"))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    match cmd.as_str() {
        "save" => {
            let path = args.next().expect("usage: save <scene.json> [words.json]");
            let dev = open_dump();
            let scene = dev.capture_scene();
            std::fs::write(&path, serde_json::to_string_pretty(&scene).unwrap())
                .expect("write scene");
            if let Some(wp) = args.next() {
                let words: Vec<(u16, u16)> = {
                    let mut w: Vec<_> = dev.state_words().iter().map(|(&a, &v)| (a, v)).collect();
                    w.sort();
                    w
                };
                std::fs::write(&wp, serde_json::to_string(&words).unwrap())
                    .expect("write words");
                println!("saved {} state words to {wp}", words.len());
            }
            println!(
                "saved scene: {} inputs, {} playbacks, {} outputs -> {path}",
                scene.inputs.len(),
                scene.playbacks.len(),
                scene.outputs.len()
            );
        }
        "route" => {
            let sel = args.next().expect("usage: route <out|label> <level>");
            let level: f32 = args.next().and_then(|s| s.parse().ok()).expect("level");
            let mut dev = open_dump();
            let out = resolve_output(&dev, &sel); // mono index into outputs()
            // Input/playback sends are indexed by output PAIR, not by mono
            // channel; Output(idx) mute/volume is a mono index. See the
            // channel-numbering note in the tooling directory.
            let pair = out / 2;
            let n_in = dev.inputs().len();
            let n_pb = dev.playbacks().len();
            let mut zeroed = 0;
            for i in 0..n_in {
                if dev.volume(ChannelId::Input(i), pair).unwrap_or(0.0) > 0.0 {
                    dev.set_volume(ChannelId::Input(i), pair, 0.0).expect("zero input send");
                    zeroed += 1;
                }
            }
            for i in 0..n_pb {
                let want = if i < 2 { level } else { 0.0 };
                if (dev.volume(ChannelId::Playback(i), pair).unwrap_or(0.0) - want).abs() > 1e-6 {
                    dev.set_volume(ChannelId::Playback(i), pair, want).expect("set playback send");
                    if i >= 2 {
                        zeroed += 1;
                    }
                }
            }
            for side in [out & !1, out | 1] {
                if dev.mute(ChannelId::Output(side)).unwrap_or(false) {
                    dev.set_mute(ChannelId::Output(side), false).expect("unmute output");
                    println!("output {side} was muted -> unmuted");
                }
                let ov = dev.volume(ChannelId::Output(side), 0).unwrap_or(0.0);
                if ov < 0.01 {
                    dev.set_volume(ChannelId::Output(side), 0, 1.0).expect("raise output");
                    println!("output {side} volume was {ov:.3} -> 1.0");
                }
            }
            println!(
                "routed playback AN1/2 -> {} (pair {pair}) at {level:.3}; zeroed {zeroed} other sends",
                dev.outputs()[out].name
            );
        }
        "restore" => {
            let path = args.next().expect("usage: restore <scene.json>");
            let scene: Scene =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read scene"))
                    .expect("parse scene");
            let mut dev = open_dump();
            dev.apply_scene(&scene).expect("apply scene");
            println!("restored scene from {path}");
        }
        "loopback" => {
            let pair: usize = args.next().and_then(|s| s.parse().ok()).expect("pair");
            let on: u32 = args.next().and_then(|s| s.parse().ok()).expect("0|1");
            let mut dev = open_dump();
            dev.set_loopback(pair, on != 0).expect("set loopback");
            println!("loopback pair {pair} = {on}");
        }
        "mgrid" => {
            // Dump raw matrix coefficients for playback sources (0x20..0x29)
            // across DSP outputs 0..7, then optionally write playback 1 ->
            // output <out> at unity and re-dump to compare.  Addresses are
            // 0x4000 + dsp*0x40 + source (UCX protocol spec 4.3).
            let out: Option<usize> = args.next().and_then(|s| s.parse().ok());
            let mut dev = open_dump();
            let dump_grid = |dev: &Fireface, tag: &str| {
                println!("--- matrix grid {tag} (raw hex, dsp rows x playback-source cols)");
                print!("dsp ");
                for s in 0x20..=0x29 { print!(" {:04x}", s); }
                println!();
                for dsp in 0..8u16 {
                    print!("{dsp:3} ");
                    for s in 0x20..=0x29u16 {
                        let addr = 0x4000 + dsp * 0x40 + s;
                        let v = dev.state_words().get(&addr).copied().unwrap_or(0);
                        print!(" {:04x}", v);
                    }
                    println!();
                }
            };
            dump_grid(&dev, "before");
            if let Some(out) = out {
                dev.set_volume(ChannelId::Playback(0), out, 1.0).expect("set pb1 send");
                println!("wrote playback 1 -> output {} = 1.0", out + 1);
                drop(dev);
                let dev2 = open_dump();
                dump_grid(&dev2, "after fresh dump");
                println!("parsed pb1 -> {}: {:.3}", out + 1,
                    dev2.volume(ChannelId::Playback(0), out).unwrap_or(0.0));
            }
        }
        "wtest" => {
            // Decisive control-write round-trip: set output <out> volume to
            // <level>, re-dump fresh, read back, then restore the original.
            let out: usize = args.next().and_then(|s| s.parse().ok()).expect("out index");
            let level: f32 = args.next().and_then(|s| s.parse().ok()).expect("level");
            let mut dev = open_dump();
            let orig = dev.volume(ChannelId::Output(out), 0).unwrap_or(0.0);
            println!("before: output {} volume {orig:.3}", out + 1);
            dev.set_volume(ChannelId::Output(out), 0, level).expect("set output volume");
            println!("wrote output {} volume {level:.3}", out + 1);
            drop(dev);
            // Fresh handle + fresh dump: the readback is the device's state,
            // not the writing handle's in-memory model.
            let dev2 = open_dump();
            let after = dev2.volume(ChannelId::Output(out), 0).unwrap_or(0.0);
            println!("after fresh dump: output {} volume {after:.3}", out + 1);
            println!("write landed: {}", (after - level).abs() < 1e-3);
            drop(dev2);
            let mut dev3 = open_dump();
            dev3.set_volume(ChannelId::Output(out), 0, orig).expect("restore output volume");
            println!("restored output {} volume {orig:.3}", out + 1);
        }
        "volumes" => {
            let out: usize = args.next().and_then(|s| s.parse().ok()).expect("out index");
            let dev = open_dump();
            for (i, _) in dev.inputs().iter().enumerate() {
                let v = dev.volume(ChannelId::Input(i), out).unwrap_or(0.0);
                if v > 0.0 {
                    println!("input {} -> {out}: {v:.3}", i + 1);
                }
            }
            for (i, _) in dev.playbacks().iter().enumerate() {
                let v = dev.volume(ChannelId::Playback(i), out).unwrap_or(0.0);
                if v > 0.0 {
                    println!("playback {} -> {out}: {v:.3}", i + 1);
                }
            }
            println!(
                "output {}: volume {:.3} mute {}",
                out + 1,
                dev.volume(ChannelId::Output(out), 0).unwrap_or(0.0),
                dev.mute(ChannelId::Output(out)).unwrap_or(false)
            );
        }
        "outputs" => {
            let dev = open_dump();
            println!("model: {:?}", dev.model());
            for (i, o) in dev.outputs().iter().enumerate() {
                println!("out {i:2}  {}", o.name);
            }
        }
        "inputs" => {
            let dev = open_dump();
            for (i, c) in dev.inputs().iter().enumerate() {
                println!("in  {i:2}  {}", c.name);
            }
        }
        _ => {
            eprintln!("usage: fireface-scene-ctl outputs|inputs|save|route|restore ...");
            std::process::exit(2);
        }
    }
}
