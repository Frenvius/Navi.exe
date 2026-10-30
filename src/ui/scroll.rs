use super::details::Viewport;
use crate::app::scroll::{lines_from, page_from, wheel_line_limit, wheel_offset, wheel_steps};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Start,
    End,
    Before,
    After,
    Thumb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub axis: Axis,
    pub part: Part,
}

#[derive(Clone, Copy)]
pub struct Bar {
    pub axis: Axis,
    pub length: f32,
    pub maximum: f32,
    pub thumb_size: f32,
    pub thumb_pos: f32,
}

impl Bar {
    pub fn travel(&self) -> f32 {
        (self.length - 34.0 - self.thumb_size).max(0.0)
    }
    pub fn hit(&self, position: f32) -> Hit {
        let part = if position < 17.0 {
            Part::Start
        } else if position >= self.length - 17.0 {
            Part::End
        } else if position < self.thumb_pos {
            Part::Before
        } else if position >= self.thumb_pos + self.thumb_size {
            Part::After
        } else {
            Part::Thumb
        };
        Hit {
            axis: self.axis,
            part,
        }
    }
}

struct Drag {
    axis: Axis,
    pointer: f32,
    value: f32,
}

#[derive(Default)]
pub struct State {
    pub x: f32,
    pub y: f32,
    pub view: Option<Viewport>,
    pub hover: Option<Hit>,
    pub pressed: Option<Hit>,
    pub repeating: bool,
    remainder: i32,
    pointer: (f32, f32),
    drag: Option<Drag>,
    steps: Vec<(f32, f32)>,
    pitch: f32,
    page_units: f32,
    geometry: std::sync::Arc<crate::app::group::DetailsLayout>,
    grid: Option<std::sync::Arc<super::grid::Model>>,
    columns: Vec<f32>,
    unit_starts: Vec<f32>,
    pub gallery_motion: Option<(f32, f32, std::time::Instant)>,
}

impl State {
    pub fn layout_folder(&mut self, view: Viewport, folder: &super::list::Folder) {
        let Some(model) = &folder.grid else {
            self.grid = None;
            self.columns = Vec::new();
            self.unit_starts = Vec::new();
            self.gallery_motion = None;
            self.layout(view, &folder.layout);
            return;
        };
        let cold = self.grid.as_ref().is_none_or(|old| {
            old.layout_id != model.layout_id
                || old.kind != model.kind
                || old.spec.kind != model.spec.kind
                || old.spec.column_major != model.spec.column_major
        });
        let resized = self
            .view
            .is_some_and(|old| old.rows_height != view.rows_height)
            || self.grid.as_ref().is_some_and(|old| {
                old.spec.icon == model.spec.icon && old.layout.columns != model.layout.columns
            });
        let changed = self
            .grid
            .as_ref()
            .is_none_or(|old| !std::sync::Arc::ptr_eq(old, model));
        if changed {
            self.grid = Some(model.clone());
            self.steps = model.layout.items.clone();
            self.pitch = crate::app::grid_layout::page_pitch(&model.spec);
            self.columns = if model.spec.column_major && view.horizontal {
                crate::app::grid_layout::column_lefts(&model.layout.cells)
            } else {
                Vec::new()
            };
            self.unit_starts = if model.kind == super::grid::Kind::Drives {
                let mut starts = self.steps.iter().map(|step| step.0).collect::<Vec<_>>();
                if let Some(first) = starts.first_mut() {
                    *first = 0.0;
                }
                starts.push(model.layout.height);
                starts
            } else {
                Vec::new()
            };
        }
        self.view = Some(view);
        self.x = self.x.clamp(0.0, view.maximum_x()).round();
        self.y = self.y.clamp(0.0, view.maximum_y());
        if self.gallery_motion.is_none() {
            self.y = self.y.round();
        }
        if !self.unit_starts.is_empty() {
            self.page_units = (view.rows_height / crate::app::drives_layout::PITCH * 100.0).ceil();
        } else if cold {
            self.page_units = self.units((view.rows_height - 1.0).max(0.0));
        } else if resized || (view.maximum_y() > 0.0 && self.y >= view.maximum_y()) {
            self.page_units =
                (self.steps.len() as f32 * 100.0 - self.units(view.maximum_y()) - 1.0).max(0.0);
        }
    }

