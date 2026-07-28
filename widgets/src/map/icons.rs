//! Zoom-constant vector POI symbols.
//!
//! The openstreetmap-carto symbol SVGs (CC0) are tessellated ONCE at their
//! final on-screen size and appended into the tile's cached vector buffers as
//! anchor + screen-px-offset vertices; the map vertex shader adds the offset
//! AFTER the map transform, so symbols keep a constant pixel size at every
//! zoom — pure vector, no raster atlas. (The same encoding can later back a
//! single-quad fragment-side curve evaluator without changing the tile data.)

use super::label::{
    LABEL_CLASS_AMENITY, LABEL_CLASS_CULTURE, LABEL_CLASS_DEFAULT, LABEL_CLASS_GREEN,
    LABEL_CLASS_MUTED, LABEL_CLASS_SHOP, LABEL_CLASS_TRANSPORT, LABEL_CLASS_TREE,
};
use crate::makepad_draw::vector::{
    document::SvgNode, parse::parse_svg, LineJoin, PathCmd, Tessellator, VVertex, VectorPath,
};
use std::collections::HashMap;
use std::sync::OnceLock;

/// On-screen symbol size; carto icons are authored at 14x14.
pub const ICON_SIZE_PX: f32 = 14.0;
/// Symbols appear from this view-zoom bucket (carto shows the full POI
/// symbol set from z17).
pub const ICON_MIN_ZOOM: u32 = 17;

pub struct IconMesh {
    pub verts: Vec<VVertex>,
    pub indices: Vec<u32>,
}

const ICON_SVGS: &[(&str, &str)] = &[
    ("alcohol", include_str!("icons/alcohol.svg")),
    ("atm", include_str!("icons/atm.svg")),
    ("bakery", include_str!("icons/bakery.svg")),
    ("bank", include_str!("icons/bank.svg")),
    ("bar", include_str!("icons/bar.svg")),
    ("beauty", include_str!("icons/beauty.svg")),
    ("bicycle", include_str!("icons/bicycle.svg")),
    ("butcher", include_str!("icons/butcher.svg")),
    ("cafe", include_str!("icons/cafe.svg")),
    ("cinema", include_str!("icons/cinema.svg")),
    ("clothes", include_str!("icons/clothes.svg")),
    ("coffee", include_str!("icons/coffee.svg")),
    ("convenience", include_str!("icons/convenience.svg")),
    ("fast_food", include_str!("icons/fast_food.svg")),
    ("florist", include_str!("icons/florist.svg")),
    ("gift", include_str!("icons/gift.svg")),
    ("greengrocer", include_str!("icons/greengrocer.svg")),
    ("hairdresser", include_str!("icons/hairdresser.svg")),
    ("hotel", include_str!("icons/hotel.svg")),
    ("ice_cream", include_str!("icons/ice_cream.svg")),
    ("laundry", include_str!("icons/laundry.svg")),
    ("library", include_str!("icons/library.svg")),
    ("museum", include_str!("icons/museum.svg")),
    ("nightclub", include_str!("icons/nightclub.svg")),
    ("pharmacy", include_str!("icons/pharmacy.svg")),
    ("place_of_worship", include_str!("icons/place_of_worship.svg")),
    ("pub", include_str!("icons/pub.svg")),
    ("restaurant", include_str!("icons/restaurant.svg")),
    ("supermarket", include_str!("icons/supermarket.svg")),
    ("theatre", include_str!("icons/theatre.svg")),
    // Micro-POIs from the all-tag detail archive
    ("bench", include_str!("icons/bench.svg")),
    ("waste_basket", include_str!("icons/waste_basket.svg")),
    ("recycling", include_str!("icons/recycling.svg")),
    ("playground", include_str!("icons/playground.svg")),
    ("statue", include_str!("icons/statue.svg")),
    ("entrance", include_str!("icons/entrance.svg")),
    ("information", include_str!("icons/information.svg")),
    ("traffic_signals", include_str!("icons/traffic_signals.svg")),
    ("parking", include_str!("icons/parking.svg")),
    ("charger", include_str!("icons/charger.svg")),
];

fn icons() -> &'static HashMap<&'static str, IconMesh> {
    static CACHE: OnceLock<HashMap<&'static str, IconMesh>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut out = HashMap::new();
        for (name, svg) in ICON_SVGS {
            if let Some(mesh) = build_icon_mesh(svg) {
                out.insert(*name, mesh);
            }
        }
        // Trees render as plain canopy discs (carto draws them as circles,
        // not glyphs).
        if let Some(mesh) = build_disc_mesh(3.4) {
            out.insert("tree", mesh);
        }
        // Generic small dot for named POIs with no dedicated symbol.
        if let Some(mesh) = build_disc_mesh(2.4) {
            out.insert("dot", mesh);
        }
        // Dark center dot layered over the light tree canopy.
        if let Some(mesh) = build_disc_mesh(1.3) {
            out.insert("tree_core", mesh);
        }
        out
    })
}

pub fn icon_mesh(name: &str) -> Option<&'static IconMesh> {
    icons().get(name)
}

fn transform_coord(value: f32, center: f32, scale: f32) -> f32 {
    (value - center) * scale
}

