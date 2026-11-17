//! Parity between the generic engine and the Mackie codec.
//!
//! The acceptance gate of the generic controller model: on the Mackie
//! definition, decode + dispatch must produce the same messages and
//! navigation as `mackie::parse`, and the feedback must produce the same
//! message multiset as `mackie::refresh`. The wire order of independent
//! messages carries no meaning; the multisets must match exactly.

use std::collections::BTreeMap;

use totalcontrol_core::ChannelId;

use crate::app::Message;
use crate::host_settings::ControllerValue;
use crate::mackie::{
    self, MackieConfig, MackieNav, MidiEvent, StripFeedback, Window, STRIPS,
};
use crate::osc_rme::Bus;

use super::engine::{self, ControlEvent, DispatchCtx, FeedbackCtx, InstanceState, MainFlags};
use super::map::ControllerMap;

fn map() -> ControllerMap {
    ControllerMap::parse(super::load::EMBEDDED_MACKIE)
        .unwrap()
        .validate()
        .unwrap()
}

fn values(cfg: &MackieConfig) -> BTreeMap<String, ControllerValue> {
    let bools = [
        ("protocol", cfg.protocol),
        ("full_lcd", cfg.full_lcd),
        ("send_names", cfg.send_names),
        ("stereo_to_mono", cfg.stereo_to_mono),
        ("submix_any_row", cfg.submix_any_row),
        ("background_disable", cfg.background_disable),
        ("send_levels", cfg.send_levels),
        ("peak_hold", cfg.peak_hold),
        ("submix_linked", cfg.submix_linked),
    ];
    bools
        .iter()
        .map(|(k, v)| (k.to_string(), ControllerValue::Bool(*v)))
        .collect()
}

fn window() -> Window {
    std::array::from_fn(|i| Some(ChannelId::Input(i)))
}

fn out_window() -> Window {
    std::array::from_fn(|i| Some(ChannelId::Output(i * 2)))
}

const COUNTS: [usize; 3] = [18, 18, 18];
const STRIP_COUNTS: [usize; 3] = [18, 18, 9];

fn navs() -> Vec<MackieNav> {
    let mut navs = vec![MackieNav::default()];
    let mut trim = MackieNav::default();
    trim.trim = true;
    navs.push(trim);
    let mut out = MackieNav::default();
    out.bus = Bus::Output;
    out.submix = 2;
    out.bank = 3;
    navs.push(out);
    let mut play = MackieNav::default();
    play.bus = Bus::Playback;
    navs.push(play);
    navs
}

/// Run one event through both paths and assert the messages and the
/// navigation agree.
fn assert_parity(map: &ControllerMap, cfg: &MackieConfig, nav: &MackieNav, ev: &MidiEvent) {
    let vals = values(cfg);
    let assignments = BTreeMap::new();
    let ctx = DispatchCtx {
        window: window(),
        out_window: out_window(),
        counts: COUNTS,
        strip_counts: STRIP_COUNTS,
        submix_count: 9,
        values: &vals,
        assignments: &assignments,
        external_input: false,
        selected_pair: None,
    };

    let mut legacy_nav = nav.clone();
    let legacy = mackie::parse(
        0,
        ev,
        &mut legacy_nav,
        cfg,
        &window(),
        &out_window(),
        COUNTS,
        STRIP_COUNTS,
        9,
    );

    let mut state = InstanceState { nav: nav.clone(), ..Default::default() };
    let events = engine::decode(map, &mut state, ev);
    let mut generic: Vec<Message> = Vec::new();
    for cev in &events {
        generic.extend(engine::dispatch(map, &mut state, &ctx, cev));
    }

    let legacy_msgs = legacy.unwrap_or_default();
    assert_eq!(
        format!("{legacy_msgs:?}"),
        format!("{generic:?}"),
        "messages differ for {ev:?} (protocol={}, bus={:?}, trim={})",
        cfg.protocol,
        nav.bus,
        nav.trim
    );
    assert_eq!(legacy_nav.nav(), state.nav.nav(), "nav differs for {ev:?}");
    assert_eq!(legacy_nav.sel_held, state.nav.sel_held, "sel_held differs for {ev:?}");
}

