mod geom;
pub(crate) mod mvt;
mod spool;
mod store;

use flate2::write::GzEncoder;
use flate2::Compression;
use geom::{
    emit_lines, emit_point, emit_polygons, group_polygon_rings, project_decimicro, project_node,
    project_path, PolygonPart, SourcePath,
};
use makepad_mbtile_reader::MbtilesWriter;
use mvt::{encode_tile, Layer, OsmType, TagPair};
use osmpbf::{BlobDecode, BlobReader, Element, RelMemberType};
use smallvec::SmallVec;
use spool::{records_to_tiles, BlockSpoolWriter, SortedBlock};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::hash::{BuildHasherDefault, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use store::{
    NodeCoord, NodeStore, NodeStoreBuilder, PagedBitset, PagedBitsetWriter, WayStore,
    WayStoreBuilder,
};

const DEFAULT_ZOOM: u8 = 14;
type SourceTags<'a> = SmallVec<[(Cow<'a, str>, Cow<'a, str>); 8]>;

#[derive(Default)]
struct FastHasher(u64);

impl Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut hash = self.0 ^ 0xcbf2_9ce4_8422_2325;
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        self.0 = hash;
    }

    fn write_u64(&mut self, value: u64) {
        let mut mixed = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^= mixed >> 31;
        self.0 = self.0.rotate_left(27) ^ mixed;
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }
}

type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

