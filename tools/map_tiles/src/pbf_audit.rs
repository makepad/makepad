use osmpbf::{BlobReader, BlobType, Element, ElementReader};
use std::path::Path;
use std::time::Instant;

pub fn inspect_header(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    let mut reader =
        BlobReader::from_path(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let blob = reader
        .next()
        .ok_or_else(|| format!("{} is empty", path.display()))?
        .map_err(|err| format!("read {} header blob: {err}", path.display()))?;
    if blob.get_type() != BlobType::OsmHeader {
        return Err(format!("{} does not begin with an OSMHeader blob", path.display()));
    }
    let header = blob
        .to_headerblock()
        .map_err(|err| format!("decode {} header: {err}", path.display()))?;
    println!("source={}", path.display());
    println!("required_features={}", header.required_features().join(","));
    println!("optional_features={}", header.optional_features().join(","));
    println!(
        "sorted_type_then_id={}",
        header
            .optional_features()
            .iter()
            .any(|value| value == "Sort.Type_then_ID")
    );
    println!(
        "locations_on_ways={}",
        header
            .optional_features()
            .iter()
            .any(|value| value == "LocationsOnWays")
    );
    if let Some(bbox) = header.bbox() {
        println!(
            "bbox={:.7},{:.7},{:.7},{:.7}",
            bbox.left, bbox.bottom, bbox.right, bbox.top
        );
    }
    if let Some(program) = header.writing_program() {
        println!("writing_program={program}");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AuditStats {
    nodes: u64,
    tagged_nodes: u64,
    ways: u64,
    tagged_ways: u64,
    closed_ways: u64,
    way_node_refs: u64,
    relations: u64,
    tagged_relations: u64,
    relation_members: u64,
    tags: u64,
    benches: u64,
    playgrounds: u64,
    buildings: u64,
    building_parts: u64,
    addresses: u64,
    heights: u64,
    min_heights: u64,
    building_levels: u64,
    building_min_levels: u64,
    roof_shapes: u64,
    roof_heights: u64,
    roof_levels: u64,
    roof_directions: u64,
    roof_orientations: u64,
    roof_angles: u64,
    building_materials: u64,
    building_colours: u64,
    roof_materials: u64,
    roof_colours: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct TagStats {
    count: u64,
    bench: bool,
    playground: bool,
    building: bool,
    building_part: bool,
    address: bool,
    height: bool,
    min_height: bool,
    building_levels: bool,
    building_min_level: bool,
    roof_shape: bool,
    roof_height: bool,
    roof_levels: bool,
    roof_direction: bool,
    roof_orientation: bool,
    roof_angle: bool,
    building_material: bool,
    building_colour: bool,
    roof_material: bool,
    roof_colour: bool,
}

impl AuditStats {
    fn merge(self, other: Self) -> Self {
        Self {
            nodes: self.nodes + other.nodes,
            tagged_nodes: self.tagged_nodes + other.tagged_nodes,
            ways: self.ways + other.ways,
            tagged_ways: self.tagged_ways + other.tagged_ways,
            closed_ways: self.closed_ways + other.closed_ways,
            way_node_refs: self.way_node_refs + other.way_node_refs,
            relations: self.relations + other.relations,
            tagged_relations: self.tagged_relations + other.tagged_relations,
            relation_members: self.relation_members + other.relation_members,
            tags: self.tags + other.tags,
            benches: self.benches + other.benches,
            playgrounds: self.playgrounds + other.playgrounds,
            buildings: self.buildings + other.buildings,
            building_parts: self.building_parts + other.building_parts,
            addresses: self.addresses + other.addresses,
            heights: self.heights + other.heights,
            min_heights: self.min_heights + other.min_heights,
            building_levels: self.building_levels + other.building_levels,
            building_min_levels: self.building_min_levels + other.building_min_levels,
            roof_shapes: self.roof_shapes + other.roof_shapes,
            roof_heights: self.roof_heights + other.roof_heights,
            roof_levels: self.roof_levels + other.roof_levels,
            roof_directions: self.roof_directions + other.roof_directions,
            roof_orientations: self.roof_orientations + other.roof_orientations,
            roof_angles: self.roof_angles + other.roof_angles,
            building_materials: self.building_materials + other.building_materials,
            building_colours: self.building_colours + other.building_colours,
            roof_materials: self.roof_materials + other.roof_materials,
            roof_colours: self.roof_colours + other.roof_colours,
        }
    }

    fn add_tags(&mut self, tags: TagStats) {
        self.tags += tags.count;
        self.benches += u64::from(tags.bench);
        self.playgrounds += u64::from(tags.playground);
        self.buildings += u64::from(tags.building);
        self.building_parts += u64::from(tags.building_part);
        self.addresses += u64::from(tags.address);
        self.heights += u64::from(tags.height);
        self.min_heights += u64::from(tags.min_height);
        self.building_levels += u64::from(tags.building_levels);
        self.building_min_levels += u64::from(tags.building_min_level);
        self.roof_shapes += u64::from(tags.roof_shape);
        self.roof_heights += u64::from(tags.roof_height);
        self.roof_levels += u64::from(tags.roof_levels);
        self.roof_directions += u64::from(tags.roof_direction);
        self.roof_orientations += u64::from(tags.roof_orientation);
        self.roof_angles += u64::from(tags.roof_angle);
        self.building_materials += u64::from(tags.building_material);
        self.building_colours += u64::from(tags.building_colour);
        self.roof_materials += u64::from(tags.roof_material);
        self.roof_colours += u64::from(tags.roof_colour);
    }
}

fn inspect_tags<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> TagStats {
    let mut result = TagStats::default();
    for (key, value) in tags {
        result.count += 1;
        result.bench |= key == "amenity" && value == "bench";
        result.playground |= key == "leisure" && value == "playground";
        result.building |= key == "building";
        result.building_part |= key == "building:part";
        result.address |= key.starts_with("addr:");
        result.height |= key == "height";
        result.min_height |= key == "min_height";
        result.building_levels |= key == "building:levels";
        result.building_min_level |= key == "building:min_level";
        result.roof_shape |= key == "roof:shape";
        result.roof_height |= key == "roof:height";
        result.roof_levels |= key == "roof:levels";
        result.roof_direction |= key == "roof:direction";
        result.roof_orientation |= key == "roof:orientation";
        result.roof_angle |= key == "roof:angle";
        result.building_material |= key == "building:material";
        result.building_colour |= key == "building:colour" || key == "building:color";
        result.roof_material |= key == "roof:material";
        result.roof_colour |= key == "roof:colour" || key == "roof:color";
    }
    result
}

fn inspect_element(element: Element<'_>) -> AuditStats {
    let mut stats = AuditStats::default();
    match element {
        Element::Node(node) => {
            stats.nodes = 1;
            let tags = inspect_tags(node.tags());
            stats.tagged_nodes = u64::from(tags.count != 0);
            stats.add_tags(tags);
        }
        Element::DenseNode(node) => {
            stats.nodes = 1;
            let tags = inspect_tags(node.tags());
            stats.tagged_nodes = u64::from(tags.count != 0);
            stats.add_tags(tags);
        }
        Element::Way(way) => {
            stats.ways = 1;
            let tags = inspect_tags(way.tags());
            stats.tagged_ways = u64::from(tags.count != 0);
            stats.add_tags(tags);

            let mut first = None;
            let mut last = None;
            for node_ref in way.refs() {
                first.get_or_insert(node_ref);
                last = Some(node_ref);
                stats.way_node_refs += 1;
            }
            stats.closed_ways =
                u64::from(stats.way_node_refs > 1 && first.is_some() && first == last);
        }
        Element::Relation(relation) => {
            stats.relations = 1;
            let tags = inspect_tags(relation.tags());
            stats.tagged_relations = u64::from(tags.count != 0);
            stats.add_tags(tags);
            stats.relation_members = relation.members().count() as u64;
        }
    }
    stats
}

pub fn audit(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    let source_bytes = path
        .metadata()
        .map_err(|err| format!("stat {}: {err}", path.display()))?
        .len();
    let started = Instant::now();
    let reader =
        ElementReader::from_path(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let stats = reader
        .par_map_reduce(inspect_element, AuditStats::default, AuditStats::merge)
        .map_err(|err| format!("read {}: {err}", path.display()))?;

    println!("source={}", path.display());
    println!("source_bytes={source_bytes}");
    println!("nodes={}", stats.nodes);
    println!("tagged_nodes={}", stats.tagged_nodes);
    println!("ways={}", stats.ways);
    println!("tagged_ways={}", stats.tagged_ways);
    println!("closed_ways={}", stats.closed_ways);
    println!("way_node_refs={}", stats.way_node_refs);
    println!("relations={}", stats.relations);
    println!("tagged_relations={}", stats.tagged_relations);
    println!("relation_members={}", stats.relation_members);
    println!("tags={}", stats.tags);
    println!("amenity_bench_features={}", stats.benches);
    println!("leisure_playground_features={}", stats.playgrounds);
    println!("building_tag_features={}", stats.buildings);
    println!("building_part_features={}", stats.building_parts);
    println!("addressed_features={}", stats.addresses);
    println!("height_features={}", stats.heights);
    println!("min_height_features={}", stats.min_heights);
    println!("building_levels_features={}", stats.building_levels);
    println!(
        "building_min_level_features={}",
        stats.building_min_levels
    );
    println!("roof_shape_features={}", stats.roof_shapes);
    println!("roof_height_features={}", stats.roof_heights);
    println!("roof_levels_features={}", stats.roof_levels);
    println!("roof_direction_features={}", stats.roof_directions);
    println!("roof_orientation_features={}", stats.roof_orientations);
    println!("roof_angle_features={}", stats.roof_angles);
    println!("building_material_features={}", stats.building_materials);
    println!("building_colour_features={}", stats.building_colours);
    println!("roof_material_features={}", stats.roof_materials);
    println!("roof_colour_features={}", stats.roof_colours);
    println!("elapsed_seconds={:.3}", started.elapsed().as_secs_f64());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_audit_counts_features_once_and_all_tags() {
        let tags = [
            ("amenity", "bench"),
            ("leisure", "playground"),
            ("building", "yes"),
            ("building:part", "yes"),
            ("addr:housenumber", "12"),
            ("height", "12.5"),
            ("building:levels", "4"),
            ("roof:shape", "gabled"),
            ("name", "Example"),
        ];
        let result = inspect_tags(tags.into_iter());
        assert_eq!(result.count, 9);
        assert!(result.bench);
        assert!(result.playground);
        assert!(result.building);
        assert!(result.building_part);
        assert!(result.address);
        assert!(result.height);
        assert!(result.building_levels);
        assert!(result.roof_shape);
    }
}