fn corpus() -> Vec<MidiEvent> {
    let mut evs = Vec::new();
    // Every note on channel 1, at the release/press boundary velocities,
    // plus a foreign channel.
    for note in 0..=127u8 {
        for vel in [0u8, 63, 64, 127] {
            evs.push(MidiEvent::NoteOn { ch: 0, note, vel });
        }
        evs.push(MidiEvent::NoteOff { ch: 0, note });
        evs.push(MidiEvent::NoteOn { ch: 1, note, vel: 127 });
    }
    // The loopback note.
    evs.push(MidiEvent::NoteOn {
        ch: mackie::LOOPBACK_CHANNEL,
        note: mackie::LOOPBACK_NOTE,
        vel: mackie::LOOPBACK_VELOCITY,
    });
    // Faders.
    for ch in 0..=9u8 {
        for value in [0u16, 104 * 129, 8192, 16383] {
            evs.push(MidiEvent::PitchBend { ch, value });
        }
    }
    // Simple CC volume and gain.
    for val in [0u8, 64, 104, 127] {
        evs.push(MidiEvent::Cc { ch: 0, cc: 7, val });
        for ch in 0..=15u8 {
            evs.push(MidiEvent::Cc { ch, cc: 9, val });
        }
    }
    // Row faders: CC 102..117 on channels 1..12.
    for ch in 0..=11u8 {
        for cc in [102u8, 103, 110, 117] {
            for val in [0u8, 104, 127] {
                evs.push(MidiEvent::Cc { ch, cc, val });
            }
        }
    }
    // Channel 13: trim and submix selection.
    for cc in [102u8, 103, 104, 105, 116, 126, 127] {
        for val in [0u8, 127] {
            evs.push(MidiEvent::Cc { ch: 12, cc, val });
        }
    }
    // VPOTs and jog.
    for cc in 16..=23u8 {
        for val in [0u8, 1, 63, 64, 65, 127] {
            evs.push(MidiEvent::Cc { ch: 0, cc, val });
        }
    }
    for val in [1u8, 65] {
        evs.push(MidiEvent::Cc { ch: 0, cc: 60, val });
    }
    // FX preset channels: unsupported, ignored.
    evs.push(MidiEvent::Cc { ch: 2, cc: 12, val: 3 });
    evs.push(MidiEvent::Cc { ch: 3, cc: 12, val: 3 });
    evs
}

#[test]
fn decode_and_dispatch_match_the_mackie_codec() {
    let map = map();
    for protocol in [true, false] {
        let cfg = MackieConfig { protocol, ..Default::default() };
        for nav in navs() {
            for ev in corpus() {
                assert_parity(&map, &cfg, &nav, &ev);
            }
        }
    }
}

#[test]
fn the_submix_any_row_option_matches() {
    let map = map();
    let cfg = MackieConfig { submix_any_row: true, ..Default::default() };
    for nav in navs() {
        for note in 0..=7u8 {
            assert_parity(&map, &cfg, &nav, &MidiEvent::NoteOn { ch: 0, note, vel: 127 });
        }
    }
}

// ---- feedback parity ------------------------------------------------------

/// Split a concatenated MIDI byte stream into messages.
fn split_messages(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let status = bytes[i];
        if status == 0xf0 {
            let start = i;
            while i < bytes.len() && bytes[i] != 0xf7 {
                i += 1;
            }
            i = (i + 1).min(bytes.len());
            out.push(bytes[start..i].to_vec());
            continue;
        }
        let len = match status & 0xf0 {
            0xc0 | 0xd0 => 2,
            0xf0 => match status {
                0xf1 | 0xf3 => 2,
                0xf2 => 3,
                _ => 1,
            },
            _ => 3,
        };
        let end = (i + len).min(bytes.len());
        out.push(bytes[i..end].to_vec());
        i = end;
    }
    out
}

