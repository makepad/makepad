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
    LABEL_CLASS_MUTED, LABEL_CLASS_SHOP, LABEL_CLASS_TRANSPORT, LABEL_CLASS_TREE, LABEL_CLASS_HEALTH,
};
use super::geometry::TagLookup;
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
    ("charger_pin", include_str!("icons/charger_pin.svg")),
    ("parking", include_str!("icons/parking.svg")),
    ("charger", include_str!("icons/charger.svg")),
];

static ICON_MESH_CACHE: OnceLock<HashMap<&'static str, IconMesh>> = OnceLock::new();

fn icons() -> &'static HashMap<&'static str, IconMesh> {
    ICON_MESH_CACHE.get_or_init(|| {
        let mut out = HashMap::new();
        for (name, svg) in ICON_SVGS {
            if let Some(mesh) = build_icon_mesh(svg) {
                out.insert(*name, mesh);
            }
        }
        // Trees render as plain canopy discs (carto draws them as circles,
        // not glyphs).
        if let Some(mesh) = build_disc_mesh(5.4) {
            out.insert("tree", mesh);
        }
        // Generic small dot for named POIs with no dedicated symbol.
        if let Some(mesh) = build_disc_mesh(2.4) {
            out.insert("dot", mesh);
        }
        // Dark center dot layered over the light tree canopy.
        if let Some(mesh) = build_disc_mesh(1.7) {
            out.insert("tree_core", mesh);
        }
        // Tesla-style charger pins: wide badge (bolt + kW text) for fast
        // sites, small badge for street AC; white bolt overlays. The mesh
        // is shifted so the TAIL TIP sits exactly on the anchor (the site):
        // a centered mesh made the tip sweep across the ground when the
        // camera rotated in a tilted view.
        // wide: viewBox 34x30, tip (17,24), scale 30/34 -> tip at +7.94
        if let Some(mesh) =
            build_icon_mesh_sized_offset(include_str!("icons/charger_pin_wide.svg"), 30.0, 0.0, -7.94)
        {
            out.insert("charger_pin_fast", mesh);
        }
        // small: viewBox 22x22, tip (11,20.5), scale 16/22 -> tip at +6.91
        if let Some(mesh) =
            build_icon_mesh_sized_offset(include_str!("icons/charger_pin.svg"), 16.0, 0.0, -6.91)
        {
            out.insert("charger_pin_ac", mesh);
        }

        // Bolt overlays carry their in-pin offset IN THE MESH (screen px):
        // offsetting the anchor instead scales with the map and the pin
        // composite smears apart at fractional zooms.
        if let Some(mesh) =
            build_icon_mesh_sized_offset(include_str!("icons/charger.svg"), 9.0, -8.5, -12.35)
        {
            out.insert("charger_bolt_fast", mesh);
        }
        if let Some(mesh) =
            build_icon_mesh_sized_offset(include_str!("icons/charger.svg"), 9.0, 0.0, -8.36)
        {
            out.insert("charger_bolt_ac", mesh);
        }

        out
    })
}

pub fn icon_mesh(name: &str) -> Option<&'static IconMesh> {
    icons().get(name)
}

/// Every symbol mesh in one stable order, so a tile can name a mesh by slot
/// and the view binds one shared GPU copy per slot for instanced draws.
struct IconSlots {
    meshes: Vec<&'static IconMesh>,
    by_address: HashMap<usize, u16>,
}

static ICON_MESH_SLOTS: OnceLock<IconSlots> = OnceLock::new();

fn icon_slots() -> &'static IconSlots {
    ICON_MESH_SLOTS.get_or_init(|| {
        let registry = icons();
        let mut names: Vec<&&str> = registry.keys().collect();
        names.sort_unstable();
        let meshes: Vec<&'static IconMesh> = names.iter().map(|name| &registry[**name]).collect();
        let by_address = meshes
            .iter()
            .enumerate()
            .map(|(slot, mesh)| (*mesh as *const IconMesh as usize, slot as u16))
            .collect();
        IconSlots { meshes, by_address }
    })
}

pub(super) fn warm_icon_registries() {
    let _ = icons();
    let _ = icon_slots();
}

/// The slot of a registry mesh (every mesh `icon_mesh` hands out has one).
pub fn icon_mesh_slot(mesh: &IconMesh) -> u16 {
    icon_slots()
        .by_address
        .get(&(mesh as *const IconMesh as usize))
        .copied()
        .expect("icon mesh outside the registry")
}

