use crate::object::{number, Objects};
use crate::{PathCommand, Type3Glyph};

pub(crate) fn write(
    objects: &mut Objects,
    glyph: &Type3Glyph,
    advance: u16,
    id: u16,
    images: &mut String,
) -> usize {
    let width = number(f64::from(advance));
    let content = match glyph {
        Type3Glyph::Outline(commands) => {
            let (path, bbox) = outline(commands);
            format!("{width} 0 {} d1\n{path}f\n", bbox.map(number).join(" "))
        }
        Type3Glyph::Image {
            rgba,
            width_px,
            height_px,
            rect,
        } => {
            let expected = u64::from(*width_px)
                .checked_mul(u64::from(*height_px))
                .and_then(|pixels| pixels.checked_mul(4));
            assert!(
                *width_px != 0 && *height_px != 0 && expected == Some(rgba.len() as u64),
                "a Type 3 image takes width_px * height_px * 4 bytes and nonzero dimensions"
            );
            assert!(
                rect.iter().all(|value| value.is_finite())
                    && rect[0] <= rect[2]
                    && rect[1] <= rect[3],
                "a Type 3 image rectangle must have finite, ordered edges"
            );
            let rgb: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
                .collect();
            let mut mask = String::new();
            if rgba.chunks_exact(4).any(|pixel| pixel[3] != 255) {
                let alpha: Vec<u8> = rgba.chunks_exact(4).map(|pixel| pixel[3]).collect();
                let object = objects.reserve();
                objects.stream(
                    object,
                    &format!(
                        "/Type /XObject /Subtype /Image /Width {width_px} /Height {height_px} \
                     /ColorSpace /DeviceGray /BitsPerComponent 8"
                    ),
                    &alpha,
                );
                mask = format!("/SMask {object} 0 R");
            }
            let image = objects.reserve();
            objects.stream(
                image,
                &format!(
                    "/Type /XObject /Subtype /Image /Width {width_px} /Height {height_px} \
                 /ColorSpace /DeviceRGB /BitsPerComponent 8 {mask}"
                ),
                &rgb,
            );
            images.push_str(&format!("/Im{id} {image} 0 R "));
            format!(
                "{width} 0 d0\nq {} 0 0 {} {} {} cm /Im{id} Do Q\n",
                number(f64::from(rect[2]) - f64::from(rect[0])),
                number(f64::from(rect[3]) - f64::from(rect[1])),
                number(f64::from(rect[0])),
                number(f64::from(rect[1])),
            )
        }
    };
    let object = objects.reserve();
    objects.stream(object, "", content.as_bytes());
    object
}

fn outline(commands: &[PathCommand]) -> (String, [f64; 4]) {
    let mut path = String::new();
    let mut bounds = Bounds(None);
    let mut current = None;
    let mut start = [0.0; 2];
    for &command in commands {
        match command {
            PathCommand::MoveTo(x, y) => {
                let point = [f64::from(x), f64::from(y)];
                path.push_str(&format!("{} {} m\n", number(point[0]), number(point[1])));
                bounds.point(point);
                current = Some(point);
                start = point;
            }
            PathCommand::LineTo(x, y) => {
                assert!(
                    current.is_some(),
                    "an outline contour must start with MoveTo"
                );
                let point = [f64::from(x), f64::from(y)];
                path.push_str(&format!("{} {} l\n", number(point[0]), number(point[1])));
                bounds.point(point);
                current = Some(point);
            }
            PathCommand::QuadTo(cx, cy, x, y) => {
                let from = current.expect("an outline contour must start with MoveTo");
                let control = [f64::from(cx), f64::from(cy)];
                let to = [f64::from(x), f64::from(y)];
                let first = std::array::from_fn(|axis| {
                    from[axis] + (control[axis] - from[axis]) * 2.0 / 3.0
                });
                let second =
                    std::array::from_fn(|axis| to[axis] + (control[axis] - to[axis]) * 2.0 / 3.0);
                curve(&mut path, &mut bounds, [from, first, second, to]);
                current = Some(to);
            }
            PathCommand::CurveTo(cx1, cy1, cx2, cy2, x, y) => {
                let from = current.expect("an outline contour must start with MoveTo");
                let to = [f64::from(x), f64::from(y)];
                curve(
                    &mut path,
                    &mut bounds,
                    [
                        from,
                        [f64::from(cx1), f64::from(cy1)],
                        [f64::from(cx2), f64::from(cy2)],
                        to,
                    ],
                );
                current = Some(to);
            }
            PathCommand::Close => {
                assert!(
                    current.is_some(),
                    "an outline contour must start with MoveTo"
                );
                path.push_str("h\n");
                current = Some(start);
            }
        }
    }
    (path, bounds.0.unwrap_or([0.0; 4]))
}

struct Bounds(Option<[f64; 4]>);

impl Bounds {
    fn point(&mut self, point: [f64; 2]) {
        let bounds = self
            .0
            .get_or_insert([point[0], point[1], point[0], point[1]]);
        for axis in 0..2 {
            bounds[axis] = bounds[axis].min(point[axis]);
            bounds[axis + 2] = bounds[axis + 2].max(point[axis]);
        }
    }
}

fn curve(path: &mut String, bounds: &mut Bounds, points: [[f64; 2]; 4]) {
    let [from, first, second, to] = points;
    path.push_str(&format!(
        "{} {} {} {} {} {} c\n",
        number(first[0]),
        number(first[1]),
        number(second[0]),
        number(second[1]),
        number(to[0]),
        number(to[1]),
    ));
    bounds.point(to);
    // A cubic's extrema are its endpoints and the interior roots of its derivative.
    for axis in 0..2 {
        let a = -from[axis] + 3.0 * first[axis] - 3.0 * second[axis] + to[axis];
        let b = 2.0 * (from[axis] - 2.0 * first[axis] + second[axis]);
        let c = first[axis] - from[axis];
        let roots = if a == 0.0 {
            if b == 0.0 {
                [0.0, 0.0]
            } else {
                [-c / b, 0.0]
            }
        } else {
            let discriminant = b * b - 4.0 * a * c;
            if discriminant < 0.0 {
                continue;
            }
            let root = discriminant.sqrt();
            // Avoid cancellation in one root; the other follows from their product.
            let q = -0.5 * (b + root.copysign(b));
            if q == 0.0 {
                [0.0, 0.0]
            } else {
                [q / a, c / q]
            }
        };
        for t in roots {
            if t > 0.0 && t < 1.0 {
                let s = 1.0 - t;
                bounds.point(std::array::from_fn(|axis| {
                    s * s * s * from[axis]
                        + 3.0 * s * s * t * first[axis]
                        + 3.0 * s * t * t * second[axis]
                        + t * t * t * to[axis]
                }));
            }
        }
    }
}