fn sorted(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut msgs = split_messages(bytes);
    msgs.sort();
    msgs
}

fn feedback_input() -> mackie::FeedbackInput {
    let strips: [StripFeedback; STRIPS] = std::array::from_fn(|i| StripFeedback {
        fader: (i as f32) / 8.0,
        mute: i % 2 == 0,
        solo: i % 3 == 0,
        select: i == 2,
        rec: i == 4,
        pan: (i as i8 - 4) * 20,
        name: format!("Ch {}", i + 1),
        value: format!("{:.1}", -(i as f32) * 1.5),
    });
    mackie::FeedbackInput {
        strips,
        master_fader: 0.75,
        dim: true,
        talkback: false,
        mono: true,
        speaker_b: false,
        master_mute: false,
        master_solo: true,
        trim: true,
        rude_solo: true,
        full_lcd: true,
        send_names: true,
        row_name: "In",
        first_channel: 1,
    }
}

fn feedback_ctx<'a>(
    input: &mackie::FeedbackInput,
    vals: &'a BTreeMap<String, ControllerValue>,
) -> FeedbackCtx<'a> {
    FeedbackCtx {
        strips: input.strips.clone(),
        master_fader: input.master_fader,
        main: MainFlags {
            dim: input.dim,
            talkback: input.talkback,
            mono: input.mono,
            speaker_b: input.speaker_b,
            mute: input.master_mute,
            solo: input.master_solo,
            rude_solo: input.rude_solo,
        },
        trim: input.trim,
        external_input: false,
        levels: std::array::from_fn(|i| -(i as f32) * 7.0),
        over: std::array::from_fn(|i| i == 3),
        row_name: input.row_name,
        first_channel: input.first_channel,
        snapshot_current: Some(3),
        values: vals,
    }
}

#[test]
fn feedback_matches_the_mackie_encoder() {
    let map = map();
    for full_lcd in [true, false] {
        for send_names in [true, false] {
            let cfg = MackieConfig { full_lcd, send_names, ..Default::default() };
            let vals = values(&cfg);
            let mut input = feedback_input();
            input.full_lcd = full_lcd;
            input.send_names = send_names;

            let mut legacy_surface = mackie::MackieSurface::default();
            let mut legacy_out = Vec::new();
            mackie::refresh(&input, &mut legacy_surface, &mut legacy_out);

            let mut state = InstanceState::default();
            let ctx = feedback_ctx(&input, &vals);
            let mut generic_out = Vec::new();
            engine::refresh(&map, &mut state, &ctx, &mut generic_out);

            assert_eq!(
                sorted(&legacy_out),
                sorted(&generic_out),
                "feedback differs (full_lcd={full_lcd}, send_names={send_names})"
            );

            // A second refresh with unchanged state emits nothing.
            let mut legacy_again = Vec::new();
            mackie::refresh(&input, &mut legacy_surface, &mut legacy_again);
            let mut generic_again = Vec::new();
            engine::refresh(&map, &mut state, &ctx, &mut generic_again);
            assert!(legacy_again.is_empty());
            assert!(generic_again.is_empty(), "{generic_again:?}");
        }
    }
}