    pub fn animate_gallery(&mut self) -> bool {
        let Some((from, to, start)) = self.gallery_motion else {
            return false;
        };
        let (value, done) = crate::app::gallery::animated_offset(from, to, start.elapsed());
        self.y = value;
        if done {
            self.gallery_motion = None;
        }
        true
    }
    pub fn layout(
        &mut self,
        view: Viewport,
        geometry: &std::sync::Arc<crate::app::group::DetailsLayout>,
    ) {
        let cold = self.view.is_none()
            || !std::sync::Arc::ptr_eq(&self.geometry, geometry)
            || self.pitch != view.pitch;
        let resized = self
            .view
            .is_some_and(|old| old.rows_height != view.rows_height);
        if cold {
            self.steps = geometry.steps(view.plate);
            self.geometry = geometry.clone();
            self.pitch = view.pitch;
        }
        self.x = self.x.clamp(0.0, view.maximum_x()).round();
        self.y = self.y.clamp(0.0, view.maximum_y()).round();
        self.view = Some(view);
        if cold {
            self.page_units = self.units((view.rows_height - 1.0).max(0.0));
        } else if resized || (view.maximum_y() > 0.0 && self.y >= view.maximum_y()) {
            self.page_units =
                (self.steps.len() as f32 * 100.0 - self.units(view.maximum_y()) - 1.0).max(0.0);
        }
    }

    pub fn bar(&self, axis: Axis) -> Option<Bar> {
        let view = self.view?;
        let grid_units = self.grid.is_some();
        let (shown, length, page, extent, maximum, value) = match axis {
            Axis::Vertical => (
                view.bar,
                view.bottom,
                if grid_units {
                    self.page_units
                } else {
                    (view.rows_height / view.pitch).floor()
                },
                if grid_units {
                    self.steps.len() as f32 * 100.0 - 1.0
                } else {
                    self.steps.len() as f32
                },
                view.maximum_y(),
                self.y,
            ),
            Axis::Horizontal => (
                view.horizontal,
                view.right,
                if self.columns.is_empty() {
                    view.right
                } else {
                    1.0
                },
                if self.columns.is_empty() {
                    view.extent_x + 1.0
                } else {
                    self.columns.len() as f32
                },
                view.maximum_x(),
                self.x,
            ),
        };
        if !shown || length < 34.0 {
            return None;
        }
        let thumb_size = ((length - 34.0) * page / extent.max(1.0))
            .round()
            .clamp(17.0, (length - 34.0).max(17.0));
        let thumb_pos = 17.0
            + if maximum > 0.0 {
                let ratio = match axis {
                    Axis::Vertical => self.units(value) / self.maximum_units().max(1.0),
                    Axis::Horizontal => {
                        if self.columns.is_empty() {
                            value / maximum
                        } else {
                            crate::app::grid_layout::column_units(&self.columns, value)
                                / (self.columns.len().saturating_sub(1) as f32 * 48.0).max(1.0)
                        }
                    }
                };
                (ratio * (length - 34.0 - thumb_size)).round()
            } else {
                0.0
            };
        Some(Bar {
            axis,
            length,
            maximum,
            thumb_size,
            thumb_pos,
        })
    }

    pub fn hit(&self, point: (f32, f32)) -> Option<Hit> {
        let view = self.view?;
        let (x, y) = (point.0 - view.x, point.1 - view.y);
        if x < 0.0 || y < 0.0 || x >= view.width || y >= view.height {
            return None;
        }
        if x >= view.right && y < view.bottom {
            return self.bar(Axis::Vertical).map(|bar| bar.hit(y));
        }
        if y >= view.bottom && x < view.right {
            return self.bar(Axis::Horizontal).map(|bar| bar.hit(x));
        }
        None
    }

    pub fn mouse_move(&mut self, point: (f32, f32)) -> bool {
        self.pointer = point;
        let old = (self.x, self.y, self.hover);
        self.hover = self.hit(point);
        if let Some(drag) = &self.drag {
            if let Some(bar) = self.bar(drag.axis) {
                let position = match drag.axis {
                    Axis::Vertical => point.1,
                    Axis::Horizontal => point.0,
                };
                let value = match drag.axis {
                    Axis::Vertical => {
                        let maximum = self.maximum_units();
                        let units = (drag.value
                            + (position - drag.pointer) * maximum / bar.travel().max(1.0))
                        .round()
                        .clamp(0.0, maximum);
                        self.pixels(units).clamp(0.0, bar.maximum)
                    }
                    Axis::Horizontal if !self.columns.is_empty() => {
                        let units = drag.value
                            + (position - drag.pointer)
                                * self.columns.len().saturating_sub(1) as f32
                                * 48.0
                                / bar.travel().max(1.0);
                        crate::app::grid_layout::column_pixels(&self.columns, units)
                            .clamp(0.0, bar.maximum)
                            .round()
                    }
                    Axis::Horizontal => (drag.value
                        + (position - drag.pointer) * bar.maximum / bar.travel().max(1.0))
                    .round()
                    .clamp(0.0, bar.maximum),
                };
                match drag.axis {
                    Axis::Vertical => self.y = value,
                    Axis::Horizontal => self.x = value,
                };
            }
        }
        old != (self.x, self.y, self.hover)
    }

    pub fn leave(&mut self) -> bool {
        let changed = self.hover.is_some();
        self.hover = None;
        changed
    }

