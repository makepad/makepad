use bender_filler::{FillRule, Filler};
use bender_geometry::{Mesh, Point, Polygon, Vector};
use bender_viewer::{draw, Draw, Viewer};
use rand::Rng;

fn main() {
    let mut rng = rand::thread_rng();
    let mut positions = Vec::new();
    let mut velocities = Vec::new();
    for _ in 0..25 {
        positions.push(Point::new(
            2.0 * rng.gen::<f32>() - 1.0,
            2.0 * rng.gen::<f32>() - 1.0,
        ));
        velocities.push(Vector::new(
            0.01 * rng.gen::<f32>() - 0.005,
            0.01 * rng.gen::<f32>() - 0.005,
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
        let polygons = [Polygon {
            vertices: positions.clone(),
        }];
        let mut mesh = Mesh::new();
        Filler::new().fill(&polygons, FillRule::EvenOdd, &mut mesh);
        mesh.draw(draw::Options::default());
        for polygon in &polygons {
            polygon.draw(draw::Options::default());
        }
    });
}