pub fn icon_mesh_by_slot(slot: u16) -> Option<&'static IconMesh> {
    icon_slots().meshes.get(slot as usize).copied()
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
    build_icon_mesh_sized(svg, ICON_SIZE_PX)
}

fn build_icon_mesh_sized_offset(
    svg: &str,
    size_px: f32,
    dx: f32,
    dy: f32,
) -> Option<IconMesh> {
    let mut mesh = build_icon_mesh_sized(svg, size_px)?;
    for vertex in &mut mesh.verts {
        vertex.x += dx;
        vertex.y += dy;
    }
    Some(mesh)
}

/// Seven-segment digit meshes: pin badges draw their kW number as PART
/// of the icon composite — same anchor, same billboard transform, so the

/// "/" separator for the in-pin "kW/stalls" text: a diagonal bar in the

/// Small multiplication cross for the stall-count line ("x5"): two

fn build_icon_mesh_sized(svg: &str, size_px: f32) -> Option<IconMesh> {
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

    let scale = size_px / width.max(height);
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
pub fn micro_icon_for_tags(tags: &impl TagLookup) -> Option<(&'static str, u8)> {
    if tags.get("natural") == Some("tree") {
        return Some(("tree", LABEL_CLASS_TREE));
    }
    if let Some(amenity) = tags.get("amenity") {
        return match amenity {
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
    if tags.get("highway") == Some("traffic_signals") {
        return Some(("traffic_signals", LABEL_CLASS_MUTED));
    }
    // Offices (TomTom etc.) only exist in the detail archive; carto shows
    // them as a small dot + name from street-level zoom.
    if tags.contains_key("office") && tags.contains_key("name") {
        return Some(("dot", LABEL_CLASS_MUTED));
    }
    if let Some(leisure) = tags.get("leisure") {
        return match leisure {
            "playground" => Some(("playground", LABEL_CLASS_GREEN)),
            "picnic_table" => Some(("bench", LABEL_CLASS_GREEN)),
            _ => None,
        };
    }
    if let Some(tourism) = tags.get("tourism") {
        return match tourism {
            "artwork" => Some(("statue", LABEL_CLASS_CULTURE)),
            "information" => Some(("information", LABEL_CLASS_CULTURE)),
            _ => None,
        };
    }
    // Building/station entrances (door icon, high zoom only — the caller
    // gates the zoom).
    if tags.get("railway") == Some("subway_entrance") {
        return Some(("entrance", LABEL_CLASS_TRANSPORT));
    }
    if let Some(entrance) = tags.get("entrance") {
        return match entrance {
            "no" => None,
            _ => Some(("entrance", LABEL_CLASS_MUTED)),
        };
    }
    if let Some(historic) = tags.get("historic") {
        return match historic {
            "memorial" | "monument" | "statue" => Some(("statue", LABEL_CLASS_CULTURE)),
            _ => None,
        };
    }
    None
}

/// Map shortbread poi attributes to a symbol + label color class.
pub fn icon_for_tags(tags: &impl TagLookup) -> Option<(&'static str, u8)> {
    match tags.get("layer") {
        Some("micro_pois") => return micro_icon_for_tags(tags),
        // Geodata overlays (layers.md). Charger pins color by BRAND —
        // red is exclusively Tesla Superchargers; other brands split
        // fast DC amber / street AC blue (kW still shows in the bubble).
        Some("chargers") => {
            let kw = tags
                .get("max_kw")
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0);
            let is_tesla = tags
                .get("operator")
                .or_else(|| tags.get("brand"))
                .is_some_and(|value| value.to_lowercase().contains("tesla"));
            let class = if is_tesla {
                LABEL_CLASS_HEALTH
            } else if kw >= 50.0 {
                LABEL_CLASS_AMENITY
            } else {
                LABEL_CLASS_TRANSPORT
            };
            return Some(("charger", class));
        }
        Some("stops") => return Some(("dot", LABEL_CLASS_TRANSPORT)),
        _ => {}
    }
    if let Some(shop) = tags.get("shop") {
        let name = match shop {
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
        return match amenity {
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
        return match tourism {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_icon_registries_are_warmed() {
        super::super::warm_shared_registries();
        assert!(ICON_MESH_CACHE.get().is_some());
        assert!(ICON_MESH_SLOTS.get().is_some());
    }
}
