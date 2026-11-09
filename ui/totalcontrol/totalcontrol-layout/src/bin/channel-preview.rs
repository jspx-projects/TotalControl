//! Render the standard channel templates to an SVG sheet.
//!
//! ```text
//! cargo run -p totalcontrol-layout --bin channel-preview -- out.svg [scale]
//! ```

use std::fmt::Write as _;
use std::path::PathBuf;

use totalcontrol_layout::{ChannelKind, ColorScheme, Rect, TemplateSpec, TEMPLATES, VU_LABELS};

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("channel-preview.svg"));
    let scale = args
        .next()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(2.0)
        .clamp(0.75, 3.0);

    let svg = sheet(scale);
    if let Err(error) = std::fs::write(&out, svg) {
        eprintln!("could not write {}: {error}", out.display());
        std::process::exit(1);
    }
    println!("wrote {}", out.display());
}

fn sheet(scale: f32) -> String {
    let colors = ColorScheme::default();
    let card_w = 90.0 * scale;
    let card_h = 350.0 * scale;
    let gutter = 24.0 * scale;
    let header = 48.0 * scale;
    let footer = 24.0 * scale;
    let margin = 20.0 * scale;
    let width = margin * 2.0
        + TEMPLATES.len() as f32 * card_w
        + (TEMPLATES.len().saturating_sub(1)) as f32 * gutter;
    let height = margin * 2.0 + header + card_h + footer;

    let mut svg = String::new();
    let _ = write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.1}" height="{height:.1}" viewBox="0 0 {width:.1} {height:.1}">"#
    );
    let _ = write!(
        svg,
        r#"<rect width="100%" height="100%" fill="{}"/>"#,
        colors.surface.app_background
    );
    let _ = write!(
        svg,
        r#"<text x="{margin:.1}" y="{y:.1}" fill="{}" font-family="sans-serif" font-size="{size:.1}" font-weight="600">TotalControl channel templates</text>"#,
        colors.text.primary,
        y = margin + 18.0 * scale,
        size = 15.0 * scale,
    );

    for (index, spec) in TEMPLATES.iter().enumerate() {
        let x = margin + index as f32 * (card_w + gutter);
        let y = margin + header;
        let _ = write!(
            svg,
            r#"<text x="{x:.1}" y="{label_y:.1}" fill="{}" font-family="sans-serif" font-size="{size:.1}">{}</text>"#,
            colors.text.secondary,
            spec.id.label(),
            label_y = y - 8.0 * scale,
            size = 9.0 * scale,
        );
        draw_template(&mut svg, *spec, x, y, scale, &colors);
    }

    let _ = write!(svg, "</svg>");
    svg
}