fn collect_paths(nodes: &[SvgNode], out: &mut VectorPath) {
    for node in nodes {
        match node {
            SvgNode::Path(p) => {
                for cmd in &p.path.cmds {
                    let t = &p.transform;
                    out.cmds.push(match *cmd {
                        PathCmd::MoveTo(x, y) => {
                            let (x, y) = t.apply(x, y);
                            PathCmd::MoveTo(x, y)
                        }
                        PathCmd::LineTo(x, y) => {
                            let (x, y) = t.apply(x, y);
                            PathCmd::LineTo(x, y)
                        }
                        PathCmd::BezierTo(x1, y1, x2, y2, x, y) => {
                            let (x1, y1) = t.apply(x1, y1);
                            let (x2, y2) = t.apply(x2, y2);
                            let (x, y) = t.apply(x, y);
                            PathCmd::BezierTo(x1, y1, x2, y2, x, y)
                        }
                        PathCmd::Close => PathCmd::Close,
                        PathCmd::Winding(w) => PathCmd::Winding(w),
                    });
                }
            }
            SvgNode::Group(g) => collect_paths(&g.children, out),
            _ => {}
        }
    }
}

/// Parse + tessellate one symbol at its final screen size, centered on the
/// origin so vertices double as screen-px offsets from the anchor point.
fn build_icon_mesh(svg: &str) -> Option<IconMesh> {
    let doc = parse_svg(svg);
    let (width, height) = doc.logical_size();
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let mut path = VectorPath::new();
    collect_paths(&doc.root, &mut path);
    if path.cmds.is_empty() {
        return None;
    }

    let scale = ICON_SIZE_PX / width.max(height);
    let (center_x, center_y) = (width * 0.5, height * 0.5);
    for cmd in &mut path.cmds {
        match cmd {
            PathCmd::MoveTo(x, y) | PathCmd::LineTo(x, y) => {
                *x = transform_coord(*x, center_x, scale);
                *y = transform_coord(*y, center_y, scale);
            }
            PathCmd::BezierTo(x1, y1, x2, y2, x, y) => {
                *x1 = transform_coord(*x1, center_x, scale);
                *y1 = transform_coord(*y1, center_y, scale);
                *x2 = transform_coord(*x2, center_x, scale);
                *y2 = transform_coord(*y2, center_y, scale);
                *x = transform_coord(*x, center_x, scale);
                *y = transform_coord(*y, center_y, scale);
            }
            PathCmd::Close | PathCmd::Winding(_) => {}
        }
    }

    let mut tess = Tessellator::default();
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    tess.flatten(&path, 0.05);
    tess.fill(1.0, LineJoin::Miter, 4.0, false, &mut verts, &mut indices);
    if verts.is_empty() || indices.is_empty() {
        return None;
    }
    Some(IconMesh { verts, indices })
}

/// Circle mesh for canopy/dot symbols, centered on the origin like the SVG
/// icons (vertices double as screen-px offsets from the anchor).
fn build_disc_mesh(radius: f32) -> Option<IconMesh> {
    const K: f32 = 0.552_284_75;
    let r = radius;
    let k = r * K;
    let mut path = VectorPath::new();
    path.cmds.push(PathCmd::MoveTo(r, 0.0));
    path.cmds.push(PathCmd::BezierTo(r, k, k, r, 0.0, r));
    path.cmds.push(PathCmd::BezierTo(-k, r, -r, k, -r, 0.0));
    path.cmds.push(PathCmd::BezierTo(-r, -k, -k, -r, 0.0, -r));
    path.cmds.push(PathCmd::BezierTo(k, -r, r, -k, r, 0.0));
    path.cmds.push(PathCmd::Close);

    let mut tess = Tessellator::default();
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    tess.flatten(&path, 0.05);
    tess.fill(1.0, LineJoin::Miter, 4.0, false, &mut verts, &mut indices);
    if verts.is_empty() || indices.is_empty() {
        return None;
    }
    Some(IconMesh { verts, indices })
}