    pub fn press(&mut self, point: (f32, f32)) -> bool {
        let Some(hit) = self.hit(point) else {
            return false;
        };
        self.pointer = point;
        self.hover = Some(hit);
        self.pressed = Some(hit);
        self.repeating = false;
        if hit.part == Part::Thumb {
            self.drag = Some(Drag {
                axis: hit.axis,
                pointer: match hit.axis {
                    Axis::Vertical => point.1,
                    Axis::Horizontal => point.0,
                },
                value: match hit.axis {
                    Axis::Vertical => self.units(self.y),
                    Axis::Horizontal => {
                        if self.columns.is_empty() {
                            self.x
                        } else {
                            crate::app::grid_layout::column_units(&self.columns, self.x)
                        }
                    }
                },
            });
        } else {
            self.step(hit);
        }
        true
    }

    pub fn release(&mut self) -> bool {
        let changed = self.pressed.take().is_some();
        self.drag = None;
        self.repeating = false;
        self.hover = self.hit(self.pointer);
        changed
    }

    pub fn tick(&mut self) -> bool {
        if self.pressed.is_none() {
            return false;
        }
        self.repeating = true;
        let old = (self.x, self.y);
        if let Some(hit) = self
            .pressed
            .filter(|hit| self.hover == Some(*hit) && Some(*hit) == self.hit(self.pointer))
        {
            self.step(hit);
        }
        old != (self.x, self.y)
    }

    pub fn band_step(&mut self, axis: Axis, delta: f32) {
        let Some(view) = self.view else { return };
        match axis {
            Axis::Vertical => self.y = (self.y + delta).clamp(0.0, view.maximum_y()).round(),
            Axis::Horizontal => {
                self.shift_horizontal(if self.columns.is_empty() {
                    delta
                } else {
                    delta.signum() * 48.0
                });
            }
        }
    }

    fn shift_horizontal(&mut self, amount: f32) {
        let Some(view) = self.view else { return };
        self.x = if self.columns.is_empty() {
            self.x + amount
        } else {
            crate::app::grid_layout::column_pixels(
                &self.columns,
                crate::app::grid_layout::column_units(&self.columns, self.x) + amount,
            )
        }
        .clamp(0.0, view.maximum_x())
        .round();
    }

    fn step(&mut self, hit: Hit) {
        let Some(view) = self.view else { return };
        let direction = match hit.part {
            Part::Start | Part::Before => -1,
            Part::End | Part::After => 1,
            Part::Thumb => return,
        };
        let page = matches!(hit.part, Part::Before | Part::After);
        match hit.axis {
            Axis::Vertical => {
                self.y = if page {
                    page_from(
                        &self.steps,
                        self.y,
                        view.rows_height,
                        direction,
                        if self.grid.is_some() { 0.0 } else { 1.0 },
                    )
                } else {
                    lines_from(&self.steps, self.y, view.rows_height, direction)
                }
                .clamp(0.0, view.maximum_y())
                .round();
            }
            Axis::Horizontal => {
                let amount = if page {
                    if self.columns.is_empty() {
                        view.right
                    } else {
                        48.0
                    }
                } else {
                    10.0
                };
                self.shift_horizontal(direction as f32 * amount);
            }
        }
    }

    pub fn wheel(&mut self, point: (f32, f32), delta: i32) -> bool {
        let Some(view) = self.view else { return false };
        if point.0 < view.x
            || point.0 >= view.x + view.width
            || point.1 < view.y
            || point.1 >= view.y + view.height
        {
            return false;
        }
        let old = (self.x, self.y);
        let notches = wheel_steps(&mut self.remainder, delta);
        if notches != 0 {
            if self
                .grid
                .as_ref()
                .is_some_and(|model| model.kind == super::grid::Kind::Gallery)
            {
                let target = (self.gallery_motion.map_or(self.y, |motion| motion.1)
                    - notches as f32 * 48.0)
                    .clamp(0.0, view.maximum_y());
                if target != self.y {
                    self.gallery_motion = Some((self.y, target, std::time::Instant::now()));
                    return true;
                }
                return false;
            }
            if self
                .grid
                .as_ref()
                .is_some_and(|model| model.spec.column_major)
            {
                self.x = crate::app::grid_layout::column_offset(&self.columns, self.x, notches)
                    .clamp(0.0, view.maximum_x());
                return old != (self.x, self.y);
            }
            self.y = wheel_offset(
                &self.steps,
                self.y,
                view.rows_height,
                view.maximum_y(),
                if self.grid.is_some() { 0.0 } else { 1.0 },
                if self.grid.is_some() {
                    u32::MAX
                } else {
                    wheel_line_limit(view.rows_height, view.pitch)
                },
                notches,
            )
            .round();
        }
        old != (self.x, self.y)
    }

