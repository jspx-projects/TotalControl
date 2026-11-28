//! Decode the UCX full state dump into scene fields.
//!
//! The dump is requested with `0x3f99 = 0x0812`; see
//! `docs/spec/FIREFFACE-UCX-PROTOCOL-SPEC.md`, appendix A.

use std::collections::HashMap;

use crate::channel::{
    Dynamics, Echo, EqBandType, InputEq, InputLevel, Reverb,
};
use crate::scene::Scene;

/// Output pairs in the requested dump.  TotalControl's pair 0 is Phones;
/// the dump stores Phones at 0x18 and AN1/2 at 0x12.
const OUTPUT_BLOCKS: [u16; 9] = [0x18, 0x12, 0x14, 0x16, 0x1a, 0x1c, 0x1e, 0x20, 0x22];

#[derive(Clone, Debug, Default)]
pub struct UcxDump {
    pub words: HashMap<u16, u16>,
}

impl UcxDump {
    pub fn new(words: HashMap<u16, u16>) -> Self {
        Self { words }
    }

    fn input(&self, idx: usize, offset: u16) -> Option<u16> {
        self.words.get(&(((idx as u16) << 8) | offset)).copied()
    }

    fn output(&self, pair: usize, side: usize, offset: u16) -> Option<u16> {
        let block = *OUTPUT_BLOCKS.get(pair)? + side as u16;
        self.words.get(&((block << 8) | offset)).copied()
    }

    fn matrix_at(&self, pair: usize, side: usize, source: usize) -> Option<(u16, u16)> {
        let block = *OUTPUT_BLOCKS.get(pair)? + side as u16;
        let offset = if source < 8 {
            0xa0 + 2 * source as u16
        } else if source < 10 {
            0xc0 + 2 * (source as u16 - 8)
        } else if source < 18 {
            0xe0 + 2 * (source as u16 - 10)
        } else {
            return None;
        };
        Some((
            self.words.get(&((block << 8) | offset)).copied()?,
            self.words.get(&((block << 8) | offset + 1)).copied()?,
        ))
    }

    fn signed(word: u16) -> i16 {
        word as i16
    }

    fn tenths_db(word: u16) -> f32 {
        Self::signed(word) as f32 / 10.0
    }

    fn db_to_linear(db: f32) -> f32 {
        10f32.powf(db / 20.0)
    }

    fn matrix_level(word: u16) -> f32 {
        // A zero record is no send (not unity).
        if word == 0 {
            return 0.0;
        }
        let db = (Self::signed(word) >> 4) as f32 / 10.0;
        Self::db_to_linear(db)
    }

    fn matrix_pan(word: u16) -> i8 {
        Self::signed(word).clamp(-100, 100) as i8
    }

    fn pan_split(volume: f32, pan: i8) -> (f32, f32) {
        let v = volume.clamp(0.0, 2.0);
        let p = pan.clamp(-100, 100) as f32 / 100.0;
        (
            v * if p > 0.0 { 1.0 - p } else { 1.0 },
            v * if p < 0.0 { 1.0 + p } else { 1.0 },
        )
    }

