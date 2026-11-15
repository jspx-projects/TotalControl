//! Shared settings-dialog primitives.
//!
//! The GUI spec (§1.1) defines the settings-dialog behaviour once:
//! changes apply as they are made, the dialog stays open, a single
//! right-aligned `Close` dismisses it, and a section that is not enabled
//! is grayed out. These helpers are the single place that behaviour is
//! built, so every settings dialog looks and acts the same:
//!
//! - [`section`], [`help`] — the level-1 title and the muted help line.
//! - [`check`], [`radio_choice`] — controls that gray out when their
//!   section is disabled (no handler → iced `Disabled` → the one
//!   `theme::disabled_text` appearance).
//! - [`action_button`], [`primary_button`], [`footer`] — action buttons
//!   right-aligned in a fixed order; `Cancel` sits left of `OK` when a
//!   dialog needs a confirmation pair.

use iced::widget::{button, checkbox, radio, row, text};
use iced::{Element, Length};

use crate::app::Message;
use crate::theme;

use super::strip;

/// A level-1 section title: the label followed by a hairline rule.
pub fn section(label: &'static str, scale: f32) -> Element<'static, Message> {
    row![
        text(label)
            .color(theme::text_sec())
            .size(theme::TEXT_DIALOG * scale),
        iced::widget::rule::horizontal(1),
    ]
    .spacing(theme::SPACE_MD * scale)
    .align_y(iced::Alignment::Center)
    .into()
}

/// A muted help line. Settings dialogs carry short labels only; the
/// explanations live in the manual.
pub fn help(label: &'static str, scale: f32) -> Element<'static, Message> {
    text(label)
        .color(theme::text_sec())
        .size(theme::TEXT_DIALOG * scale)
        .into()
}

/// A checkbox. With `enabled = false` it has no toggle handler, so iced
/// renders it `Disabled` and it grays out with the section.
pub fn check(
    label: &'static str,
    value: bool,
    message: Message,
    enabled: bool,
    scale: f32,
) -> Element<'static, Message> {
    let mut cb = checkbox(value)
        .label(label)
        .text_size(theme::TEXT_DIALOG * scale)
        .style(theme::checkbox);
    if enabled {
        cb = cb.on_toggle(move |_| message.clone());
    }
    cb.into()
}

/// A radio button, grayed out with its section when `enabled = false`.
/// iced radios have no disabled status, so a disabled choice renders as
/// muted text with a filled/empty dot instead of an interactive control.
pub fn radio_choice(
    label: &'static str,
    selected: bool,
    message: Message,
    enabled: bool,
    scale: f32,
) -> Element<'static, Message> {
    if enabled {
        radio(label, true, Some(selected), move |_| message.clone())
            .size(16.0 * scale)
            .text_size(theme::TEXT_DIALOG * scale)
            .style(theme::radio)
            .into()
    } else {
        text(format!("{} {label}", if selected { "●" } else { "○" }))
            .color(theme::disabled_text())
            .size(theme::TEXT_DIALOG * scale)
            .into()
    }
}

/// A ghost action button (`Close`, `Cancel`, `Apply`, …).
pub fn action_button(
    label: &'static str,
    message: Message,
    scale: f32,
) -> Element<'static, Message> {
    button(strip::centered_label_auto(label, theme::TEXT_DIALOG * scale))
        .padding([theme::SPACE_SM * scale, theme::SPACE_LG * scale])
        .style(theme::ghost_button)
        .on_press(message)
        .into()
}

/// The dialog footer: action buttons right-aligned, in the given order.
/// A settings dialog passes one `Close`; a confirmation pair passes
/// `Cancel` then `OK`.
pub fn footer(
    buttons: Vec<Element<'static, Message>>,
    scale: f32,
) -> Element<'static, Message> {
    let mut r = row![iced::widget::Space::new().width(Length::Fill)]
        .spacing(theme::SPACE_MD * scale)
        .align_y(iced::Alignment::Center)
        .width(Length::Fill);
    for b in buttons {
        r = r.push(b);
    }
    r.into()
}