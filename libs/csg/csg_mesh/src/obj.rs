// OBJ file I/O (simple subset - vertices and faces only)

use crate::mesh::TriMesh;
use makepad_csg_math::{dvec3, Vec3d};
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};

/// Write a mesh to OBJ format.
pub fn write_obj(mesh: &TriMesh, path: &str) -> io::Result<()> {
    let f = File::create(path)?;
    let mut w = BufWriter::new(f);

    writeln!(w, "# CSG mesh output")?;
    writeln!(w, "# Vertices: {}", mesh.vertex_count())?;
    writeln!(w, "# Triangles: {}", mesh.triangle_count())?;

    for v in &mesh.vertices {
        writeln!(w, "v {} {} {}", v.x, v.y, v.z)?;
    }

    // OBJ uses 1-based indices
    for &[a, b, c] in &mesh.triangles {
        writeln!(w, "f {} {} {}", a + 1, b + 1, c + 1)?;
    }

    w.flush()?;
    Ok(())
}

/// Read a mesh from OBJ format (simple: only v and f lines, triangulated faces).
pub fn read_obj(path: &str) -> io::Result<TriMesh> {
    let f = File::open(path)?;
    let reader = BufReader::new(f);
    let mut mesh = TriMesh::new();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        match parts[0] {
            "v" if parts.len() >= 4 => {
                let x: f64 = parts[1].parse().unwrap_or(0.0);
                let y: f64 = parts[2].parse().unwrap_or(0.0);
                let z: f64 = parts[3].parse().unwrap_or(0.0);
                mesh.add_vertex(dvec3(x, y, z));
            }
            "f" if parts.len() >= 4 => {
                // Parse face indices (OBJ is 1-based, may have v/vt/vn format)
                let vert_count = mesh.vertex_count() as u32;
                let indices: Vec<u32> = parts[1..]
                    .iter()
                    .filter_map(|s| {
                        // Handle v/vt/vn format: take only the vertex index
                        let idx_str = s.split('/').next()?;
                        let i = idx_str.parse::<u32>().ok()?;
                        if i == 0 || i > vert_count {
                            return None; // invalid index
                        }
                        Some(i - 1) // 1-based to 0-based
                    })
                    .collect();

                // Fan triangulate if more than 3 vertices
                if indices.len() >= 3 {
                    for i in 1..indices.len() - 1 {
                        mesh.add_triangle(indices[0], indices[i], indices[i + 1]);
                    }
                }
            }
            _ => {} // skip other lines
        }
    }

    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_unit_cube;
    use std::fs;

    #[test]
    fn test_obj_roundtrip() {
        let cube = make_unit_cube();
        let path = "/tmp/csg_test_cube.obj";
        write_obj(&cube, path).unwrap();

        let loaded = read_obj(path).unwrap();
        assert_eq!(loaded.vertex_count(), 8);
        assert_eq!(loaded.triangle_count(), 12);

        let bb = loaded.bounding_box();
        assert!((bb.min.x - (-0.5)).abs() < 1e-12);
        assert!((bb.max.x - 0.5).abs() < 1e-12);

        fs::remove_file(path).ok();
    }
}
