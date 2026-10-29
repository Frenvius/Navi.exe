use super::sort::{Sort, SortKey};

pub struct Drag {
    pub column: usize,
    pub press_x: f32,
    pub anchor: Option<f32>,
    pub widths: Vec<f32>,
    pub order: Vec<usize>,
    pub dropped: bool,
}

pub struct Position {
    pub ghost: f32,
    pub over: bool,
}

impl Drag {
    pub fn new(column: usize, widths: Vec<f32>, x: f32) -> Option<Self> {
        if column >= widths.len() {
            return None;
        }
        Some(Self {
            column,
            press_x: x,
            anchor: None,
            order: (0..widths.len()).collect(),
            widths,
            dropped: false,
        })
    }

    pub fn lefts(&self) -> Vec<f32> {
        let mut lefts = vec![0.0; self.widths.len()];
        let mut x = 0.0;
        for &i in &self.order {
            lefts[i] = x;
            x += self.widths[i];
        }
        lefts
    }

    pub fn moved(&mut self, x: f32) -> Option<Position> {
        if self.dropped {
            return None;
        }
        let anchor = match self.anchor {
            Some(anchor) => anchor,
            None if (x - self.press_x).abs() > 4.0 => {
                self.anchor = Some(x);
                x
            }
            None => return None,
        };
        let mut right = 0.0;
        let target = self
            .widths
            .iter()
            .position(|width| {
                right += width;
                x <= right
            })
            .unwrap_or(self.widths.len() - 1);
        self.order.retain(|&i| i != self.column);
        self.order.insert(target, self.column);
        let original: f32 = self.widths[..self.column].iter().sum();
        let neighbour = if self.column > 0 {
            self.widths[..self.column - 1].iter().sum()
        } else {
            f32::MIN
        };
        Some(Position {
            ghost: original + x - anchor,
            over: x >= neighbour - 24.0,
        })
    }

    pub fn finish(&mut self) -> Option<Vec<usize>> {
        if self.anchor.is_none() || self.dropped {
            return None;
        }
        self.dropped = true;
        Some(std::mem::take(&mut self.order))
    }
}

pub fn clicked(current: Option<Sort>, key: SortKey) -> Sort {
    Sort {
        key,
        descending: current
            .filter(|sort| sort.key == key)
            .map_or_else(|| key.descending_first(), |sort| !sort.descending),
    }
}

pub fn fit_width(header: f32, value: f32, indent: f32) -> f32 {
    (header + 28.0).max(value + indent + 8.0).max(80.0)
}

pub fn resize_width(start: f32, delta: f32) -> f32 {
    (start + delta).max(80.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drag_threshold_original_slots_and_release_match_measured_header() {
        assert!(Drag::new(4, vec![150.0, 272.0, 120.0, 80.0], 470.0).is_none());
        let mut drag = Drag::new(2, vec![150.0, 272.0, 120.0, 80.0], 470.0).unwrap();
        assert!(drag.moved(466.0).is_none());
        assert_eq!(drag.moved(465.0).unwrap().ghost, 422.0);
        drag.moved(422.0);
        assert_eq!(drag.order, vec![0, 2, 1, 3]);
        assert_eq!(drag.lefts(), vec![0.0, 270.0, 150.0, 542.0]);
        assert_eq!(drag.finish(), Some(vec![0, 2, 1, 3]));
        assert!(drag.moved(600.0).is_none());
        assert!(drag.finish().is_none());
    }
}
