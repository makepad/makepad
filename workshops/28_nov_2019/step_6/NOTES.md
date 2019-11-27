* In the file `/step_5/src/lib.rs`, add the following lines:
  
      mod math;
      mod sierpinski;

* In the file `/step_5/src/sierpinski.rs`, add the following lines:
    
      use crate::math::Vec3;

      pub fn sierpinski(level: i32) -> Vec<f32> {
          println!(
              "Generating Sierpinski tetrahedron with level {} in Rust",
              level
          );
          let mut vertices = Vec::new();
          generate_tetrahedron(
              Vec3::new(0.0, 0.0, 1.0),
              Vec3::new(f32::sqrt(8.0 / 9.0), 0.0, -1.0 / 3.0),
              Vec3::new(-f32::sqrt(2.0 / 9.0), f32::sqrt(2.0 / 3.0), -1.0 / 3.0),
              Vec3::new(-f32::sqrt(2.0 / 9.0), -f32::sqrt(2.0 / 3.0), -1.0 / 3.0),
              level,
              &mut vertices,
          );
          vertices
      }

      pub fn generate_tetrahedron(
          p0: Vec3,
          p1: Vec3,
          p2: Vec3,
          p3: Vec3,
          level: i32,
          vertices: &mut Vec<f32>,
      ) {
          if level == 0 {
              generate_triangle(p0, p1, p2, vertices);
              generate_triangle(p0, p2, p3, vertices);
              generate_triangle(p0, p3, p1, vertices);
              generate_triangle(p1, p2, p3, vertices);
          } else {
              let p01 = p0.lerp(p1, 0.5);
              let p02 = p0.lerp(p2, 0.5);
              let p03 = p0.lerp(p3, 0.5);
              let p12 = p1.lerp(p2, 0.5);
              let p23 = p2.lerp(p3, 0.5);
              let p31 = p3.lerp(p1, 0.5);
              generate_tetrahedron(p0, p01, p02, p03, level - 1, vertices);
              generate_tetrahedron(p01, p31, p1, p12, level - 1, vertices);
              generate_tetrahedron(p02, p12, p2, p23, level - 1, vertices);
              generate_tetrahedron(p03, p23, p3, p31, level - 1, vertices);
          }
      }

      fn generate_triangle(p0: Vec3, p1: Vec3, p2: Vec3, vertices: &mut Vec<f32>) {
          vertices.extend([p0.x, p0.y, p0.z].iter().cloned());
          vertices.extend([p1.x, p1.y, p1.z].iter().cloned());
          vertices.extend([p2.x, p2.y, p2.z].iter().cloned());
      }