    /// Compare the dump with `scene` and return human-readable differences.
    /// The dump's known fields are also applied to a clone of the scene.
    pub fn differences_into(&self, scene: &mut Scene) -> Vec<String> {
        let mut out = Vec::new();
        if self.words.is_empty() {
            return out;
        }
        // Compare against the same pair-normalised model that the apply
        // path writes.
        scene.normalize_ucx_pairs();

        for (idx, input) in scene.inputs.iter_mut().enumerate().take(18) {
            if idx < 4 {
                if let Some(word) = self.input(idx, 0x07) {
                    let gain = if idx < 2 {
                        word as u32 / 10
                    } else {
                        word as u32 / 5
                    };
                    if input.gain != Some(gain) {
                        let old = input
                            .gain
                            .map(|g| format!("{g}"))
                            .unwrap_or_else(|| "-".into());
                        out.push(format!(
                            "AN{} gain: device {gain}, TotalControl {old}",
                            idx + 1
                        ));
                        input.gain = Some(gain);
                    }
                }
            }
            if let Some(word) = self.input(idx, 0x08) {
                if idx < 2 {
                    let phantom = word & 1 != 0;
                    if input.phantom != phantom {
                        out.push(format!(
                            "AN{} 48 V: device {phantom}, TotalControl {}",
                            idx + 1,
                            input.phantom
                        ));
                        input.phantom = phantom;
                    }
                } else if let Some(level) = InputLevel::from_code(word) {
                    if input.level != Some(level) {
                        out.push(format!(
                            "AN{} Level: device {:?}, TotalControl {:?}",
                            idx + 1,
                            level,
                            input.level
                        ));
                        input.level = Some(level);
                        input.ref_level = level.code();
                    }
                }
            }
            if let Some(word) = self.input(idx, 0x09) {
                let on = word != 0;
                if input.instrument != on {
                    out.push(format!(
                        "AN{} Inst: device {on}, TotalControl {}",
                        idx + 1,
                        input.instrument
                    ));
                    input.instrument = on;
                }
            }
            if let Some(word) = self.input(idx, 0x0a) {
                let on = word != 0;
                if input.autoset != on {
                    out.push(format!(
                        "AN{} AutoSet: device {on}, TotalControl {}",
                        idx + 1,
                        input.autoset
                    ));
                    input.autoset = on;
                }
            }
            if let Some(word) = self.input(idx, 0x00) {
                let mute = word != 0;
                if input.mute != mute {
                    out.push(format!(
                        "AN{} mute: device {mute}, TotalControl {}",
                        idx + 1,
                        input.mute
                    ));
                    input.mute = mute;
                }
            }
            if let Some(word) = self.input(idx, 0x01) {
                let db = Self::tenths_db(word);
                if (input.fx_send_db - db).abs() > 0.05 {
                    out.push(format!(
                        "AN{} FX send: device {db:.1} dB, TotalControl {:.1} dB",
                        idx + 1,
                        input.fx_send_db
                    ));
                    input.fx_send_db = db;
                }
            }
            if let Some(word) = self.input(idx, 0x06) {
                let phase = word != 0;
                if input.phase != phase {
                    out.push(format!(
                        "AN{} phase: device {phase}, TotalControl {}",
                        idx + 1,
                        input.phase
                    ));
                    input.phase = phase;
                }
            }
            if let Some(word) = self.input(idx, 0x05) {
                let ms = word != 0;
                if input.ms_proc != ms {
                    out.push(format!(
                        "AN{} MS: device {ms}, TotalControl {}",
                        idx + 1,
                        input.ms_proc
                    ));
                    input.ms_proc = ms;
                }
            }
            if let Some(eq) = input.eq.as_mut() {
                apply_eq(&|off| self.input(idx, off), &format!("AN{}", idx + 1), eq, &mut out);
            }
            if let Some(dyn_) = input.dynamics.as_mut() {
                apply_dynamics(&|off| self.input(idx, off), &format!("AN{}", idx + 1), dyn_, &mut out);
            }

            // The dump stores the post-trim, post-pan send.  Compare it
            // with the model's pre-trim fader and pan only for the four
            // mono inputs; stereo pairs use the pair writer's 2x2 matrix
            // and need the width/M-S model to reconstruct.
            if idx < 4 {
                for pair in 0..9 {
                    let Some((left, left_pan)) = self.matrix_at(pair, 0, idx) else {
                        continue;
                    };
                    let Some((right, _)) = self.matrix_at(pair, 1, idx) else {
                        continue;
                    };
                    let volume = input.volumes.get(pair).copied().unwrap_or(0.0);
                    let pan = input.pans.get(pair).copied().unwrap_or(0);
                    let (expected_left, expected_right) = Self::pan_split(volume, pan);
                    let trim = |side: usize| {
                        let no_trim = scene
                            .outputs
                            .get(pair * 2 + side)
                            .is_some_and(|output| output.no_trim);
                        if no_trim {
                            1.0
                        } else {
                            Self::db_to_linear(input.trim)
                        }
                    };
                    let expected_left = expected_left * trim(0);
                    let expected_right = expected_right * trim(1);
                    let device_left = Self::matrix_level(left);
                    let device_right = Self::matrix_level(right);
                    if (device_left - expected_left).abs() > 0.005
                        || (device_right - expected_right).abs() > 0.005
                    {
                        out.push(format!(
                            "AN{} -> output {}: device {:.3}/{:.3}, TotalControl {:.3}/{:.3}",
                            idx + 1,
                            pair,
                            device_left,
                            device_right,
                            expected_left,
                            expected_right
                        ));
                    }
                    if expected_left.max(expected_right) > 0.0005 {
                        let device_pan = Self::matrix_pan(left_pan);
                        if device_pan != pan {
                            out.push(format!(
                                "AN{} -> output {} pan: device {device_pan}, TotalControl {pan}",
                                idx + 1,
                                pair
                            ));
                        }
                    }
                }
            }
        }

        for pair in 0..9 {
            for side in 0..2 {
                let idx = pair * 2 + side;
                let Some(output) = scene.outputs.get_mut(idx) else {
                    continue;
                };
                if let Some(word) = self.output(pair, side, 0x00) {
                    let db = Self::tenths_db(word);
                    let volume = Self::db_to_linear(db);
                    if (output.volume - volume).abs() > 0.005 {
                        out.push(format!(
                            "output {idx} volume: device {db:.1} dB, TotalControl {:.1} dB",
                            linear_db(output.volume)
                        ));
                        output.volume = volume;
                    }
                }
                if let Some(word) = self.output(pair, side, 0x01) {
                    let balance = Self::signed(word).clamp(-100, 100) as i8;
                    if output.balance != balance {
                        out.push(format!(
                            "output {idx} balance: device {balance}, TotalControl {}",
                            output.balance
                        ));
                        output.balance = balance;
                    }
                }
                if let Some(word) = self.output(pair, side, 0x02) {
                    // Bit 0 is mute; bit 1 marks this pair as Main Out.
                    let mute = word & 1 != 0;
                    if output.mute != mute {
                        out.push(format!(
                            "output {idx} mute: device {mute}, TotalControl {}",
                            output.mute
                        ));
                        output.mute = mute;
                    }
                }
                if let Some(word) = self.output(pair, side, 0x03) {
                    let db = Self::tenths_db(word);
                    if (output.fx_return_db - db).abs() > 0.05 {
                        out.push(format!(
                            "output {idx} FX return: device {db:.1} dB, TotalControl {:.1} dB",
                            output.fx_return_db
                        ));
                        output.fx_return_db = db;
                    }
                }
                if let Some(word) = self.output(pair, side, 0x07) {
                    let phase = word != 0;
                    if side == 0 {
                        if output.phase_l != phase {
                            out.push(format!(
                                "output {idx} phase L: device {phase}, TotalControl {}",
                                output.phase_l
                            ));
                            output.phase_l = phase;
                        }
                    } else if output.phase_r != phase {
                        out.push(format!(
                            "output {idx} phase R: device {phase}, TotalControl {}",
                            output.phase_r
                        ));
                        output.phase_r = phase;
                    }
                }
                let label = format!("output {idx}");
                if let Some(eq) = output.eq.as_mut() {
                    apply_eq(&|off| self.output(pair, side, off), &label, eq, &mut out);
                }
                if let Some(dyn_) = output.dynamics.as_mut() {
                    apply_dynamics(&|off| self.output(pair, side, off), &label, dyn_, &mut out);
                }
            }
        }

        if let Some(reverb) = scene.settings.reverb.as_mut() {
            apply_reverb(&self.words, reverb, &mut out);
        }
        if let Some(echo) = scene.settings.echo.as_mut() {
            apply_echo(&self.words, echo, &mut out);
        }

        out
    }