fn draw_template(
    svg: &mut String,
    spec: TemplateSpec,
    ox: f32,
    oy: f32,
    scale: f32,
    c: &ColorScheme,
) {
    let layout = spec.layout(scale);
    let card = translated(layout.card, ox, oy);
    let active = matches!(spec.kind, ChannelKind::Input | ChannelKind::Playback);
    fill_rect(
        svg,
        card,
        if active {
            c.surface.card_active
        } else {
            c.surface.card
        },
        c.surface.border,
        8.0 * scale,
    );

    draw_text(
        svg,
        translated(layout.name, ox, oy),
        sample_name(spec),
        c.text.primary,
        10.0 * scale,
    );

    let pan = translated(layout.pan, ox, oy);
    let _ = write!(
        svg,
        r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="{face}" stroke="{border}" stroke-width="{sw:.1}"/>"#,
        cx = pan.centre_x(),
        cy = pan.centre_y(),
        r = 20.0 * scale,
        face = "#303138",
        border = c.surface.border,
        sw = 1.0 * scale,
    );
    draw_text(svg, pan, "C", c.text.secondary, 9.0 * scale);

    let labels = if matches!(spec.kind, ChannelKind::Output) {
        ["M", "CUE"]
    } else {
        ["M", "S"]
    };
    for (slot, label) in layout.buttons.iter().zip(labels) {
        let r = translated(*slot, ox, oy);
        fill_rect(svg, r, c.control.button_face, c.surface.border, 3.0 * scale);
        draw_text(svg, r, label, c.text.secondary, 7.5 * scale);
    }

    for (slot, value) in layout.readouts.iter().zip(["-12.3", "-9.8"]) {
        let r = translated(*slot, ox, oy);
        draw_text_aligned(svg, r, value, c.text.secondary, 7.0 * scale, "end");
    }

    let vu = translated(layout.vu, ox, oy);
    fill_rect(svg, vu, "#1e1f23", "#1e1f23", 2.0 * scale);
    let bars = if spec.stereo { 2 } else { 1 };
    let gap = 2.0 * scale;
    let bar_w = if bars == 2 {
        (vu.width - gap) / 2.0
    } else {
        vu.width
    };
    for index in 0..bars {
        let bx = vu.x + index as f32 * (bar_w + gap);
        let bar = Rect::new(bx, vu.y, bar_w, vu.height);
        let level = 0.72 - index as f32 * 0.08;
        let filled = Rect::new(
            bar.x,
            bar.y + bar.height * (1.0 - level),
            bar.width,
            bar.height * level,
        );
        let color = if level > 0.88 {
            c.meter.red
        } else if level > 0.65 {
            c.meter.yellow
        } else {
            c.meter.green
        };
        let _ = write!(
            svg,
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" fill="{color}"/>"#,
            x = filled.x,
            y = filled.y,
            w = filled.width,
            h = filled.height,
        );
    }
    for (db, label) in VU_LABELS {
        let t = db_to_t(db);
        let y = vu.bottom() - vu.height * t;
        let _ = write!(
            svg,
            r#"<text x="{x:.1}" y="{y:.1}" fill="{color}" font-family="sans-serif" font-size="{size:.1}" text-anchor="middle" dominant-baseline="middle">{label}</text>"#,
            x = vu.centre_x(),
            y = y,
            color = c.text.primary,
            size = 5.2 * scale,
        );
    }

    let track = translated(layout.fader, ox, oy);
    let slot_w = 4.3 * scale;
    let slot = Rect::new(
        track.centre_x() - slot_w / 2.0,
        track.y,
        slot_w,
        track.height,
    );
    fill_rect(
        svg,
        slot,
        c.control.fader_slot,
        c.control.fader_slot,
        1.0 * scale,
    );
    let cap_centre = slot.y + slot.height * 0.44;
    let fill = Rect::new(slot.x, cap_centre, slot.width, slot.bottom() - cap_centre);
    fill_rect(svg, fill, c.control.fader_fill, c.control.fader_fill, 0.0);
    let cap = Rect::new(
        track.centre_x() - 11.0 * scale,
        cap_centre - 7.5 * scale,
        22.0 * scale,
        15.0 * scale,
    );
    fill_rect(svg, cap, "#c9cbd1", "#8b8e96", 2.0 * scale);
    // 0 dB tick and the marks enough to show the geometry.
    for db in [-60.0f32, -40.0, -20.0, -10.0, -6.0, 0.0, 3.0] {
        let y = track.bottom() - track.height * db_to_t(db);
        let mark_w = if db == 3.0 { 5.0 * scale } else { 10.0 * scale };
        let _ = write!(
            svg,
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" fill="{color}"/>"#,
            x = track.right() - mark_w,
            y = y - 0.5 * scale,
            w = mark_w,
            h = 1.0 * scale,
            color = if db == 3.0 { c.meter.red } else { "#6b6d75" },
        );
    }

    if let Some(fx) = layout.fx {
        let fx = translated(fx, ox, oy);
        fill_rect(
            svg,
            fx,
            c.control.fader_slot,
            c.control.fader_slot,
            1.0 * scale,
        );
        let fx_fill = Rect::new(
            fx.x,
            fx.y + 16.0 * scale,
            fx.width,
            fx.height - 16.0 * scale,
        );
        let fx_color = if matches!(spec.kind, ChannelKind::Output) {
            c.accent.cue
        } else {
            "#ff9f43"
        };
        fill_rect(svg, fx_fill, fx_color, fx_color, 0.0);
        let fx_cap = Rect::new(
            fx.centre_x() - 5.0 * scale,
            fx.y + 12.0 * scale,
            10.0 * scale,
            8.0 * scale,
        );
        fill_rect(svg, fx_cap, "#c9cbd1", "#8b8e96", 1.0 * scale);
    }

    let tab_labels = ["T", "⚙", "EQ", "D"];
    let tab_present = [
        spec.features.trim,
        true,
        spec.features.eq,
        spec.features.dynamics,
    ];
    for index in 0..4 {
        let face = translated(layout.tab_faces[index], ox, oy);
        let glyph = translated(layout.tab_glyphs[index], ox, oy);
        if tab_present[index] {
            // At rest a tab shows only its glyph; the dark face is used for
            // hover or while its slideout is open.
            let _ = face;
            draw_text(svg, glyph, tab_labels[index], c.text.secondary, 7.5 * scale);
        } else {
            let outline = Rect::new(glyph.x, glyph.y, glyph.width, glyph.height);
            let _ = write!(
                svg,
                r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" fill="none" stroke="{color}" stroke-width="{sw:.1}" rx="{r:.1}"/>"#,
                x = outline.x,
                y = outline.y,
                w = outline.width,
                h = outline.height,
                color = c.surface.border,
                sw = 0.8 * scale,
                r = 2.0 * scale,
            );
        }
    }

    let db = translated(layout.db_readout, ox, oy);
    draw_text(svg, db, "0.0", c.text.primary, 7.0 * scale);
    let minus = translated(layout.collapse_face, ox, oy);
    fill_rect(
        svg,
        minus,
        c.control.button_face,
        c.surface.border,
        3.0 * scale,
    );
    draw_text(svg, minus, "−", c.text.secondary, 8.0 * scale);

    if let Some(submix) = layout.submix {
        let bar = translated(submix, ox, oy);
        fill_rect(
            svg,
            bar,
            c.control.button_face,
            c.surface.border,
            4.0 * scale,
        );
        draw_text(svg, bar, "Phones 7/8", c.text.secondary, 7.0 * scale);
        let _ = write!(
            svg,
            r#"<text x="{x:.1}" y="{y:.1}" fill="{color}" font-family="sans-serif" font-size="{size:.1}" text-anchor="end" dominant-baseline="middle">▸</text>"#,
            x = bar.right() - 5.0 * scale,
            y = bar.centre_y(),
            color = c.text.secondary,
            size = 7.0 * scale,
        );
    }

    let _ = write!(
        svg,
        r#"<text x="{x:.1}" y="{y:.1}" fill="{color}" font-family="sans-serif" font-size="{size:.1}" text-anchor="middle">M1 values refresh 4×/s</text>"#,
        x = card.centre_x(),
        y = card.bottom() + 12.0 * scale,
        color = c.text.secondary,
        size = 7.0 * scale,
    );
}

