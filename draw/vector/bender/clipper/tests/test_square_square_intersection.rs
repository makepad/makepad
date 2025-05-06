use bender_clipper::{Clipper, Operation, Options};
use bender_geometry::linear_path::Command;
use bender_geometry::Point;
use bender_internal_iter::InternalIterator;

#[test]
fn test_square_square_intersection() {
    assert_eq!(
        Clipper::new()
            .clip(
                Operation::Intersection,
                [
                    Command::MoveTo(Point::new(-3.0, -3.0)),
                    Command::LineTo(Point::new(1.0, -3.0)),
                    Command::LineTo(Point::new(1.0, 1.0)),
                    Command::LineTo(Point::new(-3.0, 1.0)),
                    Command::Close,
                ]
                .iter()
                .cloned(),
                [
                    Command::MoveTo(Point::new(-1.0, -1.0)),
                    Command::LineTo(Point::new(3.0, -1.0)),
                    Command::LineTo(Point::new(3.0, 3.0)),
                    Command::LineTo(Point::new(-1.0, 3.0)),
                    Command::Close,
                ]
                .iter()
                .cloned(),
                Options::default(),
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .collect::<Vec<_>>(),
        [
            Command::MoveTo(Point::new(1.0, 1.0)),
            Command::LineTo(Point::new(-1.0, 1.0)),
            Command::LineTo(Point::new(-1.0, -1.0)),
            Command::LineTo(Point::new(1.0, -1.0)),
            Command::Close
        ]
    );
}