    pub fn apply_to_scene(&self, scene: &mut Scene) -> Vec<String> {
        self.differences_into(scene)
    }
}

fn apply_eq(
    word: &dyn Fn(u16) -> Option<u16>,
    label: &str,
    eq: &mut InputEq,
    out: &mut Vec<String>,
) {
    if let Some(w) = word(0x20) {
        let on = w != 0;
        if eq.low_cut_enabled != on {
            out.push(format!("{label} low cut: device {on}, TotalControl {}", eq.low_cut_enabled));
            eq.low_cut_enabled = on;
        }
    }
    if let Some(w) = word(0x21) {
        if eq.low_cut_freq_hz != w {
            out.push(format!(
                "{label} low cut frequency: device {w} Hz, TotalControl {} Hz",
                eq.low_cut_freq_hz
            ));
            eq.low_cut_freq_hz = w;
        }
    }
    if let Some(w) = word(0x22) {
        let slope = match w {
            0 => 6,
            1 => 12,
            2 => 18,
            3 => 24,
            _ => 6,
        };
        if eq.low_cut_slope_db_oct != slope {
            out.push(format!(
                "{label} low cut slope: device {slope} dB/oct, TotalControl {} dB/oct",
                eq.low_cut_slope_db_oct
            ));
            eq.low_cut_slope_db_oct = slope;
        }
    }
    if let Some(w) = word(0x40) {
        let on = w != 0;
        if eq.enabled != on {
            out.push(format!("{label} EQ: device {on}, TotalControl {}", eq.enabled));
            eq.enabled = on;
        }
    }
    // The band registers keep their last type and values while the EQ is
    // disabled.  They are not part of the active scene then, so a disabled
    // EQ must not report its retained bands as a mismatch.
    if !eq.enabled {
        return;
    }
    for band in 0..3 {
        let (type_off, gain_off, freq_off, q_off) = match band {
            0 => (0x41, 0x42, 0x43, 0x44),
            1 => (0x40, 0x45, 0x46, 0x47),
            _ => (0x48, 0x49, 0x4a, 0x4b),
        };
        if band != 1 {
            if let Some(w) = word(type_off) {
                let kind = match (band, w) {
                    (0, 1) => EqBandType::LowShelf,
                    (2, 1) => EqBandType::HighShelf,
                    (2, 2) => EqBandType::LowPass,
                    _ => EqBandType::Bell,
                };
                if eq.bands[band].band_type != kind {
                    out.push(format!(
                        "{label} EQ band {} type: device {:?}, TotalControl {:?}",
                        band + 1,
                        kind,
                        eq.bands[band].band_type
                    ));
                    eq.bands[band].band_type = kind;
                }
            }
        }
        if let Some(w) = word(gain_off) {
            let db = (w as i16) as f32 / 10.0;
            if (eq.bands[band].gain_db - db).abs() > 0.05 {
                out.push(format!(
                    "{label} EQ band {} gain: device {db:.1} dB, TotalControl {:.1} dB",
                    band + 1,
                    eq.bands[band].gain_db
                ));
                eq.bands[band].gain_db = db;
            }
        }
        if let Some(w) = word(freq_off) {
            if eq.bands[band].freq_hz != w {
                out.push(format!(
                    "{label} EQ band {} frequency: device {w} Hz, TotalControl {} Hz",
                    band + 1,
                    eq.bands[band].freq_hz
                ));
                eq.bands[band].freq_hz = w;
            }
        }
        if let Some(w) = word(q_off) {
            let q = w as f32 / 10.0;
            if (eq.bands[band].q - q).abs() > 0.05 {
                out.push(format!(
                    "{label} EQ band {} Q: device {q:.1}, TotalControl {:.1}",
                    band + 1,
                    eq.bands[band].q
                ));
                eq.bands[band].q = q;
            }
        }
    }
}