#[test]
fn meters_match_the_mackie_encoder() {
    let map = map();
    let cfg = MackieConfig { send_levels: true, ..Default::default() };
    let vals = values(&cfg);
    let input = feedback_input();
    let ctx = feedback_ctx(&input, &vals);

    let mut legacy_surface = mackie::MackieSurface::default();
    let mut legacy_out = Vec::new();
    mackie::refresh_meters(&ctx.levels, &ctx.over, &mut legacy_surface, &mut legacy_out);

    let mut state = InstanceState::default();
    let mut generic_out = Vec::new();
    engine::refresh_meters(&map, &mut state, &ctx, &mut generic_out);

    assert_eq!(sorted(&legacy_out), sorted(&generic_out));

    let mut legacy_again = Vec::new();
    mackie::refresh_meters(&ctx.levels, &ctx.over, &mut legacy_surface, &mut legacy_again);
    let mut generic_again = Vec::new();
    engine::refresh_meters(&map, &mut state, &ctx, &mut generic_again);
    assert!(legacy_again.is_empty());
    assert!(generic_again.is_empty());
}

#[test]
fn the_handshake_matches_the_mackie_codec() {
    let map = map();
    let mut state = InstanceState::default();
    let serial = [1u8, 2, 3, 4, 5, 6, 7];
    let challenge = [0x11u8, 0x22, 0x33, 0x44];
    let mut query = mackie::SYSEX_HEADER.to_vec();
    query.push(0x01);
    query.extend_from_slice(&serial);
    query.extend_from_slice(&challenge);
    query.push(0xf7);
    match engine::sysex(&map, &mut state, &query) {
        engine::SysexOutcome::Reply(bytes) => {
            assert_eq!(bytes, mackie::host_connection_reply(&serial, &challenge));
            assert_eq!(state.nav.serial, serial);
        }
        _ => panic!("expected a reply"),
    }
    let mut confirm = mackie::SYSEX_HEADER.to_vec();
    confirm.push(0x03);
    confirm.extend_from_slice(&serial);
    confirm.push(0xf7);
    assert!(matches!(engine::sysex(&map, &mut state, &confirm), engine::SysexOutcome::Online));
    assert!(state.nav.online);

    assert_eq!(
        engine::on_open(&map),
        vec![mackie::device_query(), mackie::version_request()]
    );
    assert_eq!(engine::on_close(&map), Some(mackie::go_offline()));
}

#[test]
fn the_loopback_guard_matches_the_detection_note() {
    let map = map();
    let note = MidiEvent::NoteOn {
        ch: mackie::LOOPBACK_CHANNEL,
        note: mackie::LOOPBACK_NOTE,
        vel: mackie::LOOPBACK_VELOCITY,
    };
    assert!(engine::is_loopback(&map, &note));
    assert!(!engine::is_loopback(&map, &MidiEvent::NoteOn { ch: 0, note: 60, vel: 127 }));
    let (bytes, period) = engine::loopback(&map).unwrap();
    assert_eq!(bytes, mackie::loopback_note().to_vec());
    assert_eq!(period, 500);
}

#[test]
fn protocol_off_keeps_the_simple_cc_path() {
    let map = map();
    let cfg = MackieConfig { protocol: false, ..Default::default() };
    let vals = values(&cfg);
    let assignments = BTreeMap::new();
    let ctx = DispatchCtx {
        window: window(),
        out_window: out_window(),
        counts: COUNTS,
        strip_counts: STRIP_COUNTS,
        submix_count: 9,
        values: &vals,
        assignments: &assignments,
        external_input: false,
        selected_pair: None,
    };
    let mut state = InstanceState::default();
    // A surface note is inert.
    let ev = MidiEvent::NoteOn { ch: 0, note: 16, vel: 127 };
    for cev in engine::decode(&map, &mut state, &ev) {
        assert!(engine::dispatch(&map, &mut state, &ctx, &cev).is_empty());
    }
    // The simple CC volume still works.
    let ev = MidiEvent::Cc { ch: 0, cc: 7, val: 104 };
    let events = engine::decode(&map, &mut state, &ev);
    assert_eq!(events.len(), 1);
    let msgs = engine::dispatch(&map, &mut state, &ctx, &events[0]);
    assert_eq!(msgs.len(), 1);
    assert!(matches!(msgs[0], Message::RmeMasterVolume(_)));
}

