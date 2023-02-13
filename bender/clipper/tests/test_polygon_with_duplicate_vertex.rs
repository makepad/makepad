use bender_clipper::{Clipper, Operation, Options};
use bender_geometry::linear_path::Command;
use bender_geometry::Point;
use bender_internal_iter::InternalIterator;
use std::iter;

#[test]
fn test_polygon_with_duplicate_vertex() {
    assert_eq!(
        Clipper::new()
            .clip(
                Operation::Union,
                [
                    Command::MoveTo(Point::new(-1.0, -1.0)),
                    Command::LineTo(Point::new(0.0, 0.0)),
                    Command::LineTo(Point::new(1.0, -1.0)),
                    Command::LineTo(Point::new(1.0, 1.0)),
                    Command::LineTo(Point::new(0.0, 0.0)),
                    Command::LineTo(Point::new(-1.0, 1.0)),
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
            )
            .collect::<Vec<_>>(),
        vec![
            Command::MoveTo(Point::new(0.0, 0.0)),
            Command::LineTo(Point::new(-1.0, 1.0)),
            Command::LineTo(Point::new(-1.0, -1.0)),
            Command::Close,
            Command::MoveTo(Point::new(1.0, 1.0)),
            Command::LineTo(Point::new(0.0, 0.0)),
            Command::LineTo(Point::new(1.0, -1.0)),
            Command::Close,
        ]
    );
}