fn apply_dynamics(
    word: &dyn Fn(u16) -> Option<u16>,
    label: &str,
    dyn_: &mut Dynamics,
    out: &mut Vec<String>,
) {
    if let Some(w) = word(0x60) {
        let on = w != 0;
        if dyn_.on != on {
            out.push(format!("{label} dynamics: device {on}, TotalControl {}", dyn_.on));
            dyn_.on = on;
        }
    }
    if let Some(w) = word(0x61) {
        let db = (w as i16) as f32 / 10.0;
        if (dyn_.gain_db - db).abs() > 0.05 {
            out.push(format!("{label} dynamics gain: device {db:.1} dB, TotalControl {:.1} dB", dyn_.gain_db));
            dyn_.gain_db = db;
        }
    }
    if let Some(w) = word(0x62) {
        // The wire value is milliseconds + 1.
        let ms = w.saturating_sub(1);
        if dyn_.attack_ms != ms {
            out.push(format!("{label} dynamics attack: device {ms} ms, TotalControl {} ms", dyn_.attack_ms));
            dyn_.attack_ms = ms;
        }
    }
    if let Some(w) = word(0x63) {
        let ms = w.saturating_sub(1);
        if dyn_.release_ms != ms {
            out.push(format!("{label} dynamics release: device {ms} ms, TotalControl {} ms", dyn_.release_ms));
            dyn_.release_ms = ms;
        }
    }
    if let Some(w) = word(0x64) {
        let db = (w as i16) as f32 / 10.0;
        if (dyn_.comp_threshold_db - db).abs() > 0.05 {
            out.push(format!("{label} compressor threshold: device {db:.1} dB, TotalControl {:.1} dB", dyn_.comp_threshold_db));
            dyn_.comp_threshold_db = db;
        }
    }
    if let Some(w) = word(0x65) {
        let ratio = w as f32 / 10.0;
        if (dyn_.comp_ratio - ratio).abs() > 0.05 {
            out.push(format!("{label} compressor ratio: device {ratio:.1}, TotalControl {:.1}", dyn_.comp_ratio));
            dyn_.comp_ratio = ratio;
        }
    }
    if let Some(w) = word(0x66) {
        let db = (w as i16) as f32 / 10.0;
        if (dyn_.exp_threshold_db - db).abs() > 0.05 {
            out.push(format!("{label} expander threshold: device {db:.1} dB, TotalControl {:.1} dB", dyn_.exp_threshold_db));
            dyn_.exp_threshold_db = db;
        }
    }
    if let Some(w) = word(0x67) {
        let ratio = w as f32 / 10.0;
        if (dyn_.exp_ratio - ratio).abs() > 0.05 {
            out.push(format!("{label} expander ratio: device {ratio:.1}, TotalControl {:.1}", dyn_.exp_ratio));
            dyn_.exp_ratio = ratio;
        }
    }
    if let Some(w) = word(0x80) {
        let on = w != 0;
        if dyn_.auto_level != on {
            out.push(format!("{label} AutoLevel: device {on}, TotalControl {}", dyn_.auto_level));
            dyn_.auto_level = on;
        }
    }
    if let Some(w) = word(0x81) {
        let db = w as f32 / 10.0;
        if (dyn_.auto_max_gain_db - db).abs() > 0.05 {
            out.push(format!("{label} AutoLevel max gain: device {db:.1} dB, TotalControl {:.1} dB", dyn_.auto_max_gain_db));
            dyn_.auto_max_gain_db = db;
        }
    }
    if let Some(w) = word(0x82) {
        let db = w as f32 / 10.0;
        if (dyn_.auto_headroom_db - db).abs() > 0.05 {
            out.push(format!("{label} AutoLevel headroom: device {db:.1} dB, TotalControl {:.1} dB", dyn_.auto_headroom_db));
            dyn_.auto_headroom_db = db;
        }
    }
    if let Some(w) = word(0x83) {
        let rise = w as f32 / 10.0;
        if (dyn_.auto_rise_s - rise).abs() > 0.05 {
            out.push(format!("{label} AutoLevel rise: device {rise:.1} s, TotalControl {:.1} s", dyn_.auto_rise_s));
            dyn_.auto_rise_s = rise;
        }
    }
}