#[test]
fn an_assignment_overrides_the_shipped_action() {
    let map = map();
    let cfg = MackieConfig::default();
    let vals = values(&cfg);
    let mut assignments = BTreeMap::new();
    assignments.insert("mute".to_string(), "strip_solo".to_string());
    let ctx = DispatchCtx {
        window: window(),
        out_window: out_window(),
        counts: COUNTS,
        strip_counts: STRIP_COUNTS,
        submix_count: 9,
        values: &vals,
        assignments: &assignments,
        external_input: false,
        selected_pair: None,
    };
    let mut state = InstanceState::default();
    let ev = MidiEvent::NoteOn { ch: 0, note: 16, vel: 127 };
    let events = engine::decode(&map, &mut state, &ev);
    let msgs = engine::dispatch(&map, &mut state, &ctx, &events[0]);
    assert_eq!(
        format!("{msgs:?}"),
        format!("{:?}", vec![Message::RmeToggle(crate::osc_rme::RmeToggle::Solo(ChannelId::Input(0)))])
    );
}

#[test]
fn the_arc_encoder_decodes_sign_and_magnitude() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/work/ARC-USB/MAPS/RME_ARC_USB_MIDI.json"
    ))
    .unwrap();
    let map = ControllerMap::parse(&text).unwrap().validate().unwrap();
    let mut state = InstanceState::default();
    let cases = [
        (0x01u8, 1i32),
        (0x0f, 15),
        (0x41, -1),
        (0x4f, -15),
        (0x00, 0),
    ];
    for (val, steps) in cases {
        let ev = MidiEvent::Cc { ch: 0, cc: 16, val };
        let events = engine::decode(&map, &mut state, &ev);
        assert_eq!(events.len(), 1, "val {val:#x}");
        match &events[0] {
            ControlEvent::Value { decoded: engine::Decoded::Steps(s), .. } => {
                assert_eq!(*s, steps, "val {val:#x}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    // Every key decodes on any channel.
    for note in 54..=69u8 {
        let ev = MidiEvent::NoteOn { ch: 4, note, vel: 127 };
        let events = engine::decode(&map, &mut state, &ev);
        assert_eq!(events.len(), 1, "note {note}");
        assert!(matches!(events[0], ControlEvent::Press { .. }));
    }
}

fn arc_map() -> ControllerMap {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/work/ARC-USB/MAPS/RME_ARC_USB_MIDI.json"
    ))
    .unwrap();
    ControllerMap::parse(&text).unwrap().validate().unwrap()
}

fn arc_ctx<'a>(
    values: &'a BTreeMap<String, ControllerValue>,
    assignments: &'a BTreeMap<String, String>,
) -> DispatchCtx<'a> {
    DispatchCtx {
        window: Default::default(),
        out_window: Default::default(),
        counts: [0; 3],
        strip_counts: [0; 3],
        submix_count: 9,
        values,
        assignments,
        external_input: false,
        selected_pair: None,
    }
}

