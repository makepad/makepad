use bender_clipper::{Clipper, Operation, Options};
use bender_geometry::linear_path::Command;
use bender_geometry::Point;
use std::iter;

#[test]
fn test_complex_polygon() {
    Clipper::new().clip(
        Operation::Union,
        [
            Command::MoveTo(Point::new(-0.26476923, 0.34974113)),
            Command::LineTo(Point::new(0.8653693, 0.587213)),
            Command::LineTo(Point::new(-0.8474376, 0.0004683449)),
            Command::LineTo(Point::new(-0.06355343, 0.7060585)),
            Command::LineTo(Point::new(-0.31334308, -0.32070628)),
            Command::LineTo(Point::new(-0.44999534, -0.62726384)),
            Command::LineTo(Point::new(0.33320078, -0.82618576)),
            Command::LineTo(Point::new(-0.25630552, -0.6764601)),
            Command::Close,
        ]
        .iter()
        .cloned(),
        iter::empty(),
        Options::default(),
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
    );
}