fn apply_reverb(words: &HashMap<u16, u16>, reverb: &mut Reverb, out: &mut Vec<String>) {
    let get = |addr: u16| words.get(&addr).copied();
    if let Some(w) = get(0x3c00) {
        let on = w != 0;
        if reverb.on != on {
            out.push(format!("Reverb: device {on}, TotalControl {}", reverb.on));
            reverb.on = on;
        }
    }
    if let Some(w) = get(0x3c01) {
        let kind = w as u8;
        if reverb.kind != kind {
            out.push(format!("Reverb type: device {kind}, TotalControl {}", reverb.kind));
            reverb.kind = kind;
        }
    }
    if let Some(w) = get(0x3c02) {
        if reverb.pre_delay_ms != w {
            out.push(format!("Reverb pre-delay: device {w} ms, TotalControl {} ms", reverb.pre_delay_ms));
            reverb.pre_delay_ms = w;
        }
    }
    if let Some(w) = get(0x3c03) {
        if reverb.low_cut_hz != w {
            out.push(format!("Reverb low cut: device {w} Hz, TotalControl {} Hz", reverb.low_cut_hz));
            reverb.low_cut_hz = w;
        }
    }
    if let Some(w) = get(0x3c04) {
        let scale = w as f32 / 100.0;
        if (reverb.room_scale - scale).abs() > 0.005 {
            out.push(format!("Reverb room scale: device {scale:.2}, TotalControl {:.2}", reverb.room_scale));
            reverb.room_scale = scale;
        }
    }
    if let Some(w) = get(0x3c05) {
        if reverb.attack_ms != w {
            out.push(format!("Reverb attack: device {w} ms, TotalControl {} ms", reverb.attack_ms));
            reverb.attack_ms = w;
        }
    }
    if let Some(w) = get(0x3c06) {
        if reverb.hold_ms != w {
            out.push(format!("Reverb hold: device {w} ms, TotalControl {} ms", reverb.hold_ms));
            reverb.hold_ms = w;
        }
    }
    if let Some(w) = get(0x3c07) {
        if reverb.release_ms != w {
            out.push(format!("Reverb release: device {w} ms, TotalControl {} ms", reverb.release_ms));
            reverb.release_ms = w;
        }
    }
    if let Some(w) = get(0x3c08) {
        if reverb.high_cut_hz != w {
            out.push(format!("Reverb high cut: device {w} Hz, TotalControl {} Hz", reverb.high_cut_hz));
            reverb.high_cut_hz = w;
        }
    }
    if let Some(w) = get(0x3c09) {
        if reverb.reverb_time_ds != w {
            out.push(format!("Reverb time: device {w} (0.1 s), TotalControl {}", reverb.reverb_time_ds));
            reverb.reverb_time_ds = w;
        }
    }
    if let Some(w) = get(0x3c0a) {
        if reverb.high_damp_hz != w {
            out.push(format!("Reverb high damp: device {w} Hz, TotalControl {} Hz", reverb.high_damp_hz));
            reverb.high_damp_hz = w;
        }
    }
    if let Some(w) = get(0x3c0b) {
        let smooth = w as u8;
        if reverb.smoothness != smooth {
            out.push(format!("Reverb smoothness: device {smooth}, TotalControl {}", reverb.smoothness));
            reverb.smoothness = smooth;
        }
    }
    if let Some(w) = get(0x3c0c) {
        let db = (w as i16) as f32 / 10.0;
        if (reverb.volume_db - db).abs() > 0.05 {
            out.push(format!("Reverb volume: device {db:.1} dB, TotalControl {:.1} dB", reverb.volume_db));
            reverb.volume_db = db;
        }
    }
    if let Some(w) = get(0x3c0d) {
        let width = w as f32;
        if (reverb.width - width).abs() > 0.05 {
            out.push(format!("Reverb width: device {width:.2}, TotalControl {:.2}", reverb.width));
            reverb.width = width;
        }
    }
}

