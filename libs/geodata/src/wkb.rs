//! GeoPackage geometry blob + (ISO/EWKB) WKB parser. XY only; Z/M dropped.

#[derive(Debug, Clone)]
pub enum Geometry {
    Point(f64, f64),
    MultiPoint(Vec<(f64, f64)>),
    LineString(Vec<(f64, f64)>),
    MultiLineString(Vec<Vec<(f64, f64)>>),
    /// Rings: first exterior, rest holes.
    Polygon(Vec<Vec<(f64, f64)>>),
    MultiPolygon(Vec<Vec<Vec<(f64, f64)>>>),
}

impl Geometry {
    /// Apply a coordinate transform to every vertex.
    pub fn map_coords(&self, f: &impl Fn(f64, f64) -> (f64, f64)) -> Geometry {
        let m1 = |pts: &Vec<(f64, f64)>| pts.iter().map(|&(x, y)| f(x, y)).collect::<Vec<_>>();
        match self {
            Geometry::Point(x, y) => {
                let (x, y) = f(*x, *y);
                Geometry::Point(x, y)
            }
            Geometry::MultiPoint(p) => Geometry::MultiPoint(m1(p)),
            Geometry::LineString(p) => Geometry::LineString(m1(p)),
            Geometry::MultiLineString(l) => {
                Geometry::MultiLineString(l.iter().map(m1).collect())
            }
            Geometry::Polygon(r) => Geometry::Polygon(r.iter().map(m1).collect()),
            Geometry::MultiPolygon(polys) => Geometry::MultiPolygon(
                polys.iter().map(|r| r.iter().map(m1).collect()).collect(),
            ),
        }
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn u32(&mut self, le: bool) -> Option<u32> {
        let bytes: [u8; 4] = self.data.get(self.pos..self.pos + 4)?.try_into().ok()?;
        self.pos += 4;
        Some(if le {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }
    fn f64(&mut self, le: bool) -> Option<f64> {
        let bytes: [u8; 8] = self.data.get(self.pos..self.pos + 8)?.try_into().ok()?;
        self.pos += 8;
        Some(if le {
            f64::from_le_bytes(bytes)
        } else {
            f64::from_be_bytes(bytes)
        })
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        if self.pos + n > self.data.len() {
            return None;
        }
        self.pos += n;
        Some(())
    }
}

/// Parse a GeoPackage geometry blob (GP header + WKB).
pub fn parse_gpkg_geometry(blob: &[u8]) -> Option<Geometry> {
    if blob.len() < 8 || blob[0] != b'G' || blob[1] != b'P' {
        return parse_wkb_geometry(blob); // tolerate raw WKB
    }
    let flags = blob[3];
    let header_le = flags & 0x01 != 0;
    let envelope_kind = (flags >> 1) & 0x07;
    if flags & 0x10 != 0 {
        return None; // declared-empty geometry
    }
    let envelope_len = match envelope_kind {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        _ => return None,
    };
    let mut cur = Cursor {
        data: blob,
        pos: 4,
    };
    let _srs_id = cur.u32(header_le)?;
    cur.skip(envelope_len)?;
    parse_wkb_geometry(&blob[cur.pos..])
}

/// Parse ISO WKB / EWKB. Returns XY geometry.
pub fn parse_wkb_geometry(data: &[u8]) -> Option<Geometry> {
    let mut cur = Cursor { data, pos: 0 };
    parse_wkb_inner(&mut cur)
}

fn parse_wkb_inner(cur: &mut Cursor) -> Option<Geometry> {
    let le = match cur.u8()? {
        0 => false,
        1 => true,
        _ => return None,
    };
    let raw_type = cur.u32(le)?;

    // EWKB flags
    let has_z_flag = raw_type & 0x8000_0000 != 0;
    let has_m_flag = raw_type & 0x4000_0000 != 0;
    let has_srid = raw_type & 0x2000_0000 != 0;
    let base_iso = raw_type & 0x0FFF_FFFF;
    // ISO encodes Z/M as +1000/+2000/+3000
    let iso_dim = base_iso / 1000;
    let base = base_iso % 1000;
    let has_z = has_z_flag || iso_dim == 1 || iso_dim == 3;
    let has_m = has_m_flag || iso_dim == 2 || iso_dim == 3;
    let extra_dims = usize::from(has_z) + usize::from(has_m);

    if has_srid {
        cur.u32(le)?;
    }

    let read_pt = |cur: &mut Cursor| -> Option<(f64, f64)> {
        let x = cur.f64(le)?;
        let y = cur.f64(le)?;
        cur.skip(extra_dims * 8)?;
        Some((x, y))
    };
    let read_ring = |cur: &mut Cursor| -> Option<Vec<(f64, f64)>> {
        let n = cur.u32(le)? as usize;
        let mut pts = Vec::with_capacity(n.min(1 << 20));
        for _ in 0..n {
            pts.push(read_pt(cur)?);
        }
        Some(pts)
    };

    match base {
        1 => {
            let (x, y) = read_pt(cur)?;
            Some(Geometry::Point(x, y))
        }
        2 => Some(Geometry::LineString(read_ring(cur)?)),
        3 => {
            let n = cur.u32(le)? as usize;
            let mut rings = Vec::with_capacity(n.min(1 << 16));
            for _ in 0..n {
                rings.push(read_ring(cur)?);
            }
            Some(Geometry::Polygon(rings))
        }
        4 => {
            let n = cur.u32(le)? as usize;
            let mut pts = Vec::with_capacity(n.min(1 << 20));
            for _ in 0..n {
                match parse_wkb_inner(cur)? {
                    Geometry::Point(x, y) => pts.push((x, y)),
                    _ => return None,
                }
            }
            Some(Geometry::MultiPoint(pts))
        }
        5 => {
            let n = cur.u32(le)? as usize;
            let mut lines = Vec::with_capacity(n.min(1 << 20));
            for _ in 0..n {
                match parse_wkb_inner(cur)? {
                    Geometry::LineString(pts) => lines.push(pts),
                    _ => return None,
                }
            }
            Some(Geometry::MultiLineString(lines))
        }
        6 => {
            let n = cur.u32(le)? as usize;
            let mut polys = Vec::with_capacity(n.min(1 << 20));
            for _ in 0..n {
                match parse_wkb_inner(cur)? {
                    Geometry::Polygon(rings) => polys.push(rings),
                    _ => return None,
                }
            }
            Some(Geometry::MultiPolygon(polys))
        }
        7 => {
            // GeometryCollection: merge whatever is inside into a MultiPolygon
            // if all polygons, else give up (none of our sources need more).
            let n = cur.u32(le)? as usize;
            let mut polys = Vec::new();
            for _ in 0..n {
                match parse_wkb_inner(cur)? {
                    Geometry::Polygon(rings) => polys.push(rings),
                    Geometry::MultiPolygon(mut more) => polys.append(&mut more),
                    _ => return None,
                }
            }
            Some(Geometry::MultiPolygon(polys))
        }
        _ => None,
    }
}
