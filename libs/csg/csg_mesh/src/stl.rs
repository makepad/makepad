// STL file I/O (binary and ASCII)
// Binary STL is the standard format for 3D printing.

use crate::mesh::TriMesh;
use makepad_csg_math::{dvec3, Vec3d};
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};

/// Write a mesh to binary STL.
pub fn write_stl_binary(mesh: &TriMesh, path: &str) -> io::Result<()> {
    let f = File::create(path)?;
    let mut w = BufWriter::new(f);

    // 80-byte header
    let header = [0u8; 80];
    w.write_all(&header)?;

    // Number of triangles
    let num_tris = mesh.triangle_count() as u32;
    w.write_all(&num_tris.to_le_bytes())?;

    for i in 0..mesh.triangle_count() {
        let normal = mesh.triangle_normal(i);
        let (va, vb, vc) = mesh.triangle_vertices(i);

        // Normal (3 floats)
        write_f32(&mut w, normal.x as f32)?;
        write_f32(&mut w, normal.y as f32)?;
        write_f32(&mut w, normal.z as f32)?;

        // Vertex 1
        write_f32(&mut w, va.x as f32)?;
        write_f32(&mut w, va.y as f32)?;
        write_f32(&mut w, va.z as f32)?;
        // Vertex 2
        write_f32(&mut w, vb.x as f32)?;
        write_f32(&mut w, vb.y as f32)?;
        write_f32(&mut w, vb.z as f32)?;
        // Vertex 3
        write_f32(&mut w, vc.x as f32)?;
        write_f32(&mut w, vc.y as f32)?;
        write_f32(&mut w, vc.z as f32)?;

        // Attribute byte count
        w.write_all(&0u16.to_le_bytes())?;
    }

    w.flush()?;
    Ok(())
}

/// Read a mesh from binary STL.
/// Note: binary STL stores unindexed triangles, so vertices will be duplicated.
/// Call weld_vertices() after reading to merge shared vertices.
pub fn read_stl_binary(path: &str) -> io::Result<TriMesh> {
    let mut f = File::open(path)?;

    // Skip 80-byte header
    let mut header = [0u8; 80];
    f.read_exact(&mut header)?;

    // Number of triangles
    let mut buf4 = [0u8; 4];
    f.read_exact(&mut buf4)?;
    let num_tris = u32::from_le_bytes(buf4) as usize;

    let mut mesh = TriMesh::with_capacity(num_tris * 3, num_tris);

    for _ in 0..num_tris {
        // Skip normal (3 floats = 12 bytes)
        let _nx = read_f32(&mut f)?;
        let _ny = read_f32(&mut f)?;
        let _nz = read_f32(&mut f)?;

        let v0 = dvec3(
            read_f32(&mut f)? as f64,
            read_f32(&mut f)? as f64,
            read_f32(&mut f)? as f64,
        );
        let v1 = dvec3(
            read_f32(&mut f)? as f64,
            read_f32(&mut f)? as f64,
            read_f32(&mut f)? as f64,
        );
        let v2 = dvec3(
            read_f32(&mut f)? as f64,
            read_f32(&mut f)? as f64,
            read_f32(&mut f)? as f64,
        );

        let a = mesh.add_vertex(v0);
        let b = mesh.add_vertex(v1);
        let c = mesh.add_vertex(v2);
        mesh.add_triangle(a, b, c);

        // Skip attribute byte count
        let mut attr = [0u8; 2];
        f.read_exact(&mut attr)?;
    }

    Ok(mesh)
}