fn apply_echo(words: &HashMap<u16, u16>, echo: &mut Echo, out: &mut Vec<String>) {
    let get = |addr: u16| words.get(&addr).copied();
    if let Some(w) = get(0x3c20) {
        let on = w != 0;
        if echo.on != on {
            out.push(format!("Echo: device {on}, TotalControl {}", echo.on));
            echo.on = on;
        }
    }
    if let Some(w) = get(0x3c21) {
        let kind = w as u8;
        if echo.kind != kind {
            out.push(format!("Echo type: device {kind}, TotalControl {}", echo.kind));
            echo.kind = kind;
        }
    }
    if let Some(w) = get(0x3c22) {
        // Register 0x3c22 is 0.01 s units; the model stores milliseconds.
        let ms = w.saturating_mul(10);
        if echo.delay_ms != ms {
            out.push(format!("Echo delay: device {ms} ms, TotalControl {} ms", echo.delay_ms));
            echo.delay_ms = ms;
        }
    }
    if let Some(w) = get(0x3c23) {
        let feedback = w as u8;
        if echo.feedback != feedback {
            out.push(format!("Echo feedback: device {feedback}, TotalControl {}", echo.feedback));
            echo.feedback = feedback;
        }
    }
    if let Some(w) = get(0x3c24) {
        let cut = w as u8;
        if echo.high_cut != cut {
            out.push(format!("Echo high cut: device {cut}, TotalControl {}", echo.high_cut));
            echo.high_cut = cut;
        }
    }
    if let Some(w) = get(0x3c25) {
        let db = (w as i16) as f32 / 10.0;
        if (echo.volume_db - db).abs() > 0.05 {
            out.push(format!("Echo volume: device {db:.1} dB, TotalControl {:.1} dB", echo.volume_db));
            echo.volume_db = db;
        }
    }
    if let Some(w) = get(0x3c26) {
        let width = w as f32;
        if (echo.width - width).abs() > 0.05 {
            out.push(format!("Echo width: device {width:.2}, TotalControl {:.2}", echo.width));
            echo.width = width;
        }
    }
}

