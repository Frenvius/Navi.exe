fn area(triangle: [[f64; 2]; 3], x: usize, y: usize) -> f64 {
    let mut points = [[0.0; 2]; 8];
    points[..3].copy_from_slice(&triangle);
    let mut count = 3;
    for (axis, edge, direction) in [
        (0, x as f64, 1.0),
        (0, x as f64 + 1.0, -1.0),
        (1, y as f64, 1.0),
        (1, y as f64 + 1.0, -1.0),
    ] {
        if count == 0 {
            return 0.0;
        }
        let mut clipped = [[0.0; 2]; 8];
        let mut size = 0;
        let mut previous = points[count - 1];
        let mut distance = direction * (previous[axis] - edge);
        for current in &points[..count] {
            let next_distance = direction * (current[axis] - edge);
            if (distance >= 0.0) != (next_distance >= 0.0) {
                let t = distance / (distance - next_distance);
                clipped[size] = [
                    previous[0] + t * (current[0] - previous[0]),
                    previous[1] + t * (current[1] - previous[1]),
                ];
                size += 1;
            }
            if next_distance >= 0.0 {
                clipped[size] = *current;
                size += 1;
            }
            previous = *current;
            distance = next_distance;
        }
        points = clipped;
        count = size;
    }
    let mut result = 0.0;
    for index in 0..count {
        let a = points[index];
        let b = points[(index + 1) % count];
        result += a[0] * b[1] - b[0] * a[1];
    }
    result.abs() * 0.5
}

pub fn triangle_rect(points: [[f64; 2]; 3], size: (usize, usize), pixels: &mut [f64]) {
    debug_assert_eq!(pixels.len(), size.0 * size.1);
    let first = |axis: usize| {
        points
            .iter()
            .map(|point| point[axis])
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0)
            .min(if axis == 0 { size.0 } else { size.1 } as f64) as usize
    };
    let last = |axis: usize| {
        points
            .iter()
            .map(|point| point[axis])
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .max(0.0)
            .min(if axis == 0 { size.0 } else { size.1 } as f64) as usize
    };
    for y in first(1)..last(1) {
        for x in first(0)..last(0) {
            pixels[y * size.0 + x] += area(points, x, y);
        }
    }
}

pub fn triangle(points: [[f64; 2]; 3], side: usize, pixels: &mut [f64]) {
    triangle_rect(points, (side, side), pixels);
}

#[windows::core::implement(windows::Win32::Graphics::Direct2D::ID2D1TessellationSink)]
struct Triangles {
    data: std::rc::Rc<std::cell::RefCell<Vec<windows::Win32::Graphics::Direct2D::D2D1_TRIANGLE>>>,
}

impl windows::Win32::Graphics::Direct2D::ID2D1TessellationSink_Impl for Triangles_Impl {
    fn AddTriangles(
        &self,
        triangles: *const windows::Win32::Graphics::Direct2D::D2D1_TRIANGLE,
        count: u32,
    ) {
        if count > 0 {
            self.data.borrow_mut().extend_from_slice(unsafe {
                std::slice::from_raw_parts(triangles, count as usize)
            });
        }
    }
    fn Close(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

pub fn triangles(
    geometry: &windows::Win32::Graphics::Direct2D::ID2D1Geometry,
    tolerance: f32,
) -> windows::core::Result<Vec<windows::Win32::Graphics::Direct2D::D2D1_TRIANGLE>> {
    let data = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink: windows::Win32::Graphics::Direct2D::ID2D1TessellationSink =
        Triangles { data: data.clone() }.into();
    unsafe {
        geometry.Tessellate(None, tolerance, &sink)?;
        sink.Close()?;
    }
    drop(sink);
    Ok(data.take())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complementary_triangles_cover_a_pixel_without_a_seam() {
        let mut pixels = [0.0];
        triangle([[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], 1, &mut pixels);
        assert_eq!(pixels[0], 0.5);
        triangle([[1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], 1, &mut pixels);
        assert_eq!(pixels[0], 1.0);
    }

    #[test]
    fn fractional_triangle_conserves_area_in_either_winding() {
        let points = [[0.25, 0.5], [3.75, 0.5], [0.25, 3.5]];
        let mut forward = [0.0; 16];
        let mut reverse = [0.0; 16];
        triangle(points, 4, &mut forward);
        triangle([points[2], points[1], points[0]], 4, &mut reverse);
        assert!((forward.iter().sum::<f64>() - 5.25).abs() < 1e-12);
        for (a, b) in forward.iter().zip(reverse) {
            assert!((*a - b).abs() < 1e-12);
        }
    }

    #[test]
    fn outside_triangles_are_clipped_to_the_image() {
        let mut pixels = [0.0];
        triangle([[-1.0, -1.0], [3.0, -1.0], [-1.0, 3.0]], 1, &mut pixels);
        assert_eq!(pixels[0], 1.0);
        triangle([[2.0, 2.0], [3.0, 2.0], [2.0, 3.0]], 1, &mut pixels);
        assert_eq!(pixels[0], 1.0);
    }
}
