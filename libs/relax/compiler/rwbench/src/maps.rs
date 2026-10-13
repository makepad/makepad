// Map tile decode, headless: an MBTiles archive of Mapbox vector tiles read back the way the map
// stack reads its archives (makepad-mbtile-reader: SQLite b-tree lookup, the per-archive tile codec
// -- brotli here, as the hosted world uses -- then protobuf walking and geometry decoding).
// The repo carries no tile set (the Amsterdam / world archives are tens of GB to bake or
// download), so setup generates a deterministic city-like pyramid -- roads, buildings, points, with
// attributes -- encodes it with the repo's MVT encoder (libs/geodata/src/mvt.rs, included by path:
// the geodata crate itself pulls in the network stack) and writes it with the repo's MbtilesWriter.
// Per iteration: open the archive, fetch + decode every tile, decode every feature's geometry,
// fold the vertices and the polygons' signed areas. The widgets' own tile builder is entangled
// with the draw/platform layer and cannot run headless.
use crate::fold;
use makepad_mbtile_reader::{read_pb_len_slice, read_pb_varint, skip_pb_field, tile_rowid_xyz, MbtilesReader, MbtilesWriter, TileCompression};

#[path = "../../../../../libs/geodata/src/mvt.rs"]
#[allow(dead_code)]
mod mvt;
use mvt::{command, zigzag, AttrVal, GeomType, PreFeature, TileEnc};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo) as u64) as i64
    }
}

fn line_commands(pts: &[(i64, i64)], close: bool) -> Vec<u32> {
    let mut c = Vec::new();
    let (mut cx, mut cy) = (0i64, 0i64);
    c.push(command(1, 1));
    c.push(zigzag(pts[0].0 - cx));
    c.push(zigzag(pts[0].1 - cy));
    cx = pts[0].0;
    cy = pts[0].1;
    c.push(command(2, (pts.len() - 1) as u32));
    for p in &pts[1..] {
        c.push(zigzag(p.0 - cx));
        c.push(zigzag(p.1 - cy));
        cx = p.0;
        cy = p.1;
    }
    if close {
        c.push(command(7, 1));
    }
    c
}

fn make_tile(rng: &mut Rng) -> Vec<u8> {
    let mut t = TileEnc::new();
    let names = ["residential", "service", "primary", "footway", "tertiary"];
    for _ in 0..rng.range(20, 60) {
        let (mut x, mut y) = (rng.range(0, 4096), rng.range(0, 4096));
        let mut pts = vec![(x, y)];
        for _ in 0..rng.range(3, 40) {
            x = (x + rng.range(-300, 300)).clamp(-64, 4160);
            y = (y + rng.range(-300, 300)).clamp(-64, 4160);
            pts.push((x, y));
        }
        let f = PreFeature {
            geom_type: GeomType::Line,
            commands: line_commands(&pts, false),
            attrs: vec![("class".into(), AttrVal::Str(names[rng.range(0, 5) as usize].into())), ("oneway".into(), AttrVal::Bool(rng.next() & 1 == 0))],
        };
        t.add_feature("transportation", &f);
    }
    for _ in 0..rng.range(40, 160) {
        let (x, y) = (rng.range(0, 3900), rng.range(0, 3900));
        let (w, h) = (rng.range(20, 160), rng.range(20, 160));
        let mut pts = vec![(x, y), (x + w, y), (x + w, y + h)];
        if rng.next() & 1 == 0 {
            pts.push((x + w / 2, y + h + rng.range(5, 40)));
        }
        pts.push((x, y + h));
        let f = PreFeature {
            geom_type: GeomType::Polygon,
            commands: line_commands(&pts, true),
            attrs: vec![("height".into(), AttrVal::Int(rng.range(3, 60))), ("kind".into(), AttrVal::Str("yes".into()))],
        };
        t.add_feature("building", &f);
    }
    for _ in 0..rng.range(5, 30) {
        let f = PreFeature {
            geom_type: GeomType::Point,
            commands: vec![command(1, 1), zigzag(rng.range(0, 4096)), zigzag(rng.range(0, 4096))],
            attrs: vec![("name".into(), AttrVal::Str(format!("poi {}", rng.range(0, 500))))],
        };
        t.add_feature("poi", &f);
    }
    t.encode()
}

