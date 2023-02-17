use bender_geometry::mesh::Vertex;
use bender_geometry::{Mesh, Point, Polyline, Vector};
use bender_stroker::{CapKind, JoinKind, Stroker};
use bender_viewer::{draw, Draw, Viewer};
use rand::Rng;

fn main() {
    let mut rng = rand::thread_rng();
    let mut positions = Vec::new();
    let mut velocities = Vec::new();
    for _ in 0..20 {
        positions.push(Point::new(
            2.0 * rng.gen::<f32>() - 1.0,
            2.0 * rng.gen::<f32>() - 1.0,
        ));
        velocities.push(Vector::new(
            0.02 * rng.gen::<f32>() - 0.01,
            0.02 * rng.gen::<f32>() - 0.01,
        ));
    }
    let mut viewer = Viewer::new();
    viewer.run(move || {
        for (position, velocity) in positions.iter_mut().zip(velocities.iter_mut()) {
            *position = Point::new(position.x() + velocity.x(), position.y() + velocity.y());
            if position.x() < -1.0 {
                *position = Point::new(-2.0 - position.x(), position.y());
                *velocity = Vector::new(-velocity.x(), velocity.y());
            }
            if 1.0 < position.x() {
                *position = Point::new(2.0 - position.x(), position.y());
                *velocity = Vector::new(-velocity.x(), velocity.y());
            }
            if position.y() < -1.0 {
                *position = Point::new(position.x(), -2.0 - position.y());
                *velocity = Vector::new(velocity.x(), -velocity.y());
            }
            if 1.0 < position.y() {
                *position = Point::new(position.x(), 2.0 - position.y());
                *velocity = Vector::new(-velocity.x(), -velocity.y());
            }
        }
        let polylines = [Polyline {
            vertices: positions.clone(),
        }];
        let mut mesh = Mesh::new();
        Stroker::new().stroke(
            &polylines,
            bender_stroker::Options {
                cap_kind: CapKind::Butt,
                join_kind: JoinKind::Round,
                ..bender_stroker::Options::default()
            },
            &mut mesh,
        );
        Mesh {
            vertices: vec![
                Vertex {
                    position: [-0.5, -0.5],
                },
                Vertex {
                    position: [0.5, -0.5],
                },
                Vertex {
                    position: [-0.5, 0.5],
                },
                Vertex {
                    position: [0.5, 0.5],
                },
            ],
            indices: vec![0, 1, 2, 2, 1, 3],
        }
        .draw(draw::Options {
            triangle_color: [1.0, 0.5, 0.5, 1.0],
            edge_color: [0.0, 0.0, 0.0, 0.0],
            vertex_color: [0.0, 0.0, 0.0, 0.0],
        });
        mesh.draw(draw::Options {
            triangle_color: [0.5, 0.5, 1.0, 0.5],
            vertex_color: [0.0, 0.0, 0.0, 0.0],
            ..draw::Options::default()
        });
    });
}
