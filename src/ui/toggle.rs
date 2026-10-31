use std::time::{Duration, Instant};

use super::{
    address::{rect, rounded},
    render::Canvas,
    theme::Theme,
};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Frame {
    pub on: f32,
    pub fill: f32,
    pub hover: f32,
    pub press: f32,
}

#[derive(Default)]
pub struct Motion {
    pub frame: Frame,
    from: Frame,
    goal: Frame,
    started: Option<Instant>,
    duration: Duration,
}

impl Motion {
    pub fn target(&mut self, on: bool, hover: bool, press: bool, animate: bool) {
        let next = Frame {
            on: u8::from(on) as f32,
            fill: u8::from(on) as f32,
            hover: u8::from(hover) as f32,
            press: u8::from(press) as f32,
        };
        if !animate {
            self.frame = next;
            self.goal = next;
            self.started = None;
        } else if next != self.goal {
            self.advance(Instant::now());
            self.from = self.frame;
            self.duration = Duration::from_millis(if next.on != self.goal.on { 167 } else { 83 });
            self.goal = next;
            self.started = Some(Instant::now());
        }
    }

    pub fn running(&self) -> bool {
        self.started.is_some()
    }

    pub fn advance(&mut self, now: Instant) -> bool {
        let Some(started) = self.started else {
            return false;
        };
        let time = (now.saturating_duration_since(started).as_secs_f32()
            / self.duration.as_secs_f32())
        .clamp(0.0, 1.0);
        let t = time.cbrt();
        let eased = 3.0 * t * t - 2.0 * time;
        let color = (now.saturating_duration_since(started).as_secs_f32() / 0.083).clamp(0.0, 1.0);
        self.frame = Frame {
            on: self.from.on + (self.goal.on - self.from.on) * eased,
            fill: self.from.fill + (self.goal.fill - self.from.fill) * color,
            hover: self.from.hover + (self.goal.hover - self.from.hover) * eased,
            press: self.from.press + (self.goal.press - self.from.press) * eased,
        };
        if time >= 1.0 {
            self.frame = self.goal;
            self.started = None;
        }
        self.running()
    }
}

pub fn mix(a: D2D1_COLOR_F, b: D2D1_COLOR_F, t: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

pub fn draw(canvas: &Canvas, bounds: D2D_RECT_F, frame: Frame, available: bool, theme: &Theme) {
    let background = theme.settings_card;
    let overlay = |color: D2D1_COLOR_F| {
        let mut result = mix(background, color, color.a);
        result.a = 1.0;
        result
    };
    let mut fill = mix(background, overlay(theme.subtle_hover), frame.hover);
    fill = mix(fill, overlay(theme.control_pressed), frame.press);
    let mut accent = theme.accent;
    accent.a *= 1.0 - frame.hover * 0.1 - frame.press * 0.1;
    accent = overlay(accent);
    let mut border = mix(theme.text_tertiary, accent, frame.fill);
    fill = mix(fill, accent, frame.fill);
    let mut knob = mix(theme.text_secondary, theme.settings_toggle_on, frame.fill);
    if !available {
        border = theme.text_disabled;
        fill = overlay(theme.subtle_pressed);
        knob = theme.text_disabled;
    }
    rounded(canvas, bounds, 10.0, &border);
    rounded(
        canvas,
        rect(bounds.left + 1.0, bounds.top + 1.0, 38.0, 18.0),
        9.0,
        &fill,
    );
    let h = 12.0 + 2.0 * frame.hover.max(frame.press);
    let w = 12.0 + 2.0 * frame.hover + (5.0 - 2.0 * frame.hover) * frame.press;
    let center = 9.5 + 20.0 * frame.on + (2.0 - 3.0 * frame.on) * frame.press;
    rounded(
        canvas,
        rect(
            bounds.left + center - w / 2.0,
            bounds.top + (20.0 - h) / 2.0,
            w,
            h,
        ),
        7.0,
        &knob,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversing_motion_is_continuous_and_finishes_without_idle_work() {
        let mut motion = Motion::default();
        motion.target(true, true, false, true);
        let started = motion.started.unwrap();
        assert!(motion.advance(started + Duration::from_millis(40)));
        assert!(motion.frame.on > 0.0 && motion.frame.on < 1.0);
        motion.target(false, false, true, true);
        let restarted = motion.started.unwrap();
        assert!(!motion.advance(restarted + Duration::from_secs(1)));
        assert_eq!(
            motion.frame,
            Frame {
                on: 0.0,
                fill: 0.0,
                hover: 0.0,
                press: 1.0
            }
        );
        assert!(!motion.advance(restarted + Duration::from_secs(2)));
        motion.target(true, false, false, false);
        assert_eq!(motion.frame.on, 1.0);
        assert!(!motion.running());
    }
}