#[derive(Clone, Debug)]
pub struct DetailOptions {
    pub source: PathBuf,
    pub output: PathBuf,
    pub store: PathBuf,
    pub zoom: u8,
    pub sort_memory_mib: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct ConversionStats {
    nodes: u64,
    tagged_nodes: u64,
    ways: u64,
    tagged_ways: u64,
    relations: u64,
    tagged_relations: u64,
    relation_way_members: u64,
    relation_node_members: u64,
    relation_relation_members: u64,
    source_tags: u64,
    building: u64,
    building_part: u64,
    height: u64,
    min_height: u64,
    building_levels: u64,
    building_min_level: u64,
    roof_shape: u64,
    roof_height: u64,
    roof_levels: u64,
    roof_direction: u64,
    roof_orientation: u64,
    roof_angle: u64,
    building_material: u64,
    building_colour: u64,
    roof_material: u64,
    roof_colour: u64,
    node_tile_records: u64,
    way_line_tile_records: u64,
    way_polygon_tile_records: u64,
    relation_point_tile_records: u64,
    relation_line_tile_records: u64,
    relation_polygon_tile_records: u64,
    missing_relation_nodes: u64,
    missing_relation_ways: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct TagFlags {
    count: u64,
    building: bool,
    building_part: bool,
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

impl ConversionStats {
    fn add_tags(&mut self, flags: TagFlags) {
        self.source_tags += flags.count;
        self.building += u64::from(flags.building);
        self.building_part += u64::from(flags.building_part);
        self.height += u64::from(flags.height);
        self.min_height += u64::from(flags.min_height);
        self.building_levels += u64::from(flags.building_levels);
        self.building_min_level += u64::from(flags.building_min_level);
        self.roof_shape += u64::from(flags.roof_shape);
        self.roof_height += u64::from(flags.roof_height);
        self.roof_levels += u64::from(flags.roof_levels);
        self.roof_direction += u64::from(flags.roof_direction);
        self.roof_orientation += u64::from(flags.roof_orientation);
        self.roof_angle += u64::from(flags.roof_angle);
        self.building_material += u64::from(flags.building_material);
        self.building_colour += u64::from(flags.building_colour);
        self.roof_material += u64::from(flags.roof_material);
        self.roof_colour += u64::from(flags.roof_colour);
    }

    fn report(self, source: &Path, zoom: u8) -> String {
        format!(
            "\
source={}
detail_zoom={}
nodes={}
tagged_nodes={}
ways={}
tagged_ways={}
relations={}
tagged_relations={}
relation_way_members={}
relation_node_members={}
relation_relation_members={}
source_tags={}
building_features={}
building_part_features={}
height_features={}
min_height_features={}
building_levels_features={}
building_min_level_features={}
roof_shape_features={}
roof_height_features={}
roof_levels_features={}
roof_direction_features={}
roof_orientation_features={}
roof_angle_features={}
building_material_features={}
building_colour_features={}
roof_material_features={}
roof_colour_features={}
node_tile_records={}
way_line_tile_records={}
way_polygon_tile_records={}
relation_point_tile_records={}
relation_line_tile_records={}
relation_polygon_tile_records={}
missing_relation_nodes={}
missing_relation_ways={}
",
            source.display(),
            zoom,
            self.nodes,
            self.tagged_nodes,
            self.ways,
            self.tagged_ways,
            self.relations,
            self.tagged_relations,
            self.relation_way_members,
            self.relation_node_members,
            self.relation_relation_members,
            self.source_tags,
            self.building,
            self.building_part,
            self.height,
            self.min_height,
            self.building_levels,
            self.building_min_level,
            self.roof_shape,
            self.roof_height,
            self.roof_levels,
            self.roof_direction,
            self.roof_orientation,
            self.roof_angle,
            self.building_material,
            self.building_colour,
            self.roof_material,
            self.roof_colour,
            self.node_tile_records,
            self.way_line_tile_records,
            self.way_polygon_tile_records,
            self.relation_point_tile_records,
            self.relation_line_tile_records,
            self.relation_polygon_tile_records,
            self.missing_relation_nodes,
            self.missing_relation_ways,
        )
    }
}

struct NativePaths {
    relation_ways: PathBuf,
    node_data: PathBuf,
    node_index: PathBuf,
    way_data: PathBuf,
    way_index: PathBuf,
    spool: PathBuf,
    audit: PathBuf,
    complete: PathBuf,
}

impl NativePaths {
    fn new(store: &Path) -> Self {
        Self {
            relation_ways: store.join("relation-ways.bits"),
            node_data: store.join("nodes.dat"),
            node_index: store.join("nodes.idx"),
            way_data: store.join("ways.dat"),
            way_index: store.join("ways.idx"),
            spool: store.join("spool"),
            audit: store.join("native-detail-audit.txt"),
            complete: store.join("spool.complete.json"),
        }
    }
}

pub fn convert_detail(options: DetailOptions) -> Result<(), String> {
    validate_detail_options(&options)?;
    let header = read_pbf_header(&options.source)?;
    if !header.sorted_type_then_id {
        return Err(format!(
            "{} does not advertise Sort.Type_then_ID; the bounded native store requires a type/id-sorted PBF",
            options.source.display()
        ));
    }
    if options.store.exists() {
        return finish_existing_detail(&options, &header);
    }

    if let Some(parent) = options.store.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    fs::create_dir(&options.store)
        .map_err(|err| format!("create native store {}: {err}", options.store.display()))?;
    let paths = NativePaths::new(&options.store);
    let started = Instant::now();
    let mut stats = ConversionStats::default();

    println!("Native OSM detail conversion");
    println!("  source: {}", options.source.display());
    println!("  output: {}", options.output.display());
    println!("  store:  {}", options.store.display());
    println!("  zoom:   {}", options.zoom);
    println!("Pass 1/5: indexing relation member ways");
    let stage_started = Instant::now();
    mark_relation_ways(&options.source, &paths.relation_ways, &mut stats)?;
    println!("  pass 1 completed in {:.1}s", stage_started.elapsed().as_secs_f64());

    println!("Pass 2/5: writing compressed node store and tagged node features");
    let stage_started = Instant::now();
    let mut spool = BlockSpoolWriter::create(&paths.spool)?;
    build_nodes(
        &options.source,
        &paths.node_data,
        &paths.node_index,
        options.zoom,
        &mut spool,
        &mut stats,
    )?;
    println!("  pass 2 completed in {:.1}s", stage_started.elapsed().as_secs_f64());

    println!("Pass 3/5: resolving ways and writing tagged way features");
    let stage_started = Instant::now();
    build_ways(
        &options.source,
        &paths,
        options.zoom,
        &mut spool,
        &mut stats,
    )?;
    println!("  pass 3 completed in {:.1}s", stage_started.elapsed().as_secs_f64());

    println!("Pass 4/5: assembling tagged relation geometries");
    let stage_started = Instant::now();
    build_relations(
        &options.source,
        &paths,
        options.zoom,
        &mut spool,
        &mut stats,
    )?;
    println!("  pass 4 completed in {:.1}s", stage_started.elapsed().as_secs_f64());
    let spool = spool.finish()?;
    println!(
        "Spool: {} blocks, {} records, {:.2} GiB",
        spool.blocks.len(),
        spool.records,
        spool.bytes as f64 / 1_073_741_824.0
    );

    let report = stats.report(&options.source, options.zoom);
    fs::write(&paths.audit, &report)
        .map_err(|err| format!("write {}: {err}", paths.audit.display()))?;
    write_complete_marker(&options, &paths, &spool)?;

    println!("Pass 5/5: external-sort blocks and stream MBTiles");
    let stage_started = Instant::now();
    let output_stats = finish_tiles(&options, &spool, header.bounds)?;
    println!("  pass 5 completed in {:.1}s", stage_started.elapsed().as_secs_f64());
    print!("{report}");
    println!(
        "Done: {} tiles, {:.2} GiB payload, {:.2} GiB file in {:.1}s",
        output_stats.tile_count,
        output_stats.tile_bytes as f64 / 1_073_741_824.0,
        output_stats.file_bytes as f64 / 1_073_741_824.0,
        started.elapsed().as_secs_f64()
    );
    println!("Scratch retained at {}", options.store.display());
    Ok(())
}

fn write_complete_marker(
    options: &DetailOptions,
    paths: &NativePaths,
    spool: &spool::SpoolSummary,
) -> Result<(), String> {
    let source_bytes = options
        .source
        .metadata()
        .map_err(|err| format!("stat {}: {err}", options.source.display()))?
        .len();
    let marker = serde_json::json!({
        "format": "makepad-native-detail-spool-v1",
        "source": options.source.display().to_string(),
        "source_bytes": source_bytes,
        "zoom": options.zoom,
        "blocks": spool.blocks.len(),
        "records": spool.records,
        "spool_bytes": spool.bytes,
    });
    let bytes = serde_json::to_vec_pretty(&marker)
        .map_err(|err| format!("serialize {}: {err}", paths.complete.display()))?;
    let partial = options.store.join("spool.complete.partial");
    fs::write(&partial, bytes).map_err(|err| format!("write {}: {err}", partial.display()))?;
    fs::rename(&partial, &paths.complete).map_err(|err| {
        format!(
            "rename {} to {}: {err}",
            partial.display(),
            paths.complete.display()
        )
    })
}

fn finish_existing_detail(
    options: &DetailOptions,
    header: &PbfHeaderInfo,
) -> Result<(), String> {
    if !options.store.is_dir() {
        return Err(format!(
            "{} exists but is not a native scratch directory",
            options.store.display()
        ));
    }
    let paths = NativePaths::new(&options.store);
    let marker_bytes = fs::read(&paths.complete).map_err(|err| {
        format!(
            "{} is incomplete and cannot be resumed (read {}: {err})",
            options.store.display(),
            paths.complete.display()
        )
    })?;
    let marker: serde_json::Value = serde_json::from_slice(&marker_bytes)
        .map_err(|err| format!("parse {}: {err}", paths.complete.display()))?;
    if marker.get("format").and_then(|value| value.as_str())
        != Some("makepad-native-detail-spool-v1")
    {
        return Err(format!(
            "{} has an unsupported native detail marker",
            paths.complete.display()
        ));
    }
    let marker_zoom = marker
        .get("zoom")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| format!("{} has no zoom", paths.complete.display()))?;
    if marker_zoom != u64::from(options.zoom) {
        return Err(format!(
            "scratch zoom {marker_zoom} does not match requested zoom {}",
            options.zoom
        ));
    }
    let source_bytes = options
        .source
        .metadata()
        .map_err(|err| format!("stat {}: {err}", options.source.display()))?
        .len();
    let marker_source_bytes = marker
        .get("source_bytes")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| format!("{} has no source_bytes", paths.complete.display()))?;
    if marker_source_bytes != source_bytes {
        return Err(format!(
            "scratch source size {marker_source_bytes} does not match {} bytes for {}",
            source_bytes,
            options.source.display()
        ));
    }
    let spool = spool::SpoolSummary::from_dir(&paths.spool)?;
    if spool.blocks.is_empty() {
        return Err(format!("{} contains no completed tile blocks", paths.spool.display()));
    }
    let started = Instant::now();
    println!("Resuming native detail output from completed scratch");
    println!("  source: {}", options.source.display());
    println!("  output: {}", options.output.display());
    println!("  store:  {}", options.store.display());
    println!("  zoom:   {}", options.zoom);
    println!("  blocks: {}", spool.blocks.len());
    let output_stats = finish_tiles(options, &spool, header.bounds)?;
    if let Ok(report) = fs::read_to_string(&paths.audit) {
        print!("{report}");
    }
    println!(
        "Done: {} tiles, {:.2} GiB payload, {:.2} GiB file in {:.1}s",
        output_stats.tile_count,
        output_stats.tile_bytes as f64 / 1_073_741_824.0,
        output_stats.file_bytes as f64 / 1_073_741_824.0,
        started.elapsed().as_secs_f64()
    );
    println!("Scratch retained at {}", options.store.display());
    Ok(())
}

fn validate_detail_options(options: &DetailOptions) -> Result<(), String> {
    if !options.source.is_file() {
        return Err(format!("{} is not a file", options.source.display()));
    }
    if options.output.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite it",
            options.output.display()
        ));
    }
    if options.output.is_dir() {
        return Err(format!("{} is a directory", options.output.display()));
    }
    if !(1..=22).contains(&options.zoom) {
        return Err(format!("detail zoom {} is outside 1..=22", options.zoom));
    }
    if options.sort_memory_mib == 0 {
        return Err("--sort-memory-mib must be at least 1".to_string());
    }
    if options.source == options.output {
        return Err("source and output paths must differ".to_string());
    }
    if let Some(parent) = options.output.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct PbfHeaderInfo {
    sorted_type_then_id: bool,
    bounds: Option<[f64; 4]>,
}