#[test]
fn the_arc_encoder_nudges_its_target() {
    let map = arc_map();
    let values = BTreeMap::new();
    let assignments = BTreeMap::new();
    let ctx = arc_ctx(&values, &assignments);
    let mut state = InstanceState::default();
    // One click clockwise: 0.5 dB on the default main target.
    let ev = MidiEvent::Cc { ch: 0, cc: 16, val: 0x01 };
    let events = engine::decode(&map, &mut state, &ev);
    let msgs = engine::dispatch(&map, &mut state, &ctx, &events[0]);
    assert_eq!(msgs.len(), 1);
    match &msgs[0] {
        Message::EncoderNudge { target, db } => {
            assert_eq!(target, "main");
            assert!((db - 0.5).abs() < 1e-6);
        }
        other => panic!("unexpected {other:?}"),
    }
    // A key bound to encoder_target switches the target.
    let ev = MidiEvent::NoteOn { ch: 0, note: 63, vel: 127 };
    let events = engine::decode(&map, &mut state, &ev);
    let _ = engine::dispatch(&map, &mut state, &ctx, &events[0]);
    assert_eq!(state.encoder_target, "phones1");
    // Fine step: 0.1 dB per click.
    let mut fine = BTreeMap::new();
    fine.insert("encoder_step".into(), ControllerValue::Choice("fine".into()));
    let ctx = arc_ctx(&fine, &assignments);
    let ev = MidiEvent::Cc { ch: 0, cc: 16, val: 0x41 };
    let events = engine::decode(&map, &mut state, &ev);
    let msgs = engine::dispatch(&map, &mut state, &ctx, &events[0]);
    match &msgs[0] {
        Message::EncoderNudge { target, db } => {
            assert_eq!(target, "phones1");
            assert!((db + 0.1).abs() < 1e-6);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn the_arc_talkback_key_is_momentary_when_the_option_says_so() {
    let map = arc_map();
    let mut values = BTreeMap::new();
    values.insert(
        "talkback_mode".into(),
        ControllerValue::Choice("momentary".into()),
    );
    let assignments = BTreeMap::new();
    let ctx = arc_ctx(&values, &assignments);
    let mut state = InstanceState::default();
    let press = MidiEvent::NoteOn { ch: 0, note: 66, vel: 127 };
    let release = MidiEvent::NoteOn { ch: 0, note: 66, vel: 0 };
    let events = engine::decode(&map, &mut state, &press);
    let msgs = engine::dispatch(&map, &mut state, &ctx, &events[0]);
    assert_eq!(format!("{msgs:?}"), format!("{:?}", vec![Message::TalkbackChanged(true)]));
    let events = engine::decode(&map, &mut state, &release);
    let msgs = engine::dispatch(&map, &mut state, &ctx, &events[0]);
    assert_eq!(format!("{msgs:?}"), format!("{:?}", vec![Message::TalkbackChanged(false)]));
    // Toggle mode: one RmeToggle on press, nothing on release.
    let values = BTreeMap::new();
    let ctx = arc_ctx(&values, &assignments);
    let events = engine::decode(&map, &mut state, &press);
    let msgs = engine::dispatch(&map, &mut state, &ctx, &events[0]);
    assert_eq!(
        format!("{msgs:?}"),
        format!("{:?}", vec![Message::RmeToggle(crate::osc_rme::RmeToggle::MainTalkback)])
    );
}

#[test]
fn the_arc_leds_mirror_the_action_state() {
    let map = arc_map();
    let values = BTreeMap::new();
    let mut state = InstanceState::default();
    let mut ctx = feedback_ctx(&feedback_input(), &values);
    ctx.main.dim = true;
    ctx.main.mono = false;
    ctx.snapshot_current = Some(3);
    let mut out = Vec::new();
    engine::refresh(&map, &mut state, &ctx, &mut out);
    let msgs = split_messages(&out);
    // Key 15 (Dim, note 68) lit; key 9 (Mono, note 62) off; key 3
    // (snapshot 3, note 56) lit while slot 3 is current.
    assert!(msgs.contains(&vec![0x90, 68, 127]), "{msgs:?}");
    assert!(msgs.contains(&vec![0x90, 62, 0]), "{msgs:?}");
    assert!(msgs.contains(&vec![0x90, 56, 127]), "{msgs:?}");
    // The LED diff: a second refresh with unchanged state is empty.
    let mut again = Vec::new();
    engine::refresh(&map, &mut state, &ctx, &mut again);
    assert!(again.is_empty(), "{again:?}");
    // led_feedback off stops the LEDs.
    let mut off = BTreeMap::new();
    off.insert("led_feedback".into(), ControllerValue::Bool(false));
    let ctx = feedback_ctx(&feedback_input(), &off);
    let mut state = InstanceState::default();
    let mut out = Vec::new();
    engine::refresh(&map, &mut state, &ctx, &mut out);
    assert!(out.is_empty());
}