fn sample_name(spec: TemplateSpec) -> &'static str {
    match spec.id {
        totalcontrol_layout::TemplateId::InputMono => "AN1",
        totalcontrol_layout::TemplateId::InputStereo => "AN1/2",
        totalcontrol_layout::TemplateId::InputMonoNoFx => "AN3",
        totalcontrol_layout::TemplateId::PlaybackStereo => "SW1/2",
        totalcontrol_layout::TemplateId::OutputStereo => "OUT1/2",
        totalcontrol_layout::TemplateId::OutputMono => "OUT3",
        totalcontrol_layout::TemplateId::ControlRoom => "MAIN",
    }
}

fn db_to_t(db: f32) -> f32 {
    // Preview-only approximation of the shared fader scale; the exact
    // mapping lives with the fader widget.
    let floor = -65.0;
    let top = 6.0;
    let t = ((db - floor) / (top - floor)).clamp(0.0, 1.0);
    t.powf(0.85)
}

fn translated(rect: Rect, ox: f32, oy: f32) -> Rect {
    Rect::new(rect.x + ox, rect.y + oy, rect.width, rect.height)
}

fn fill_rect(svg: &mut String, r: Rect, fill: &str, stroke: &str, radius: f32) {
    let _ = write!(
        svg,
        r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" fill="{fill}" stroke="{stroke}" stroke-width="{sw:.1}" rx="{radius:.1}"/>"#,
        x = r.x,
        y = r.y,
        w = r.width,
        h = r.height,
        sw = 1.0,
        radius = radius,
    );
}

fn draw_text(svg: &mut String, r: Rect, text: &str, color: &str, size: f32) {
    draw_text_aligned(svg, r, text, color, size, "middle");
}

fn draw_text_aligned(svg: &mut String, r: Rect, text: &str, color: &str, size: f32, align: &str) {
    let x = match align {
        "end" => r.right() - 2.0,
        "start" => r.x + 2.0,
        _ => r.centre_x(),
    };
    let _ = write!(
        svg,
        r#"<text x="{x:.1}" y="{y:.1}" fill="{color}" font-family="sans-serif" font-size="{size:.1}" text-anchor="{align}" dominant-baseline="middle">{text}</text>"#,
        x = x,
        y = r.centre_y(),
        size = size,
    );
}
