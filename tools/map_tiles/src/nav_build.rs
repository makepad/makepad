//! `nav-build`: one parallel scan over an .osm.pbf producing the two nav
//! artifacts (`<basename>.graph` routing graph + `<basename>.search` place
//! index) via the builders in `makepad-map-nav`. `nav-probe` exercises the
//! artifacts from the command line (search queries, test routes) without UI.

use makepad_map_nav::geo::LonLat;
use makepad_map_nav::graph::{BuildRestriction, GraphBuilder, RouteGraph, TravelMode};
use makepad_map_nav::search::{
    category_from_osm_tags, Category, SearchIndex, SearchIndexBuilder,
};
use osmpbf::{Element, ElementReader, RelMemberType};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Clone, Copy)]
pub struct BboxFilter {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl BboxFilter {
    fn contains(&self, lon: f64, lat: f64) -> bool {
        lon >= self.west && lon <= self.east && lat >= self.south && lat <= self.north
    }
}

pub struct NavBuildOptions {
    pub source: PathBuf,
    pub output_basename: PathBuf,
    pub bbox: Option<BboxFilter>,
    pub skip_addresses: bool,
}

/// Way tags that matter for routing or search; everything else is dropped
/// at scan time to keep the kept-way memory bounded.
const KEPT_WAY_TAGS: &[&str] = &[
    "highway",
    "name",
    "ref",
    "oneway",
    "oneway:bicycle",
    "junction",
    "maxspeed",
    "access",
    "vehicle",
    "motor_vehicle",
    "bicycle",
    "foot",
    "bridge",
    "tunnel",
    "route",
    "amenity",
    "shop",
    "tourism",
    "leisure",
    "historic",
    "railway",
    "aeroway",
    "natural",
    "place",
    "population",
    "addr:street",
    "addr:housenumber",
    "addr:city",
];

#[derive(Clone)]
struct RawDoc {
    name: String,
    secondary: String,
    lon: f64,
    lat: f64,
    category: u16,
    rank: u8,
}

#[derive(Default)]
struct NodePass {
    nodes: Vec<(i64, f64, f64)>,
    docs: Vec<RawDoc>,
}

impl NodePass {
    fn merge(mut a: NodePass, mut b: NodePass) -> NodePass {
        if b.nodes.len() > a.nodes.len() {
            std::mem::swap(&mut a, &mut b);
        }
        a.nodes.append(&mut b.nodes);
        a.docs.append(&mut b.docs);
        a
    }
}

#[derive(Default)]
struct WayPass {
    ways: Vec<(i64, Vec<i64>, HashMap<String, String>)>,
    restrictions: Vec<BuildRestriction>,
    way_docs: Vec<(String, String, Vec<i64>, u16, u8)>, // name, secondary, nodes, category, rank
}

impl WayPass {
    fn merge(mut a: WayPass, mut b: WayPass) -> WayPass {
        if b.ways.len() > a.ways.len() {
            std::mem::swap(&mut a, &mut b);
        }
        a.ways.append(&mut b.ways);
        a.restrictions.append(&mut b.restrictions);
        a.way_docs.append(&mut b.way_docs);
        a
    }
}

fn place_rank(category: Category, tags: &HashMap<String, String>) -> u8 {
    let base = category.base_rank();
    let pop_boost = tags
        .get("population")
        .and_then(|p| p.parse::<u64>().ok())
        .map(|p| (p / 25_000).min(35) as u8)
        .unwrap_or(0);
    base.saturating_add(pop_boost)
}

fn secondary_from_tags(tags: &HashMap<String, String>) -> String {
    let street = tags.get("addr:street").map(|s| s.as_str()).unwrap_or("");
    let number = tags.get("addr:housenumber").map(|s| s.as_str()).unwrap_or("");
    let city = tags.get("addr:city").map(|s| s.as_str()).unwrap_or("");
    match (street.is_empty(), number.is_empty(), city.is_empty()) {
        (false, false, false) => format!("{} {}, {}", street, number, city),
        (false, false, true) => format!("{} {}", street, number),
        (false, true, false) => format!("{}, {}", street, city),
        (false, true, true) => street.to_string(),
        (true, _, false) => city.to_string(),
        _ => String::new(),
    }
}

fn doc_from_tags(
    tags: &HashMap<String, String>,
    lon: f64,
    lat: f64,
    skip_addresses: bool,
) -> Option<RawDoc> {
    let category = category_from_osm_tags(tags)?;
    if category == Category::Address {
        if skip_addresses {
            return None;
        }
        // Addresses are searchable as "street housenumber".
        let street = tags.get("addr:street")?;
        let number = tags.get("addr:housenumber")?;
        let city = tags.get("addr:city").map(|s| s.as_str()).unwrap_or("");
        return Some(RawDoc {
            name: format!("{} {}", street, number),
            secondary: city.to_string(),
            lon,
            lat,
            category: category as u16,
            rank: category.base_rank(),
        });
    }
    let name = tags.get("name")?.clone();
    Some(RawDoc {
        name,
        secondary: secondary_from_tags(tags),
        lon,
        lat,
        category: category as u16,
        rank: place_rank(category, tags),
    })
}

fn collect_tags<'a>(iter: impl Iterator<Item = (&'a str, &'a str)>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (k, v) in iter {
        if KEPT_WAY_TAGS.contains(&k) {
            out.insert(k.to_string(), v.to_string());
        }
    }
    out
}

