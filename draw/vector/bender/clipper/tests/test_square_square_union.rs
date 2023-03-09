use bender_clipper::{Clipper, Operation, Options};
use bender_geometry::linear_path::Command;
use bender_geometry::Point;
use bender_internal_iter::InternalIterator;

#[test]
fn test_square_square_union() {
    assert_eq!(
        Clipper::new()
            .clip(
                Operation::Union,
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
            Command::MoveTo(Point::new(3.0, 3.0)),
            Command::LineTo(Point::new(-1.0, 3.0)),
            Command::LineTo(Point::new(-1.0, 1.0)),
            Command::LineTo(Point::new(-3.0, 1.0)),
            Command::LineTo(Point::new(-3.0, -3.0)),
            Command::LineTo(Point::new(1.0, -3.0)),
            Command::LineTo(Point::new(1.0, -1.0)),
            Command::LineTo(Point::new(3.0, -1.0)),
            Command::Close
        ]
    );
}

/*
#[test]
fn test_square_square_union() {
    let mut polygons = Vec::new();
    Clipper::new().clip_polygons(
        Operation::Union,
        &[Polygon {
            vertices: vec![
                Point::new(-3.0, -3.0),
                Point::new(1.0, -3.0),
                Point::new(1.0, 1.0),
                Point::new(-3.0, 1.0),
            ],
        }],
        &[Polygon {
            vertices: vec![
                Point::new(-1.0, -1.0),
                Point::new(3.0, -1.0),
                Point::new(3.0, 3.0),
                Point::new(-1.0, 3.0),
            ],
        }],
        Options::default(),
        &mut polygons,
    );
    assert_eq!(
        polygons,
        [Polygon {
            vertices: vec![
                Point::new(-1.0, 3.0),
                Point::new(-1.0, 1.0),
                Point::new(-3.0, 1.0),
                Point::new(-3.0, -3.0),
                Point::new(1.0, -3.0),
                Point::new(1.0, -1.0),
                Point::new(3.0, -1.0),
                Point::new(3.0, 3.0)
            ]
        }]
    );
}
*/
