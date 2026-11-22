//! A horizontal fine-value slider for Audio Configuration.
//!
//! Drag to move, Shift-drag for fine movement, wheel to step (Shift-wheel
//! finer), double-click to reset. The interaction matches the rotary knobs
//! and faders; only the drawing is a horizontal track.

use iced::widget::canvas::{self, Canvas, Geometry, Path, Stroke};
use iced::{mouse, Color, Element, Length, Point, Rectangle, Size};
use std::time::Instant;

use crate::theme;

const TRACK_H: f32 = 4.0;
const HANDLE_R: f32 = 6.0;
const DOUBLE_CLICK_MS: u128 = 300;

#[derive(Default)]
pub struct State {
    dragging: bool,
    drag_t: Option<f32>,
    drag_pos: Option<f32>,
    last_click: Option<Instant>,
}

pub struct PitchSlider<Message> {
    pub value: f32,
    pub range: (f32, f32),
    pub step: f32,
    pub scale: f32,
    pub on_change: Box<dyn Fn(f32) -> Message>,
    pub on_reset: Box<dyn Fn() -> Message>,
}

fn value_to_t(value: f32, range: (f32, f32)) -> f32 {
    ((value - range.0) / (range.1 - range.0)).clamp(0.0, 1.0)
}

fn t_to_value(t: f32, range: (f32, f32)) -> f32 {
    range.0 + t.clamp(0.0, 1.0) * (range.1 - range.0)
}

fn quantise(value: f32, step: f32, range: (f32, f32)) -> f32 {
    let step = step.max(0.0001);
    let snapped = (value / step).round() * step;
    snapped.clamp(range.0, range.1)
}

impl<Message> canvas::Program<Message> for PitchSlider<Message> {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let track_left = bounds.x + HANDLE_R * self.scale;
        let track_w = (bounds.width - 2.0 * HANDLE_R * self.scale).max(1.0);
        let locate = |x: f32| t_to_value((x - track_left) / track_w, self.range);
        // The pitch slider has no Shift fine mode: 0.1 % steps are already
        // fine enough, and a finer step makes the wheel unusably slow.
        let step = self.step;

        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let pos = cursor.position_over(bounds)?;
                let now = Instant::now();
                let is_double = state
                    .last_click
                    .is_some_and(|t| now.duration_since(t).as_millis() < DOUBLE_CLICK_MS);
                if is_double {
                    state.dragging = false;
                    state.drag_t = None;
                    state.drag_pos = None;
                    state.last_click = None;
                    return Some(canvas::Action::publish((self.on_reset)()).and_capture());
                }
                state.last_click = Some(now);
                state.dragging = true;
                state.drag_pos = Some(pos.x);
                let value = quantise(locate(pos.x), step, self.range);
                state.drag_t = Some(value_to_t(value, self.range));
                Some(canvas::Action::publish((self.on_change)(value)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if !state.dragging {
                    return None;
                }
                let pos = cursor.land().position()?;
                let prev = state.drag_pos.unwrap_or(pos.x);
                let base_t = state
                    .drag_t
                    .unwrap_or_else(|| value_to_t(self.value, self.range));
                let delta = (pos.x - prev) / track_w;
                let t = (base_t + delta).clamp(0.0, 1.0);
                let value = quantise(t_to_value(t, self.range), step, self.range);
                state.drag_pos = Some(pos.x);
                state.drag_t = Some(value_to_t(value, self.range));
                Some(canvas::Action::publish((self.on_change)(value)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if !state.dragging {
                    return None;
                }
                state.dragging = false;
                state.drag_t = None;
                state.drag_pos = None;
                Some(canvas::Action::capture())
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let _ = cursor.position_over(bounds)?;
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / 20.0,
                };
                if dy == 0.0 {
                    return None;
                }
                let value = quantise(self.value + dy * step, step, self.range);
                Some(canvas::Action::publish((self.on_change)(value)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let center_y = bounds.height / 2.0;
        let left = HANDLE_R * self.scale;
        let width = (bounds.width - 2.0 * HANDLE_R * self.scale).max(1.0);
        let t = value_to_t(self.value, self.range);
        let zero_t = value_to_t(0.0, self.range);
        let x = left + t * width;
        let zero_x = left + zero_t * width;

        frame.fill_rectangle(
            Point::new(left, center_y - TRACK_H * self.scale / 2.0),
            Size::new(width, TRACK_H * self.scale),
            theme::border(),
        );
        let (fill_left, fill_right) = if x >= zero_x {
            (zero_x, x)
        } else {
            (x, zero_x)
        };
        frame.fill_rectangle(
            Point::new(fill_left, center_y - TRACK_H * self.scale / 2.0),
            Size::new((fill_right - fill_left).max(1.0), TRACK_H * self.scale),
            theme::ACCENT,
        );
        frame.fill(
            &Path::circle(Point::new(x, center_y), HANDLE_R * self.scale),
            Color::WHITE,
        );
        frame.stroke(
            &Path::circle(Point::new(x, center_y), HANDLE_R * self.scale),
            Stroke::default()
                .with_color(theme::border())
                .with_width(1.0 * self.scale),
        );
        vec![frame.into_geometry()]
    }
}

pub fn pitch_slider(
    value: f32,
    scale: f32,
    on_change: impl Fn(f32) -> crate::app::Message + 'static,
    on_reset: impl Fn() -> crate::app::Message + 'static,
) -> Element<'static, crate::app::Message> {
    Canvas::new(PitchSlider {
        value,
        range: (-5.0, 5.0),
        step: 0.1,
        scale,
        on_change: Box::new(on_change),
        on_reset: Box::new(on_reset),
    })
    .width(Length::Fill)
    .height(Length::Fixed(24.0 * scale))
    .into()
}