struct TempArchive(std::path::PathBuf);
impl Drop for TempArchive {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// decodes one geometry command stream: vertices folded, polygon rings' signed area (f64) summed
fn geometry(cmds: &[u32], h: &mut u64, area: &mut f64) -> usize {
    let (mut x, mut y) = (0i64, 0i64);
    let mut ring: Vec<(f32, f32)> = Vec::new();
    let mut i = 0;
    let mut verts = 0;
    while i < cmds.len() {
        let id = cmds[i] & 7;
        let count = (cmds[i] >> 3) as usize;
        i += 1;
        match id {
            1 | 2 => {
                for _ in 0..count {
                    let dx = ((cmds[i] >> 1) as i64) ^ -((cmds[i] & 1) as i64);
                    let dy = ((cmds[i + 1] >> 1) as i64) ^ -((cmds[i + 1] & 1) as i64);
                    i += 2;
                    x += dx;
                    y += dy;
                    if id == 1 && !ring.is_empty() {
                        ring.clear();
                    }
                    ring.push((x as f32 / 4096.0, y as f32 / 4096.0));
                    *h = fold(*h, (x as u64) << 20 ^ y as u64);
                    verts += 1;
                }
            }
            _ => {
                let mut a = 0.0f64;
                for k in 0..ring.len() {
                    let (p, q) = (ring[k], ring[(k + 1) % ring.len()]);
                    a += (p.0 as f64) * (q.1 as f64) - (q.0 as f64) * (p.1 as f64);
                }
                *area += a * 0.5;
            }
        }
    }
    verts
}

/// walks one MVT tile: layers -> features -> geometry; returns (features, vertices)
fn decode_mvt(tile: &[u8], h: &mut u64, area: &mut f64) -> (usize, usize) {
    let (mut features, mut verts) = (0, 0);
    let mut pos = 0;
    while pos < tile.len() {
        let tag = read_pb_varint(tile, &mut pos).unwrap();
        if tag >> 3 != 3 || tag & 7 != 2 {
            skip_pb_field(tile, &mut pos, (tag & 7) as u8).unwrap();
            continue;
        }
        let layer = read_pb_len_slice(tile, &mut pos).unwrap();
        let mut lp = 0;
        while lp < layer.len() {
            let t = read_pb_varint(layer, &mut lp).unwrap();
            match (t >> 3, t & 7) {
                (2, 2) => {
                    let f = read_pb_len_slice(layer, &mut lp).unwrap();
                    features += 1;
                    let mut fp = 0;
                    while fp < f.len() {
                        let ft = read_pb_varint(f, &mut fp).unwrap();
                        match (ft >> 3, ft & 7) {
                            (4, 2) => {
                                let g = read_pb_len_slice(f, &mut fp).unwrap();
                                let mut gp = 0;
                                let mut cmds = Vec::with_capacity(g.len());
                                while gp < g.len() {
                                    cmds.push(read_pb_varint(g, &mut gp).unwrap() as u32);
                                }
                                verts += geometry(&cmds, h, area);
                            }
                            (2, 2) => {
                                let tg = read_pb_len_slice(f, &mut fp).unwrap();
                                *h = fold(*h, tg.len() as u64);
                            }
                            (_, w) => skip_pb_field(f, &mut fp, w as u8).unwrap(),
                        }
                    }
                }
                (1, 2) => {
                    let name = read_pb_len_slice(layer, &mut lp).unwrap();
                    *h = fold(*h, name.len() as u64);
                }
                (_, w) => skip_pb_field(layer, &mut lp, w as u8).unwrap(),
            }
        }
    }
    (features, verts)
}

pub fn setup() -> Box<dyn FnMut() -> u64> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let z = 14u8;
    let (x0, y0) = (8400u32, 5400u32);
    let mut coords: Vec<(u32, u32)> = Vec::new();
    for dy in 0..14 {
        for dx in 0..14 {
            coords.push((x0 + dx, y0 + dy));
        }
    }
    coords.sort_by_key(|&(x, y)| tile_rowid_xyz(z, x, y).unwrap());
    let path = std::env::temp_dir().join(format!("rwbench-maps-{}.mbtiles", std::process::id()));
    let guard = TempArchive(path.clone());
    let mut w = MbtilesWriter::create(&path).unwrap();
    w.set_tile_compression(TileCompression::Brotli { quality: 5 }, None);
    w.set_metadata("name", "rwbench");
    w.set_metadata("format", "pbf");
    for &(x, y) in &coords {
        w.write_tile_encoded(z, x, y, &make_tile(&mut rng)).unwrap();
    }
    w.finish().unwrap();
    Box::new(move || {
        let _keep = &guard;
        let mut r = MbtilesReader::open(&path).unwrap();
        let mut h = 0u64;
        let mut area = 0.0f64;
        let (mut feats, mut verts) = (0usize, 0usize);
        for &(x, y) in &coords {
            let raw = r.get_tile_decoded(z as i64, x as i64, (1i64 << z) - 1 - y as i64).unwrap().expect("tile present");
            h = fold(h, raw.len() as u64);
            let (f, v) = decode_mvt(&raw, &mut h, &mut area);
            feats += f;
            verts += v;
        }
        fold(fold(fold(h, feats as u64), verts as u64), (area * 1e6).round() as i64 as u64)
    })
}
