#![allow(dead_code)]

mod difference;
mod lerp;
mod octree;
mod sdf3;
mod sdf_grid3;
mod sphere;
mod union;
mod vector3;

use self::difference::Difference;
use self::octree::{Edge};
use self::sdf3::Sdf3;
use self::sdf_grid3::SdfGrid3;
use self::sphere::Sphere;
use self::vector3::Vector3;

fn main() {
    let a = Sphere {
        center: Vector3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        radius: 0.5,
    };
    let b = Sphere {
        center: Vector3 {
            x: 0.5,
            y: 0.0,
            z: 0.0,
        },
        radius: 0.5,
    };
    let c = Difference(a, b);

    let grid = SdfGrid3::from_sdf(
        &c,
        Vector3 { x: -1.0, y: -1.0, z: -1.0 },
        Vector3 { x: 1.0, y: 1.0, z: 1.0 },
        8,
    );

    let octree = octree::from_grid(&grid);

    let mut vertices = Vec::new();
    let mut faces = Vec::new();

    octree.traverse_leaf_edges(&mut |edge: Edge| {
        if !edge.has_sign_change().unwrap() {
            return;
        }
        let mut generate_face = |v0: Vector3, v1: Vector3, v2: Vector3| {
            let index = vertices.len();
            vertices.push(v0);
            vertices.push(v1);
            vertices.push(v2);
            faces.push((index + 0, index + 1, index + 2));
        };
        let vs = edge.vertices().unwrap();
        match vs.len() {
            3 => {
                generate_face(vs[0], vs[1], vs[2]);
            }
            4 => {
                generate_face(vs[0], vs[1], vs[2]);
                generate_face(vs[2], vs[1], vs[3]);
            }
            _ => {}
        }
    });

    for v in vertices {
        println!("v {} {} {}", v.x, v.y, v.z);
    }
    for f in faces {
        println!("f {} {} {}", f.0 + 1, f.1 + 1, f.2 + 1);
    }
}
