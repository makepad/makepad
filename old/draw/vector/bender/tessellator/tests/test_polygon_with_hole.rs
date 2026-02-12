use bender_geometry::linear_path::Command;
use bender_geometry::mesh::{Vertex, Writer};
use bender_geometry::{Mesh, Point};
use bender_tessellator::Tessellator;

#[test]
fn test_polygon_with_hole() {
    let mut mesh = Mesh::new();
    Tessellator::new().tessellate(
        [
            Command::MoveTo(Point::new(-2.0, 0.0)),
            Command::LineTo(Point::new(0.0, -2.0)),
            Command::LineTo(Point::new(2.0, 0.0)),
            Command::LineTo(Point::new(0.0, 2.0)),
            Command::Close,
            Command::MoveTo(Point::new(-1.0, 0.0)),
            Command::LineTo(Point::new(0.0, 1.0)),
            Command::LineTo(Point::new(1.0, 0.0)),
            Command::LineTo(Point::new(0.0, -1.0)),
            Command::Close,
        ]
        .iter()
        .cloned(),
        &mut Writer::new(&mut mesh),
        &mut Vec::new(),
        &mut Vec::new(),
    );
    assert_eq!(
        mesh,
        Mesh {
            vertices: vec![
                Vertex {
                    position: [-2.0, 0.0]
                },
                Vertex {
                    position: [-1.0, 0.0]
                },
                Vertex {
                    position: [0.0, -2.0]
                },
                Vertex {
                    position: [0.0, -1.0]
                },
                Vertex {
                    position: [0.0, 1.0]
                },
                Vertex {
                    position: [0.0, 2.0]
                },
                Vertex {
                    position: [1.0, 0.0]
                },
                Vertex {
                    position: [2.0, 0.0]
                }
            ],
            indices: vec![0, 1, 2, 1, 2, 3, 0, 1, 4, 0, 4, 5, 2, 3, 6, 4, 5, 6, 2, 6, 7, 5, 6, 7]
        }
    );
}