fn linear_db(v: f32) -> f32 {
    if v <= 0.0 {
        -65.0
    } else {
        20.0 * v.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{ChannelType, InputChannel, InputEq, OutputChannel};
    use std::collections::HashMap;

    fn scene() -> Scene {
        let mut scene = Scene::new("test");
        for idx in 0..18 {
            scene.inputs.push(InputChannel::new(
                idx,
                &format!("AN{}", idx + 1),
                ChannelType::Line,
                9,
            ));
            scene
                .outputs
                .push(OutputChannel::new(idx, &format!("OUT {idx}")));
        }
        scene
    }

    fn level_word(tenths: i16) -> u16 {
        (tenths.clamp(-650, 60) << 4) as u16
    }

    #[test]
    fn matrix_comparison_applies_trim_and_pan() {
        let mut scene = scene();
        let input = &mut scene.inputs[0];
        input.volumes[1] = 1.0;
        input.pans[1] = -100;
        input.trim = -17.223_293;

        let mut words = HashMap::new();
        // AN1 -> output pair 1 (AN1/2), after trim and hard-left pan.
        words.insert(0x12a0, level_word(-172));
        words.insert(0x12a1, (-100i16) as u16);
        words.insert(0x13a0, 0);
        words.insert(0x13a1, (-100i16) as u16);

        let diff = UcxDump::new(words).differences_into(&mut scene);
        assert!(diff.is_empty(), "{diff:?}");
    }

    #[test]
    fn matrix_comparison_ignores_pan_when_send_is_silent() {
        let mut scene = scene();
        let input = &mut scene.inputs[0];
        input.volumes[1] = 0.0;
        input.pans[1] = 100;

        let mut words = HashMap::new();
        words.insert(0x12a0, 0);
        words.insert(0x12a1, 0);
        words.insert(0x13a0, 0);
        words.insert(0x13a1, 0);

        let diff = UcxDump::new(words).differences_into(&mut scene);
        assert!(diff.is_empty(), "{diff:?}");
    }

    #[test]
    fn saved_dump_decodes_known_fields_and_matrix() {
        let text = include_str!("../../tests/fixtures/ucx-dump-scene.txt");
        let mut words = HashMap::new();
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let addr = u16::from_str_radix(parts.next().unwrap(), 16).unwrap();
            let value = u16::from_str_radix(parts.next().unwrap(), 16).unwrap();
            words.insert(addr, value);
        }
        let dump = UcxDump::new(words);
        assert_eq!(dump.input(0, 0x20), Some(1));
        assert_eq!(dump.input(1, 0x20), Some(1));
        assert_eq!(dump.input(0, 0x40), Some(1));
        assert_eq!(dump.input(1, 0x40), Some(1));
        assert_eq!(dump.input(0, 0x01), Some((-650i16) as u16));
        // 0x18 is Phones, 0x12 is AN1/2.
        assert_eq!(dump.output(0, 0, 0x03), Some((-455i16) as u16));
        assert_eq!(dump.output(1, 0, 0x03), Some(0));
        assert_eq!(dump.output(1, 0, 0x00), Some((-122i16) as u16));
        let (level, pan) = dump.matrix_at(1, 0, 0).unwrap();
        assert_eq!(pan, (-100i16) as u16);
        assert!((UcxDump::matrix_level(level) - 0.000_562).abs() < 1e-6);
    }

    #[test]
    fn linked_pairs_compare_against_the_left_values_and_clamped_fx_return() {
        let mut scene = scene();
        scene.settings.input_pair_link = vec![true, false, false];
        scene.inputs[0].eq = Some(InputEq {
            enabled: true,
            low_cut_enabled: true,
            ..Default::default()
        });
        scene.inputs[1].eq = Some(InputEq::default());
        scene.settings.output_link = vec![true; 9];
        scene.outputs[0].fx_return_db = -45.5;
        scene.outputs[1].fx_return_db = -45.5;
        scene.outputs[2].fx_return_db = 6.0;
        scene.outputs[3].fx_return_db = 6.0;

        let mut words = HashMap::new();
        // Linked AN1/2: both sides carry the left EQ/low-cut state.
        words.insert(0x0020, 1);
        words.insert(0x0120, 1);
        words.insert(0x0040, 1);
        words.insert(0x0140, 1);
        // 0x18 is Phones, 0x12 is AN1/2.  AN1/2's +6.0 dB request is
        // clamped to the unit's 0 dB maximum.
        words.insert(0x1803, (-455i16) as u16);
        words.insert(0x1903, (-455i16) as u16);
        words.insert(0x1203, 0);
        words.insert(0x1303, 0);

        let diff = UcxDump::new(words).differences_into(&mut scene);
        assert!(diff.is_empty(), "{diff:?}");
    }

    #[test]
    fn dump_decodes_dsp_and_globals() {
        let mut scene = scene();
        scene.inputs[0].eq = Some(InputEq::default());
        scene.inputs[0].dynamics = Some(Dynamics::default());
        scene.settings.reverb = Some(Reverb::default());
        scene.settings.echo = Some(Echo::default());
        let mut words = HashMap::new();
        words.insert(0x0020, 1);
        words.insert(0x0021, 120);
        words.insert(0x0022, 2);
        words.insert(0x0040, 1);
        words.insert(0x0041, 1);
        words.insert(0x0042, 30);
        words.insert(0x0043, 100);
        words.insert(0x0044, 10);
        words.insert(0x0060, 1);
        words.insert(0x0061, (-30i16) as u16);
        words.insert(0x0062, 21);
        words.insert(0x0063, 301);
        words.insert(0x0064, (-200i16) as u16);
        words.insert(0x0065, 20);
        words.insert(0x0066, (-600i16) as u16);
        words.insert(0x0067, 15);
        words.insert(0x0080, 1);
        words.insert(0x0081, 60);
        words.insert(0x0082, 80);
        words.insert(0x0083, 50);
        words.insert(0x3c00, 1);
        words.insert(0x3c01, 3);
        words.insert(0x3c02, 50);
        words.insert(0x3c04, 150);
        words.insert(0x3c0d, 75);
        words.insert(0x3c20, 1);
        words.insert(0x3c21, 2);
        words.insert(0x3c22, 70);
        words.insert(0x3c23, 30);
        words.insert(0x3c25, (-120i16) as u16);
        words.insert(0x3c26, 80);

        let diff = UcxDump::new(words).differences_into(&mut scene);
        assert!(!diff.is_empty());
        let eq = scene.inputs[0].eq.unwrap();
        assert!(eq.low_cut_enabled);
        assert_eq!(eq.low_cut_freq_hz, 120);
        assert_eq!(eq.low_cut_slope_db_oct, 18);
        assert_eq!(eq.bands[0].band_type, EqBandType::LowShelf);
        assert!((eq.bands[0].gain_db - 3.0).abs() < 0.01);
        assert_eq!(eq.bands[0].freq_hz, 100);
        assert!((eq.bands[0].q - 1.0).abs() < 0.01);
        let dyn_ = scene.inputs[0].dynamics.unwrap();
        assert!(dyn_.on && dyn_.auto_level);
        assert!((dyn_.gain_db + 3.0).abs() < 0.01);
        assert_eq!(dyn_.attack_ms, 20);
        assert_eq!(dyn_.release_ms, 300);
        assert!((dyn_.comp_ratio - 2.0).abs() < 0.01);
        assert!((dyn_.exp_ratio - 1.5).abs() < 0.01);
        let reverb = scene.settings.reverb.unwrap();
        assert!(reverb.on);
        assert_eq!(reverb.kind, 3);
        assert_eq!(reverb.pre_delay_ms, 50);
        assert!((reverb.room_scale - 1.5).abs() < 0.01);
        assert!((reverb.width - 75.0).abs() < 0.01);
        let echo = scene.settings.echo.unwrap();
        assert!(echo.on);
        assert_eq!(echo.kind, 2);
        assert_eq!(echo.delay_ms, 700);
        assert_eq!(echo.feedback, 30);
        assert!((echo.volume_db + 12.0).abs() < 0.01);
        assert!((echo.width - 80.0).abs() < 0.01);
    }

    #[test]
    fn output_mode_bit_one_is_not_the_output_mute_bit() {
        let mut scene = scene();
        scene.outputs[2].mute = false;
        let mut words = HashMap::new();
        // AN1's 0xNN02 = 2 is monitor-muted.  The dump does not decode the
        // mode word, so it must not be mistaken for output mute bit 0.
        words.insert(0x1202, 2);
        let diff = UcxDump::new(words).differences_into(&mut scene);
        assert!(diff.is_empty(), "{diff:?}");
    }
}