    pub fn hot(&self, axis: Axis) -> bool {
        self.hover.is_some_and(|hit| hit.axis == axis)
            || self.pressed.is_some_and(|hit| hit.axis == axis)
    }

    fn maximum_units(&self) -> f32 {
        (self.steps.len() as f32 * 100.0 - self.page_units).max(0.0)
    }

    fn units(&self, y: f32) -> f32 {
        if !self.unit_starts.is_empty() {
            return crate::app::drives_layout::units(&self.unit_starts, y);
        }
        let Some(&(first, bottom)) = self.steps.first() else {
            return 0.0;
        };
        if self.steps.get(1).is_none_or(|next| y < next.0) {
            return (y / bottom.max(first + 1.0) * 100.0)
                .floor()
                .clamp(0.0, 100.0);
        }
        let row = self
            .steps
            .partition_point(|&(top, _)| top <= y)
            .saturating_sub(1);
        let (top, bottom) = self.steps[row];
        row as f32 * 100.0
            + ((y - top) / (bottom - top).max(1.0) * 100.0)
                .floor()
                .clamp(0.0, 100.0)
    }

    fn pixels(&self, units: f32) -> f32 {
        if !self.unit_starts.is_empty() {
            return crate::app::drives_layout::pixels(&self.unit_starts, units);
        }
        let row = (units / 100.0).floor() as usize;
        let fraction = units - row as f32 * 100.0;
        let Some(&(top, bottom)) = self.steps.get(row) else {
            return self.steps.last().map_or(0.0, |row| row.1);
        };
        if row == 0 {
            return (fraction * bottom / 100.0).floor();
        }
        top + (fraction * (bottom - top) / 100.0).floor()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn geometry() -> std::sync::Arc<crate::app::group::DetailsLayout> {
        static GEOMETRY: std::sync::OnceLock<std::sync::Arc<crate::app::group::DetailsLayout>> =
            std::sync::OnceLock::new();
        GEOMETRY
            .get_or_init(|| {
                std::sync::Arc::new(crate::app::group::DetailsLayout::new(
                    5019,
                    &[],
                    &Default::default(),
                    21.0,
                    22.0,
                ))
            })
            .clone()
    }
    fn view() -> Viewport {
        Viewport {
            size: (1100.0, 719.0),
            x: 202.0,
            y: 136.0,
            width: 596.0,
            height: 559.0,
            bottom: 542.0,
            right: 579.0,
            rows_height: 517.0,
            header: 25.0,
            extent_x: 625.0,
            extent_y: 105406.0,
            pitch: 21.0,
            plate: 22.0,
            count: 5019,
            bar: true,
            horizontal: true,
            column_maximum: None,
        }
    }

    #[test]
    fn measured_thumb_drag_and_horizontal_caret_use_native_units() {
        let mut state = State::default();
        state.layout(view(), &geometry());
        assert!(state.press((790.0, 160.0)));
        state.mouse_move((790.0, 260.0));
        assert_eq!(state.y, 21368.0);
        state.release();
        assert!(state.press((772.0, 686.0)));
        assert_eq!(state.x, 10.0);
        state.release();
        assert!(state.press((772.0, 686.0)));
        assert_eq!(state.x, 20.0);
    }

    #[test]
    fn held_track_stops_under_pointer_and_late_repeat_stays_stopped() {
        let mut state = State::default();
        state.layout(view(), &geometry());
        assert!(state.press((790.0, 186.0)));
        for _ in 0..12 {
            state.tick();
        }
        let reached = state.y;
        assert!(reached > 510.0 && reached < 5100.0);
        assert!(!state.tick());
        state.release();
        assert!(!state.tick());
        assert!(!state.repeating);
        assert_eq!(state.y, reached);
    }

    #[test]
    fn taller_window_clamps_end_and_shorter_window_keeps_offset() {
        let original = view();
        let mut state = State::default();
        state.layout(original, &geometry());
        state.y = original.maximum_y();
        let mut taller = original;
        taller.rows_height += 173.0;
        taller.bottom += 173.0;
        taller.height += 173.0;
        state.layout(taller, &geometry());
        assert_eq!(state.y, 104716.0);
        state.layout(original, &geometry());
        assert_eq!(state.y, 104716.0);
    }

    #[test]
    fn reaching_end_replaces_the_initial_page_estimate_before_dragging() {
        let view = view();
        let mut state = State::default();
        state.layout(view, &geometry());
        assert_eq!(state.page_units, 2427.0);
        state.y = view.maximum_y();
        state.layout(view, &geometry());
        assert_eq!(state.page_units, 2459.0);
        state.y = 0.0;
        state.layout(view, &geometry());
        assert!(state.press((790.0, 160.0)));
        state.mouse_move((790.0, 260.0));
        assert_eq!(state.y, 21367.0);
    }
}