fn read_pbf_header(path: &Path) -> Result<PbfHeaderInfo, String> {
    let mut reader =
        BlobReader::from_path(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let blob = reader
        .next()
        .ok_or_else(|| format!("{} is empty", path.display()))?
        .map_err(|err| format!("read {} header: {err}", path.display()))?;
    let header = blob
        .to_headerblock()
        .map_err(|err| format!("decode {} header: {err}", path.display()))?;
    Ok(PbfHeaderInfo {
        sorted_type_then_id: header
            .optional_features()
            .iter()
            .any(|feature| feature == "Sort.Type_then_ID"),
        bounds: header
            .bbox()
            .map(|bbox| [bbox.left, bbox.bottom, bbox.right, bbox.top]),
    })
}

/// Ordered parallel pbf visitor: blob READS stay serial (cheap IO), the
/// expensive zlib+protobuf DECODE fans out over a worker pool, and the
/// callback runs on the calling thread in exact file order — so store
/// builders that rely on id-sorted input need no changes. This is where
/// the old converter spent most of its 10+ hours: one core decoding.
fn visit_pbf<F>(path: &Path, mut callback: F) -> Result<(), String>
where
    F: for<'a> FnMut(Element<'a>) -> Result<(), String>,
{
    use osmpbf::PrimitiveBlock;
    use std::collections::BinaryHeap;
    use std::sync::mpsc::sync_channel;
    use std::sync::{Arc, Mutex};

    let workers = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2).max(2))
        .unwrap_or(4);
    let (blob_tx, blob_rx) = sync_channel::<(u64, osmpbf::Blob)>(workers * 4);
    let blob_rx = Arc::new(Mutex::new(blob_rx));
    let (block_tx, block_rx) =
        sync_channel::<(u64, Result<Option<PrimitiveBlock>, String>)>(workers * 4);

    std::thread::scope(|scope| -> Result<(), String> {
        let path_owned = path.to_path_buf();
        let reader_handle = scope.spawn(move || -> Result<(), String> {
            let reader = BlobReader::from_path(&path_owned)
                .map_err(|err| format!("open {}: {err}", path_owned.display()))?;
            for (seq, blob) in reader.enumerate() {
                let blob =
                    blob.map_err(|err| format!("read {}: {err}", path_owned.display()))?;
                if blob_tx.send((seq as u64, blob)).is_err() {
                    break;
                }
            }
            Ok(())
        });
        for _ in 0..workers {
            let blob_rx = Arc::clone(&blob_rx);
            let block_tx = block_tx.clone();
            scope.spawn(move || {
                loop {
                    let received = { blob_rx.lock().unwrap().recv() };
                    let Ok((seq, blob)) = received else {
                        break;
                    };
                    let decoded = match blob.decode() {
                        Ok(BlobDecode::OsmData(block)) => Ok(Some(block)),
                        Ok(BlobDecode::OsmHeader(_)) | Ok(BlobDecode::Unknown(_)) => Ok(None),
                        Err(err) => Err(format!("decode: {err}")),
                    };
                    if block_tx.send((seq, decoded)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(block_tx);

        // Reassemble in order; apply the callback serially.
        let mut pending = BinaryHeap::<std::cmp::Reverse<(u64, u64)>>::new();
        let mut stash =
            std::collections::HashMap::<u64, Result<Option<PrimitiveBlock>, String>>::new();
        let mut next_seq = 0u64;
        for (seq, decoded) in block_rx.iter() {
            stash.insert(seq, decoded);
            pending.push(std::cmp::Reverse((seq, seq)));
            while stash.contains_key(&next_seq) {
                let decoded = stash.remove(&next_seq).unwrap();
                next_seq += 1;
                if let Some(block) = decoded? {
                    for element in block.elements() {
                        callback(element)?;
                    }
                }
            }
        }
        // On callback error the receivers drop and threads unwind on their
        // own; surface the reader error if any.
        match reader_handle.join() {
            Ok(result) => result,
            Err(_) => Err("pbf reader thread panicked".to_string()),
        }
    })
}

fn mark_relation_ways(
    source: &Path,
    output: &Path,
    stats: &mut ConversionStats,
) -> Result<(), String> {
    let mut bitset = PagedBitsetWriter::create(output)?;
    let mut progress = Progress::new("relations scanned");
    visit_pbf(source, |element| {
        if let Element::Relation(relation) = element {
            progress.tick(1);
            for member in relation.members() {
                match member.member_type {
                    RelMemberType::Way => {
                        bitset.set(member.member_id)?;
                        stats.relation_way_members += 1;
                    }
                    RelMemberType::Node => stats.relation_node_members += 1,
                    RelMemberType::Relation => stats.relation_relation_members += 1,
                }
            }
        }
        Ok(())
    })?;
    progress.finish();
    bitset.finish()
}

fn build_nodes(
    source: &Path,
    data_path: &Path,
    index_path: &Path,
    zoom: u8,
    spool: &mut BlockSpoolWriter,
    stats: &mut ConversionStats,
) -> Result<(), String> {
    let mut builder = NodeStoreBuilder::create(data_path, index_path)?;
    let mut progress = Progress::new("nodes stored");
    visit_pbf(source, |element| {
        let (id, lon, lat, tags) = match element {
            Element::Node(node) => (
                node.id(),
                node.decimicro_lon(),
                node.decimicro_lat(),
                collect_tags(node.tags()),
            ),
            Element::DenseNode(node) => (
                node.id,
                node.decimicro_lon(),
                node.decimicro_lat(),
                collect_tags(node.tags()),
            ),
            _ => return Ok(()),
        };
        let node = project_decimicro(id, lon, lat, zoom);
        builder.push(node)?;
        stats.nodes += 1;
        progress.tick(1);
        if !tags.is_empty() {
            stats.tagged_nodes += 1;
            stats.add_tags(inspect_tag_flags(&tags));
            stats.node_tile_records += emit_point(
                spool,
                zoom,
                Layer::OsmPoints,
                OsmType::Node,
                node.id,
                &tags,
                project_node(node),
            )?;
        }
        Ok(())
    })?;
    progress.finish();
    let count = builder.finish()?;
    if count != stats.nodes {
        return Err(format!(
            "node store wrote {count} nodes, counted {}",
            stats.nodes
        ));
    }
    Ok(())
}

/// One tagged way handed to a resolver worker.
struct WayJob {
    id: i64,
    refs: Vec<i64>,
    tags: Vec<(String, String)>,
}

/// Resolved features from a worker, ready for the single spool writer.
struct PreparedBatch {
    tags: Vec<(String, String)>,
    features: Vec<geom::PreparedFeature>,
}

fn build_ways(
    source: &Path,
    paths: &NativePaths,
    zoom: u8,
    spool: &mut BlockSpoolWriter,
    stats: &mut ConversionStats,
) -> Result<(), String> {
    use std::sync::mpsc::sync_channel;
    use std::sync::{Arc, Mutex};

    let mut relation_ways = PagedBitset::open(&paths.relation_ways)?;
    let mut ways = WayStoreBuilder::create(&paths.way_data, &paths.way_index)?;
    let mut progress = Progress::new("ways resolved");

    // Way resolution parallelizes cleanly: the spool is order-free (pass 5
    // external-sorts it) and only the relation-member way store needs
    // id-ordered pushes, which stay on this thread.
    let workers = std::thread::available_parallelism()
        .map(|n| (n.get().saturating_sub(3)).clamp(4, 12))
        .unwrap_or(4);
    eprintln!("  loading node store into RAM...");
    let load_start = std::time::Instant::now();
    let flat_nodes = std::sync::Arc::new(crate::native::store::FlatNodeStore::load(
        &paths.node_data,
        &paths.node_index,
    )?);
    eprintln!(
        "  node store loaded in {:.1}s",
        load_start.elapsed().as_secs_f64()
    );
    let (job_tx, job_rx) = sync_channel::<WayJob>(8192);
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (out_tx, out_rx) = sync_channel::<Result<PreparedBatch, String>>(8192);

    let result: Result<(u64, u64, u64, u64), String> = std::thread::scope(|scope| {
        for _ in 0..workers {
            let job_rx = Arc::clone(&job_rx);
            let out_tx = out_tx.clone();
            let flat_nodes = Arc::clone(&flat_nodes);
            scope.spawn(move || {
                loop {
                    let job = { job_rx.lock().unwrap().recv() };
                    let Ok(job) = job else {
                        break;
                    };
                    let resolved = (|| -> Result<PreparedBatch, String> {
                        let (projected, closed) = resolve_projected_refs_flat(
                            &flat_nodes,
                            job.refs.iter().copied(),
                            job.id,
                        )?;
                        let mut features = Vec::new();
                        geom::prepare_lines(
                            zoom,
                            Layer::OsmLines,
                            OsmType::Way,
                            job.id,
                            closed,
                            std::slice::from_ref(&projected),
                            &mut features,
                        )?;
                        if closed && projected.len() >= 4 {
                            geom::prepare_polygons(
                                zoom,
                                Layer::OsmPolygons,
                                OsmType::Way,
                                job.id,
                                &[PolygonPart {
                                    outer: projected,
                                    holes: Vec::new(),
                                }],
                                &mut features,
                            )?;
                        }
                        Ok(PreparedBatch {
                            tags: job.tags,
                            features,
                        })
                    })();
                    if out_tx.send(resolved).is_err() {
                        break;
                    }
                }
            });
        }
        drop(out_tx);

        // Writer thread: owns the spool, appends prepared features.
        let writer = scope.spawn(move || -> Result<(u64, u64), String> {
            let mut line_records = 0u64;
            let mut polygon_records = 0u64;
            for batch in out_rx.iter() {
                let batch = batch?;
                for feature in &batch.features {
                    spool.push_parts(
                        feature.tile_x,
                        feature.tile_y,
                        feature.layer,
                        feature.geometry_type,
                        feature.osm_type,
                        feature.id,
                        feature.closed,
                        &batch.tags,
                        feature.paths.iter().map(Vec::as_slice),
                    )?;
                    match feature.geometry_type {
                        crate::native::mvt::GeometryType::Polygon => polygon_records += 1,
                        _ => line_records += 1,
                    }
                }
            }
            Ok((line_records, polygon_records))
        });

        // Consumer: ordered pbf visit; dispatch tagged ways, keep the
        // id-ordered way store here.
        let mut total_ways = 0u64;
        let mut tagged_ways = 0u64;
        let visit_result = visit_pbf(source, |element| {
            let Element::Way(way) = element else {
                return Ok(());
            };
            total_ways += 1;
            progress.tick(1);
            let relation_member = relation_ways.contains(way.id())?;
            let tags = collect_tags(way.tags());
            if tags.is_empty() {
                if relation_member {
                    let refs = way.refs().collect::<Vec<_>>();
                    ways.push(way.id(), refs)?;
                }
                return Ok(());
            }
            tagged_ways += 1;
            stats.add_tags(inspect_tag_flags(&tags));
            let refs = way.refs().collect::<Vec<i64>>();
            if relation_member {
                ways.push(way.id(), refs.clone())?;
            }
            let owned_tags: Vec<(String, String)> = tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            job_tx
                .send(WayJob {
                    id: way.id(),
                    refs,
                    tags: owned_tags,
                })
                .map_err(|_| "way resolver workers exited early".to_string())?;
            Ok(())
        });
        drop(job_tx);
        let (line_records, polygon_records) = match writer.join() {
            Ok(result) => result?,
            Err(_) => return Err("spool writer thread panicked".to_string()),
        };
        visit_result?;
        Ok((total_ways, tagged_ways, line_records, polygon_records))
    });
    let (total_ways, tagged_ways, line_records, polygon_records) = result?;
    stats.ways += total_ways;
    stats.tagged_ways += tagged_ways;
    stats.way_line_tile_records += line_records;
    stats.way_polygon_tile_records += polygon_records;
    progress.finish();
    ways.finish()?;
    Ok(())
}

/// One tagged relation handed to an assembler worker.
struct RelationJob {
    id: i64,
    tags: Vec<(String, String)>,
    /// (member kind: 0 node / 1 way, id, role_is_inner)
    members: Vec<(u8, i64, bool)>,
    is_multipolygon: bool,
}

struct RelationOut {
    batch: PreparedBatch,
    missing_nodes: u64,
    missing_ways: u64,
}

fn build_relations(
    source: &Path,
    paths: &NativePaths,
    zoom: u8,
    spool: &mut BlockSpoolWriter,
    stats: &mut ConversionStats,
) -> Result<(), String> {
    use std::sync::mpsc::sync_channel;
    use std::sync::{Arc, Mutex};

    eprintln!("  loading node + way stores into RAM...");
    let load_start = std::time::Instant::now();
    let flat_nodes = Arc::new(crate::native::store::FlatNodeStore::load(
        &paths.node_data,
        &paths.node_index,
    )?);
    let flat_ways = Arc::new(crate::native::store::FlatWayStore::load(
        &paths.way_data,
        &paths.way_index,
    )?);
    eprintln!(
        "  stores loaded in {:.1}s",
        load_start.elapsed().as_secs_f64()
    );
    let mut progress = Progress::new("relations assembled");
    let workers = std::thread::available_parallelism()
        .map(|n| (n.get().saturating_sub(3)).clamp(4, 12))
        .unwrap_or(4);
    let (job_tx, job_rx) = sync_channel::<RelationJob>(4096);
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (out_tx, out_rx) = sync_channel::<Result<RelationOut, String>>(4096);

    let result: Result<(u64, u64, u64, u64, u64, u64, u64), String> =
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let job_rx = Arc::clone(&job_rx);
                let out_tx = out_tx.clone();
                let flat_nodes = Arc::clone(&flat_nodes);
                let flat_ways = Arc::clone(&flat_ways);
                scope.spawn(move || {
                    loop {
                        let job = { job_rx.lock().unwrap().recv() };
                        let Ok(job) = job else {
                            break;
                        };
                        let assembled = (|| -> Result<RelationOut, String> {
                            let mut missing_nodes = 0u64;
                            let mut missing_ways = 0u64;
                            let mut features = Vec::new();
                            let mut outer = Vec::<SourcePath>::new();
                            let mut inner = Vec::<SourcePath>::new();
                            let mut lines = Vec::<Vec<geom::GlobalPoint>>::new();
                            for &(kind, member_id, is_inner) in &job.members {
                                if kind == 0 {
                                    if let Some(node) = flat_nodes.get(member_id)? {
                                        geom::prepare_point(
                                            zoom,
                                            Layer::OsmRelationPoints,
                                            OsmType::Relation,
                                            job.id,
                                            project_node(node),
                                            &mut features,
                                        )?;
                                    } else {
                                        missing_nodes += 1;
                                    }
                                    continue;
                                }
                                let Some(refs) = flat_ways.get(member_id)? else {
                                    missing_ways += 1;
                                    continue;
                                };
                                let mut coordinates = Vec::with_capacity(refs.len());
                                for &node_id in refs {
                                    let Some(node) = flat_nodes.get(node_id)? else {
                                        return Err(format!(
                                            "OSM object {} references missing node {node_id}",
                                            member_id
                                        ));
                                    };
                                    coordinates.push(node);
                                }
                                let source_path = SourcePath {
                                    nodes: coordinates,
                                };
                                lines.push(project_path(&source_path.nodes));
                                if is_inner {
                                    inner.push(source_path);
                                } else {
                                    outer.push(source_path);
                                }
                            }
                            geom::prepare_lines(
                                zoom,
                                Layer::OsmRelationLines,
                                OsmType::Relation,
                                job.id,
                                false,
                                &lines,
                                &mut features,
                            )?;
                            if job.is_multipolygon {
                                let (polygons, _) = group_polygon_rings(outer, inner);
                                geom::prepare_polygons(
                                    zoom,
                                    Layer::OsmRelationPolygons,
                                    OsmType::Relation,
                                    job.id,
                                    &polygons,
                                    &mut features,
                                )?;
                            }
                            Ok(RelationOut {
                                batch: PreparedBatch {
                                    tags: job.tags,
                                    features,
                                },
                                missing_nodes,
                                missing_ways,
                            })
                        })();
                        if out_tx.send(assembled).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(out_tx);

            let writer = scope.spawn(move || -> Result<(u64, u64, u64, u64, u64), String> {
                let mut point_records = 0u64;
                let mut line_records = 0u64;
                let mut polygon_records = 0u64;
                let mut missing_nodes = 0u64;
                let mut missing_ways = 0u64;
                for out in out_rx.iter() {
                    let out = out?;
                    missing_nodes += out.missing_nodes;
                    missing_ways += out.missing_ways;
                    for feature in &out.batch.features {
                        spool.push_parts(
                            feature.tile_x,
                            feature.tile_y,
                            feature.layer,
                            feature.geometry_type,
                            feature.osm_type,
                            feature.id,
                            feature.closed,
                            &out.batch.tags,
                            feature.paths.iter().map(Vec::as_slice),
                        )?;
                        match feature.geometry_type {
                            crate::native::mvt::GeometryType::Point => point_records += 1,
                            crate::native::mvt::GeometryType::Polygon => polygon_records += 1,
                            _ => line_records += 1,
                        }
                    }
                }
                Ok((
                    point_records,
                    line_records,
                    polygon_records,
                    missing_nodes,
                    missing_ways,
                ))
            });

            let mut total = 0u64;
            let mut tagged = 0u64;
            let visit_result = visit_pbf(source, |element| {
                let Element::Relation(relation) = element else {
                    return Ok(());
                };
                total += 1;
                progress.tick(1);
                let tags = collect_tags(relation.tags());
                if tags.is_empty() {
                    return Ok(());
                }
                tagged += 1;
                stats.add_tags(inspect_tag_flags(&tags));
                let is_multipolygon = tag_value(&tags, "type") == Some("multipolygon");
                let mut members = Vec::new();
                let mut nested = 0u64;
                for member in relation.members() {
                    let role = member.role().map_err(|err| {
                        format!("read relation {} member role: {err}", relation.id())
                    })?;
                    match member.member_type {
                        RelMemberType::Node => members.push((0u8, member.member_id, false)),
                        RelMemberType::Way => {
                            members.push((1u8, member.member_id, role == "inner"))
                        }
                        RelMemberType::Relation => nested += 1,
                    }
                }
                let mut owned_tags: Vec<(String, String)> = tags
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                if nested != 0 {
                    owned_tags.push((
                        "__makepad_nested_relation_members".to_string(),
                        nested.to_string(),
                    ));
                }
                job_tx
                    .send(RelationJob {
                        id: relation.id(),
                        tags: owned_tags,
                        members,
                        is_multipolygon,
                    })
                    .map_err(|_| "relation assembler workers exited early".to_string())?;
                Ok(())
            });
            drop(job_tx);
            let (points, lines, polygons, missing_nodes, missing_ways) = match writer.join() {
                Ok(result) => result?,
                Err(_) => return Err("relation spool writer panicked".to_string()),
            };
            visit_result?;
            Ok((
                total,
                tagged,
                points,
                lines,
                polygons,
                missing_nodes,
                missing_ways,
            ))
        });
    let (total, tagged, points, lines, polygons, missing_nodes, missing_ways) = result?;
    stats.relations += total;
    stats.tagged_relations += tagged;
    stats.relation_point_tile_records += points;
    stats.relation_line_tile_records += lines;
    stats.relation_polygon_tile_records += polygons;
    stats.missing_relation_nodes += missing_nodes;
    stats.missing_relation_ways += missing_ways;
    progress.finish();
    Ok(())
}

fn resolve_nodes(
    store: &mut NodeStore,
    refs: &[i64],
    object_id: i64,
) -> Result<Vec<NodeCoord>, String> {
    let mut result = Vec::with_capacity(refs.len());
    for &node_id in refs {
        let Some(node) = store.get(node_id)? else {
            return Err(format!(
                "OSM object {object_id} references missing node {node_id}"
            ));
        };
        result.push(node);
    }
    Ok(result)
}

fn resolve_projected_refs_flat(
    store: &crate::native::store::FlatNodeStore,
    refs: impl Iterator<Item = i64>,
    object_id: i64,
) -> Result<(Vec<geom::GlobalPoint>, bool), String> {
    let (minimum, maximum) = refs.size_hint();
    let mut result = Vec::with_capacity(maximum.unwrap_or(minimum));
    let mut first_id = None;
    let mut last_id = None;
    let mut count = 0_usize;
    for node_id in refs {
        let Some(node) = store.get(node_id)? else {
            return Err(format!(
                "OSM object {object_id} references missing node {node_id}"
            ));
        };
        first_id.get_or_insert(node_id);
        last_id = Some(node_id);
        count += 1;
        let point = project_node(node);
        if result.last() != Some(&point) {
            result.push(point);
        }
    }
    let closed = count > 2 && first_id == last_id;
    Ok((result, closed))
}

fn resolve_projected_refs(
    store: &mut NodeStore,
    refs: impl Iterator<Item = i64>,
    object_id: i64,
) -> Result<(Vec<geom::GlobalPoint>, bool), String> {
    let (minimum, maximum) = refs.size_hint();
    let mut result = Vec::with_capacity(maximum.unwrap_or(minimum));
    let mut first_id = None;
    let mut last_id = None;
    let mut count = 0_usize;
    for node_id in refs {
        let Some(node) = store.get(node_id)? else {
            return Err(format!(
                "OSM object {object_id} references missing node {node_id}"
            ));
        };
        first_id.get_or_insert(node_id);
        last_id = Some(node_id);
        count += 1;
        let point = project_node(node);
        if result.last() != Some(&point) {
            result.push(point);
        }
    }
    Ok((result, count > 2 && first_id == last_id))
}

fn finish_tiles(
    options: &DetailOptions,
    spool: &spool::SpoolSummary,
    bounds: Option<[f64; 4]>,
) -> Result<makepad_mbtile_reader::MbtilesWriterStats, String> {
    let mut writer = MbtilesWriter::create(&options.output)
        .map_err(|err| format!("create {}: {err}", options.output.display()))?;
    writer.set_metadata("name", "Makepad native all-tag OSM detail");
    writer.set_metadata(
        "description",
        "All tagged spatial OSM elements with original tags and IDs",
    );
    writer.set_metadata("type", "overlay");
    writer.set_metadata("version", "1");
    writer.set_metadata("format", "pbf");
    writer.set_metadata("scheme", "tms");
    writer.set_metadata("minzoom", options.zoom.to_string());
    writer.set_metadata("maxzoom", options.zoom.to_string());
    let bounds = bounds.unwrap_or([-180.0, -85.051_128_8, 180.0, 85.051_128_8]);
    writer.set_metadata(
        "bounds",
        format!(
            "{:.7},{:.7},{:.7},{:.7}",
            bounds[0], bounds[1], bounds[2], bounds[3]
        ),
    );
    writer.set_metadata(
        "center",
        format!(
            "{:.7},{:.7},{}",
            (bounds[0] + bounds[2]) * 0.5,
            (bounds[1] + bounds[3]) * 0.5,
            options.zoom.min(14)
        ),
    );
    writer.set_metadata("attribution", "OpenStreetMap contributors");
    writer.set_metadata("license", "Open Database License 1.0");
    writer.set_metadata("makepad_source_kind", "osm-all-tags-native-detail-v1");
    writer.set_metadata("makepad_source_file", options.source.display().to_string());
    writer.set_metadata("makepad_all_osm_tags", "true");
    writer.set_metadata("makepad_detail_zoom", options.zoom.to_string());
    writer.set_metadata(
        "makepad_2_5d_tags",
        "building,building:part,height,min_height,building:levels,building:min_level,roof:shape,roof:height,roof:levels,roof:direction,roof:orientation,roof:angle,building:material,building:colour,roof:material,roof:colour",
    );
    writer.set_metadata(
        "json",
        r#"{"vector_layers":[{"id":"osm_points","fields":{}},{"id":"osm_lines","fields":{}},{"id":"osm_polygons","fields":{}},{"id":"osm_relation_points","fields":{}},{"id":"osm_relation_lines","fields":{}},{"id":"osm_relation_polygons","fields":{}}]}"#,
    );

    let sort_memory = options
        .sort_memory_mib
        .checked_mul(1024 * 1024)
        .ok_or_else(|| "--sort-memory-mib overflow".to_string())?;
    let mut progress = Progress::new("spool blocks encoded");
    for &block in &spool.blocks {
        let sorted = SortedBlock::prepare(&spool.dir, block, Some(sort_memory))?;
        let mut sorted = records_to_tiles(sorted, block, |x, y, features| {
            let pbf = encode_tile(features)?;
            let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
            gzip.write_all(&pbf)
                .map_err(|err| format!("gzip tile {}/{x}/{y}: {err}", options.zoom))?;
            let tile = gzip
                .finish()
                .map_err(|err| format!("finish gzip tile {}/{x}/{y}: {err}", options.zoom))?;
            writer
                .write_tile_xyz(options.zoom, x, y, &tile)
                .map_err(|err| format!("write tile {}/{x}/{y}: {err}", options.zoom))
        })?;
        sorted.cleanup_chunks()?;
        progress.tick(1);
    }
    progress.finish();
    writer
        .finish()
        .map_err(|err| format!("finish {}: {err}", options.output.display()))
}

fn collect_tags<'a>(
    tags: impl Iterator<Item = (&'a str, &'a str)>,
) -> SourceTags<'a> {
    tags.map(|(key, value)| (Cow::Borrowed(key), Cow::Borrowed(value)))
        .collect()
}

fn tag_value<'a, T: TagPair>(tags: &'a [T], key: &str) -> Option<&'a str> {
    tags.iter()
        .find(|tag| tag.key() == key)
        .map(TagPair::value)
}

fn inspect_tag_flags<T: TagPair>(tags: &[T]) -> TagFlags {
    let mut flags = TagFlags {
        count: tags.len() as u64,
        ..TagFlags::default()
    };
    for tag in tags {
        match tag.key() {
            "building" => flags.building = true,
            "building:part" => flags.building_part = true,
            "height" => flags.height = true,
            "min_height" => flags.min_height = true,
            "building:levels" => flags.building_levels = true,
            "building:min_level" => flags.building_min_level = true,
            "roof:shape" => flags.roof_shape = true,
            "roof:height" => flags.roof_height = true,
            "roof:levels" => flags.roof_levels = true,
            "roof:direction" => flags.roof_direction = true,
            "roof:orientation" => flags.roof_orientation = true,
            "roof:angle" => flags.roof_angle = true,
            "building:material" => flags.building_material = true,
            "building:colour" | "building:color" => flags.building_colour = true,
            "roof:material" => flags.roof_material = true,
            "roof:colour" | "roof:color" => flags.roof_colour = true,
            _ => {}
        }
    }
    flags
}

struct Progress {
    label: &'static str,
    count: u64,
    last: Instant,
}

impl Progress {
    fn new(label: &'static str) -> Self {
        Self {
            label,
            count: 0,
            last: Instant::now(),
        }
    }

    fn tick(&mut self, count: u64) {
        self.count += count;
        if self.last.elapsed() >= Duration::from_secs(5) {
            println!("  {} {}", self.count, self.label);
            self.last = Instant::now();
        }
    }

    fn finish(&self) {
        println!("  {} {}", self.count, self.label);
    }
}

pub fn default_detail_options(source: PathBuf, output: PathBuf, store: PathBuf) -> DetailOptions {
    DetailOptions {
        source,
        output,
        store,
        zoom: DEFAULT_ZOOM,
        sort_memory_mib: 256,
    }
}

pub fn inspect_mvt_tile(input: &[u8]) -> Result<mvt::TileInspection, String> {
    mvt::inspect_tile(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_source_tags_and_2_5d_fields_are_recognized() {
        let tags = vec![
            ("building".to_string(), "apartments".to_string()),
            ("building:part".to_string(), "yes".to_string()),
            ("height".to_string(), "17.5".to_string()),
            ("min_height".to_string(), "3".to_string()),
            ("building:levels".to_string(), "5".to_string()),
            ("building:min_level".to_string(), "1".to_string()),
            ("roof:shape".to_string(), "gabled".to_string()),
            ("roof:height".to_string(), "2.4".to_string()),
            ("roof:levels".to_string(), "1".to_string()),
            ("roof:direction".to_string(), "85".to_string()),
            ("roof:orientation".to_string(), "along".to_string()),
            ("roof:angle".to_string(), "35".to_string()),
            ("building:material".to_string(), "brick".to_string()),
            ("building:colour".to_string(), "#9f8062".to_string()),
            ("roof:material".to_string(), "tiles".to_string()),
            ("roof:colour".to_string(), "red".to_string()),
            ("name".to_string(), "Example".to_string()),
        ];
        let flags = inspect_tag_flags(&tags);
        assert_eq!(flags.count, tags.len() as u64);
        assert!(flags.building);
        assert!(flags.building_part);
        assert!(flags.height);
        assert!(flags.min_height);
        assert!(flags.building_levels);
        assert!(flags.building_min_level);
        assert!(flags.roof_shape);
        assert!(flags.roof_height);
        assert!(flags.roof_levels);
        assert!(flags.roof_direction);
        assert!(flags.roof_orientation);
        assert!(flags.roof_angle);
        assert!(flags.building_material);
        assert!(flags.building_colour);
        assert!(flags.roof_material);
        assert!(flags.roof_colour);
    }
}