/// Micro-POI symbols sourced from the all-tag detail archive (not present in
/// shortbread pois): trees, benches, bins, recycling, playgrounds, artwork.
pub fn micro_icon_for_tags(tags: &HashMap<String, String>) -> Option<(&'static str, u8)> {
    if tags.get("natural").map(|v| v.as_str()) == Some("tree") {
        return Some(("tree", LABEL_CLASS_TREE));
    }
    if let Some(amenity) = tags.get("amenity") {
        return match amenity.as_str() {
            "bench" => Some(("bench", LABEL_CLASS_MUTED)),
            "waste_basket" | "waste_disposal" => Some(("waste_basket", LABEL_CLASS_MUTED)),
            "recycling" => Some(("recycling", LABEL_CLASS_MUTED)),
            "bicycle_parking" => Some(("bicycle", LABEL_CLASS_TRANSPORT)),
            "parking" => Some(("parking", LABEL_CLASS_TRANSPORT)),
            "parking_entrance" => Some(("parking", LABEL_CLASS_TRANSPORT)),
            "charging_station" => Some(("charger", LABEL_CLASS_TRANSPORT)),
            _ => None,
        };
    }
    if tags.get("highway").map(|v| v.as_str()) == Some("traffic_signals") {
        return Some(("traffic_signals", LABEL_CLASS_MUTED));
    }
    // Offices (TomTom etc.) only exist in the detail archive; carto shows
    // them as a small dot + name from street-level zoom.
    if tags.contains_key("office") && tags.contains_key("name") {
        return Some(("dot", LABEL_CLASS_MUTED));
    }
    if let Some(leisure) = tags.get("leisure") {
        return match leisure.as_str() {
            "playground" => Some(("playground", LABEL_CLASS_GREEN)),
            "picnic_table" => Some(("bench", LABEL_CLASS_GREEN)),
            _ => None,
        };
    }
    if let Some(tourism) = tags.get("tourism") {
        return match tourism.as_str() {
            "artwork" => Some(("statue", LABEL_CLASS_CULTURE)),
            "information" => Some(("information", LABEL_CLASS_CULTURE)),
            _ => None,
        };
    }
    // Building/station entrances (door icon, high zoom only — the caller
    // gates the zoom).
    if tags.get("railway").map(|v| v.as_str()) == Some("subway_entrance") {
        return Some(("entrance", LABEL_CLASS_TRANSPORT));
    }
    if let Some(entrance) = tags.get("entrance") {
        return match entrance.as_str() {
            "no" => None,
            _ => Some(("entrance", LABEL_CLASS_MUTED)),
        };
    }
    if let Some(historic) = tags.get("historic") {
        return match historic.as_str() {
            "memorial" | "monument" | "statue" => Some(("statue", LABEL_CLASS_CULTURE)),
            _ => None,
        };
    }
    None
}

/// Map shortbread poi attributes to a symbol + label color class.
pub fn icon_for_tags(tags: &HashMap<String, String>) -> Option<(&'static str, u8)> {
    if tags.get("layer").map(|v| v.as_str()) == Some("micro_pois") {
        return micro_icon_for_tags(tags);
    }
    if let Some(shop) = tags.get("shop") {
        let name = match shop.as_str() {
            "supermarket" => "supermarket",
            "bakery" => "bakery",
            "butcher" => "butcher",
            "clothes" | "fashion" | "shoes" => "clothes",
            "hairdresser" => "hairdresser",
            "coffee" => "coffee",
            "alcohol" | "wine" => "alcohol",
            "beauty" | "cosmetics" => "beauty",
            "bicycle" => "bicycle",
            "florist" => "florist",
            "gift" => "gift",
            "greengrocer" => "greengrocer",
            "laundry" | "dry_cleaning" => "laundry",
            _ => "convenience",
        };
        return Some((name, LABEL_CLASS_SHOP));
    }
    if let Some(amenity) = tags.get("amenity") {
        return match amenity.as_str() {
            "restaurant" | "food_court" => Some(("restaurant", LABEL_CLASS_AMENITY)),
            "cafe" => Some(("cafe", LABEL_CLASS_AMENITY)),
            "fast_food" => Some(("fast_food", LABEL_CLASS_AMENITY)),
            "bar" => Some(("bar", LABEL_CLASS_AMENITY)),
            "pub" | "biergarten" => Some(("pub", LABEL_CLASS_AMENITY)),
            "ice_cream" => Some(("ice_cream", LABEL_CLASS_AMENITY)),
            "nightclub" => Some(("nightclub", LABEL_CLASS_AMENITY)),
            "pharmacy" => Some(("pharmacy", LABEL_CLASS_DEFAULT)),
            "bank" => Some(("bank", LABEL_CLASS_DEFAULT)),
            "atm" => Some(("atm", LABEL_CLASS_DEFAULT)),
            "parking" => Some(("parking", LABEL_CLASS_TRANSPORT)),
            "charging_station" => Some(("charger", LABEL_CLASS_TRANSPORT)),
            "place_of_worship" => Some(("place_of_worship", LABEL_CLASS_CULTURE)),
            "cinema" => Some(("cinema", LABEL_CLASS_CULTURE)),
            "theatre" => Some(("theatre", LABEL_CLASS_CULTURE)),
            "library" => Some(("library", LABEL_CLASS_CULTURE)),
            _ => None,
        };
    }
    if let Some(tourism) = tags.get("tourism") {
        return match tourism.as_str() {
            "hotel" | "guest_house" | "hostel" => Some(("hotel", LABEL_CLASS_CULTURE)),
            "museum" | "gallery" => Some(("museum", LABEL_CLASS_CULTURE)),
            "information" => Some(("information", LABEL_CLASS_CULTURE)),
            _ => Some(("dot", LABEL_CLASS_CULTURE)),
        };
    }
    if tags.contains_key("leisure") {
        return Some(("dot", LABEL_CLASS_GREEN));
    }
    // Named POI with no matched symbol: carto's small colored dot, so the
    // place still shows up (and its label gets the class color).
    if tags.contains_key("name")
        && (tags.contains_key("amenity") || tags.contains_key("office") || tags.contains_key("craft"))
    {
        return Some(("dot", LABEL_CLASS_DEFAULT));
    }
    None
}