/// Full tag map (used for nodes where category detection wants everything).
fn collect_all_tags<'a>(
    iter: impl Iterator<Item = (&'a str, &'a str)>,
) -> HashMap<String, String> {
    iter.map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

pub fn nav_build(options: NavBuildOptions) -> Result<(), String> {
    let total_start = Instant::now();
    let bbox = options.bbox;
    let skip_addresses = options.skip_addresses;

    // --- Pass 1: node coordinates (bbox-filtered) + node-anchored docs ---
    eprintln!("nav-build: pass 1/2 (nodes) over {}", options.source.display());
    let pass1_start = Instant::now();
    let reader = ElementReader::from_path(&options.source)
        .map_err(|err| format!("open {}: {err}", options.source.display()))?;
    let node_pass = reader
        .par_map_reduce(
            |element| {
                let mut acc = NodePass::default();
                let (id, lon, lat, has_tags) = match &element {
                    Element::Node(node) => (node.id(), node.lon(), node.lat(), true),
                    Element::DenseNode(node) => (node.id(), node.lon(), node.lat(), true),
                    _ => return acc,
                };
                if let Some(bbox) = &bbox {
                    if !bbox.contains(lon, lat) {
                        return acc;
                    }
                }
                acc.nodes.push((id, lon, lat));
                if has_tags {
                    let tags = match &element {
                        Element::Node(node) => collect_all_tags(node.tags()),
                        Element::DenseNode(node) => collect_all_tags(node.tags()),
                        _ => unreachable!(),
                    };
                    if !tags.is_empty() {
                        if let Some(doc) = doc_from_tags(&tags, lon, lat, skip_addresses) {
                            acc.docs.push(doc);
                        }
                    }
                }
                acc
            },
            NodePass::default,
            NodePass::merge,
        )
        .map_err(|err| format!("pbf node pass: {err}"))?;
    let node_map: HashMap<i64, (f64, f64)> = node_pass
        .nodes
        .iter()
        .map(|&(id, lon, lat)| (id, (lon, lat)))
        .collect();
    eprintln!(
        "nav-build: pass 1 done in {:.1}s — {} nodes in region, {} tagged docs",
        pass1_start.elapsed().as_secs_f64(),
        node_map.len(),
        node_pass.docs.len()
    );

    // --- Pass 2: routable ways, named/categorized ways, restrictions ---
    eprintln!("nav-build: pass 2/2 (ways + relations)");
    let pass2_start = Instant::now();
    let reader = ElementReader::from_path(&options.source)
        .map_err(|err| format!("open {}: {err}", options.source.display()))?;
    let node_map_ref = &node_map;
    let way_pass = reader
        .par_map_reduce(
            |element| {
                let mut acc = WayPass::default();
                match element {
                    Element::Way(way) => {
                        let refs: Vec<i64> = way.refs().collect();
                        if refs.len() < 2 || !refs.iter().any(|r| node_map_ref.contains_key(r)) {
                            return acc;
                        }
                        let tags = collect_tags(way.tags());
                        if tags.is_empty() {
                            return acc;
                        }
                        let routable =
                            tags.contains_key("highway") || tags.get("route").map(|r| r.as_str()) == Some("ferry");
                        if routable {
                            acc.ways.push((way.id(), refs.clone(), tags.clone()));
                        }
                        // Named/categorized ways (buildings, parks, POIs
                        // mapped as areas, addresses on building outlines).
                        if let Some(doc) = doc_from_tags(&tags, 0.0, 0.0, skip_addresses) {
                            // Street docs come from named highways below.
                            acc.way_docs.push((
                                doc.name,
                                doc.secondary,
                                refs.clone(),
                                doc.category,
                                doc.rank,
                            ));
                        } else if routable && !tags.contains_key("place") {
                            if let Some(name) = tags.get("name") {
                                // Named street.
                                let cat = Category::Street;
                                acc.way_docs.push((
                                    name.clone(),
                                    String::new(),
                                    refs,
                                    cat as u16,
                                    cat.base_rank(),
                                ));
                            }
                        }
                    }
                    Element::Relation(relation) => {
                        let mut is_restriction = false;
                        let mut restriction_value = String::new();
                        for (k, v) in relation.tags() {
                            if k == "type" && v == "restriction" {
                                is_restriction = true;
                            }
                            if k == "restriction" {
                                restriction_value = v.to_string();
                            }
                        }
                        if !is_restriction || restriction_value.is_empty() {
                            return acc;
                        }
                        let only = restriction_value.starts_with("only_");
                        let banned = restriction_value.starts_with("no_");
                        if !only && !banned {
                            return acc;
                        }
                        let mut from_way = None;
                        let mut to_way = None;
                        let mut via_node = None;
                        for member in relation.members() {
                            let role = member.role().unwrap_or("");
                            match (role, member.member_type) {
                                ("from", RelMemberType::Way) => from_way = Some(member.member_id),
                                ("to", RelMemberType::Way) => to_way = Some(member.member_id),
                                ("via", RelMemberType::Node) => via_node = Some(member.member_id),
                                _ => {}
                            }
                        }
                        if let (Some(from_way), Some(via_node), Some(to_way)) =
                            (from_way, via_node, to_way)
                        {
                            if node_map_ref.contains_key(&via_node) {
                                acc.restrictions.push(BuildRestriction {
                                    from_way,
                                    via_node,
                                    to_way,
                                    only,
                                });
                            }
                        }
                    }
                    _ => {}
                }
                acc
            },
            WayPass::default,
            WayPass::merge,
        )
        .map_err(|err| format!("pbf way pass: {err}"))?;
    eprintln!(
        "nav-build: pass 2 done in {:.1}s — {} routable ways, {} way docs, {} restrictions",
        pass2_start.elapsed().as_secs_f64(),
        way_pass.ways.len(),
        way_pass.way_docs.len(),
        way_pass.restrictions.len()
    );

    // --- Build the routing graph ---
    let build_start = Instant::now();
    let mut graph_builder = GraphBuilder::new();
    for &(id, lon, lat) in &node_pass.nodes {
        graph_builder.add_node(id, lon, lat);
    }
    for (id, refs, tags) in way_pass.ways {
        graph_builder.add_way(id, refs, tags);
    }
    for r in way_pass.restrictions {
        graph_builder.add_restriction(r);
    }
    let graph = graph_builder.build();
    eprintln!(
        "nav-build: graph built in {:.1}s — {} vertices, {} directed edges, {} restricted vias",
        build_start.elapsed().as_secs_f64(),
        graph.vertices.len(),
        graph.edges.len(),
        graph.restrictions.len()
    );

    // --- Build the search index ---
    let search_start = Instant::now();
    let mut search_builder = SearchIndexBuilder::new();
    for doc in &node_pass.docs {
        search_builder.add(
            &doc.name,
            &doc.secondary,
            LonLat::new(doc.lon, doc.lat),
            Category::from_u16(doc.category),
            doc.rank,
        );
    }
    for (name, secondary, refs, category, rank) in &way_pass.way_docs {
        // Way centroid from its in-region nodes.
        let mut count = 0usize;
        let (mut sum_lon, mut sum_lat) = (0.0f64, 0.0f64);
        for r in refs {
            if let Some(&(lon, lat)) = node_map.get(r) {
                sum_lon += lon;
                sum_lat += lat;
                count += 1;
            }
        }
        if count == 0 {
            continue;
        }
        search_builder.add(
            name,
            secondary,
            LonLat::new(sum_lon / count as f64, sum_lat / count as f64),
            Category::from_u16(*category),
            *rank,
        );
    }
    let raw_docs = search_builder.len();
    let index = search_builder.build();
    eprintln!(
        "nav-build: search index built in {:.1}s — {} docs ({} before dedup)",
        search_start.elapsed().as_secs_f64(),
        index.doc_count(),
        raw_docs
    );

    // --- Write artifacts ---
    let graph_path = options.output_basename.with_extension("graph");
    let search_path = options.output_basename.with_extension("search");
    let graph_bytes = graph.serialize();
    std::fs::write(&graph_path, &graph_bytes)
        .map_err(|err| format!("write {}: {err}", graph_path.display()))?;
    let search_bytes = index.serialize();
    std::fs::write(&search_path, &search_bytes)
        .map_err(|err| format!("write {}: {err}", search_path.display()))?;
    eprintln!(
        "nav-build: done in {:.1}s\n  {} ({:.1} MB)\n  {} ({:.1} MB)",
        total_start.elapsed().as_secs_f64(),
        graph_path.display(),
        graph_bytes.len() as f64 / 1e6,
        search_path.display(),
        search_bytes.len() as f64 / 1e6,
    );
    Ok(())
}

// --- nav-probe ---

pub fn nav_probe(args: &[String]) -> Result<(), String> {
    if args.len() < 3 {
        return Err(
            "Usage: nav-probe <basename> search <query...> [--near lon,lat]\n       nav-probe <basename> route <lon,lat> <lon,lat> [--mode car|bike|foot]"
                .to_string(),
        );
    }
    let basename = Path::new(&args[1]);
    match args[2].as_str() {
        "search" => {
            let mut near = None;
            let mut terms = Vec::new();
            let mut i = 3;
            while i < args.len() {
                if args[i] == "--near" {
                    i += 1;
                    near = Some(parse_lon_lat(args.get(i).ok_or("--near needs lon,lat")?)?);
                } else {
                    terms.push(args[i].clone());
                }
                i += 1;
            }
            let query = terms.join(" ");
            let path = basename.with_extension("search");
            let load_start = Instant::now();
            let data =
                std::fs::read(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
            let index = SearchIndex::deserialize(&data).map_err(|err| err.to_string())?;
            let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;
            let query_start = Instant::now();
            let results = index.query(&query, near, 10);
            let query_ms = query_start.elapsed().as_secs_f64() * 1000.0;
            println!(
                "index: {} docs, loaded in {:.0}ms; query {:?} in {:.2}ms — {} results",
                index.doc_count(),
                load_ms,
                query,
                query_ms,
                results.len()
            );
            for r in results {
                let dist = r
                    .distance_m
                    .map(|d| format!(" [{:.1}km]", d / 1000.0))
                    .unwrap_or_default();
                let secondary = if r.secondary.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", r.secondary)
                };
                println!(
                    "  {:>6.1}  {} ({}){}{}  @ {:.5},{:.5}",
                    r.score,
                    r.name,
                    r.category.label(),
                    secondary,
                    dist,
                    r.pos.lon,
                    r.pos.lat
                );
            }
        }
        "route" => {
            if args.len() < 5 {
                return Err("nav-probe route needs two lon,lat points".to_string());
            }
            let from = parse_lon_lat(&args[3])?;
            let to = parse_lon_lat(&args[4])?;
            let mode = match args.iter().position(|a| a == "--mode") {
                Some(idx) => match args.get(idx + 1).map(|s| s.as_str()) {
                    Some("car") => TravelMode::Car,
                    Some("bike") => TravelMode::Bike,
                    Some("foot") => TravelMode::Foot,
                    other => return Err(format!("unknown mode {:?}", other)),
                },
                None => TravelMode::Car,
            };
            let path = basename.with_extension("graph");
            let load_start = Instant::now();
            let data =
                std::fs::read(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
            let graph = RouteGraph::deserialize(&data).map_err(|err| err.to_string())?;
            let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;
            let route_start = Instant::now();
            let route = graph.route(from, to, mode);
            let route_ms = route_start.elapsed().as_secs_f64() * 1000.0;
            println!(
                "graph: {} vertices / {} edges, loaded in {:.0}ms; {} routed in {:.1}ms",
                graph.vertices.len(),
                graph.edges.len(),
                load_ms,
                mode.label(),
                route_ms
            );
            match route {
                Some(route) => {
                    println!(
                        "route: {:.2} km, {:.0} min, {} points, {} maneuvers",
                        route.length_m / 1000.0,
                        route.duration_s / 60.0,
                        route.points.len(),
                        route.maneuvers.len()
                    );
                    for m in &route.maneuvers {
                        println!(
                            "  {:>6.2} km  {}  {}",
                            m.dist_m / 1000.0,
                            m.kind.arrow(),
                            m.text()
                        );
                    }
                }
                None => println!("route: NOT FOUND"),
            }
        }
        other => return Err(format!("unknown nav-probe action {:?}", other)),
    }
    Ok(())
}

fn parse_lon_lat(s: &str) -> Result<LonLat, String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 2 {
        return Err(format!("expected lon,lat — got {:?}", s));
    }
    let lon = parts[0]
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("bad lon {:?}", parts[0]))?;
    let lat = parts[1]
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("bad lat {:?}", parts[1]))?;
    Ok(LonLat::new(lon, lat))
}

pub fn parse_nav_build_options(args: &[String]) -> Result<NavBuildOptions, String> {
    if args.len() < 3 {
        return Err(
            "Usage: nav-build <source.osm.pbf> <output_basename> [--bbox west,south,east,north] [--skip-addresses]"
                .to_string(),
        );
    }
    let source = PathBuf::from(&args[1]);
    let output_basename = PathBuf::from(&args[2]);
    let mut bbox = None;
    let mut skip_addresses = false;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--bbox" => {
                i += 1;
                let value = args.get(i).ok_or("--bbox needs west,south,east,north")?;
                let parts: Vec<f64> = value
                    .split(',')
                    .map(|p| p.trim().parse::<f64>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| format!("bad bbox {:?}", value))?;
                if parts.len() != 4 {
                    return Err(format!("bad bbox {:?}", value));
                }
                bbox = Some(BboxFilter {
                    west: parts[0],
                    south: parts[1],
                    east: parts[2],
                    north: parts[3],
                });
            }
            "--skip-addresses" => skip_addresses = true,
            other => return Err(format!("unknown nav-build option {:?}", other)),
        }
        i += 1;
    }
    Ok(NavBuildOptions {
        source,
        output_basename,
        bbox,
        skip_addresses,
    })
}