/// Write a mesh to ASCII STL.
pub fn write_stl_ascii(mesh: &TriMesh, path: &str) -> io::Result<()> {
    let f = File::create(path)?;
    let mut w = BufWriter::new(f);

    writeln!(w, "solid mesh")?;
    for i in 0..mesh.triangle_count() {
        let n = mesh.triangle_normal(i);
        let (va, vb, vc) = mesh.triangle_vertices(i);
        writeln!(
            w,
            "  facet normal {} {} {}",
            n.x as f32, n.y as f32, n.z as f32
        )?;
        writeln!(w, "    outer loop")?;
        writeln!(
            w,
            "      vertex {} {} {}",
            va.x as f32, va.y as f32, va.z as f32
        )?;
        writeln!(
            w,
            "      vertex {} {} {}",
            vb.x as f32, vb.y as f32, vb.z as f32
        )?;
        writeln!(
            w,
            "      vertex {} {} {}",
            vc.x as f32, vc.y as f32, vc.z as f32
        )?;
        writeln!(w, "    endloop")?;
        writeln!(w, "  endfacet")?;
    }
    writeln!(w, "endsolid mesh")?;

    w.flush()?;
    Ok(())
}

/// Read a mesh from ASCII STL.
pub fn read_stl_ascii(path: &str) -> io::Result<TriMesh> {
    let f = File::open(path)?;
    let reader = BufReader::new(f);
    let mut mesh = TriMesh::new();
    let mut verts_in_facet: Vec<Vec3d> = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();

        if trimmed.starts_with("vertex") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let x: f64 = parts[1].parse().map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("bad STL vertex x: {}", e),
                    )
                })?;
                let y: f64 = parts[2].parse().map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("bad STL vertex y: {}", e),
                    )
                })?;
                let z: f64 = parts[3].parse().map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("bad STL vertex z: {}", e),
                    )
                })?;
                verts_in_facet.push(dvec3(x, y, z));
            }
        } else if trimmed.starts_with("endfacet") {
            if verts_in_facet.len() == 3 {
                let a = mesh.add_vertex(verts_in_facet[0]);
                let b = mesh.add_vertex(verts_in_facet[1]);
                let c = mesh.add_vertex(verts_in_facet[2]);
                mesh.add_triangle(a, b, c);
            }
            verts_in_facet.clear();
        }
    }

    Ok(mesh)
}

fn write_f32<W: Write>(w: &mut W, v: f32) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn read_f32<R: Read>(r: &mut R) -> io::Result<f32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(f32::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_unit_cube;
    use std::fs;

    #[test]
    fn test_stl_binary_roundtrip() {
        let cube = make_unit_cube();
        let path = "/tmp/csg_test_cube.stl";
        write_stl_binary(&cube, path).unwrap();

        let loaded = read_stl_binary(path).unwrap();
        assert_eq!(loaded.triangle_count(), 12);
        // Binary STL is unindexed: 12 triangles * 3 = 36 vertices
        assert_eq!(loaded.vertex_count(), 36);

        // After welding, should have 8 vertices
        let mut welded = loaded;
        welded.weld_vertices(0.001);
        assert_eq!(welded.vertex_count(), 8);
        assert_eq!(welded.triangle_count(), 12);

        fs::remove_file(path).ok();
    }

    #[test]
    fn test_stl_ascii_roundtrip() {
        let cube = make_unit_cube();
        let path = "/tmp/csg_test_cube_ascii.stl";
        write_stl_ascii(&cube, path).unwrap();

        let loaded = read_stl_ascii(path).unwrap();
        assert_eq!(loaded.triangle_count(), 12);

        fs::remove_file(path).ok();
    }

    #[test]
    fn test_stl_binary_bbox_preserved() {
        let cube = make_unit_cube();
        let path = "/tmp/csg_test_cube_bbox.stl";
        write_stl_binary(&cube, path).unwrap();
        let loaded = read_stl_binary(path).unwrap();

        let bb = loaded.bounding_box();
        // f32 precision in STL, so check with larger epsilon
        assert!((bb.min.x - (-0.5)).abs() < 0.001);
        assert!((bb.max.x - 0.5).abs() < 0.001);
        assert!((bb.min.y - (-0.5)).abs() < 0.001);
        assert!((bb.max.y - 0.5).abs() < 0.001);

        fs::remove_file(path).ok();
    }
}
