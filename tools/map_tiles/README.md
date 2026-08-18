# Makepad map tile tools

`makepad-map-tiles` prepares two complementary, disk-streamable map archives:

- a compact Shortbread base pyramid copied from VersaTiles; and
- an all-tag street-detail overlay built directly from a sorted OSM PBF.

Both outputs are standard gzip-MVT MBTiles files. The pure-Rust MBTiles writer
orders rows in 256×256 tile blocks and records a `makepad_rowid_scheme` marker,
so Makepad can seek directly to a requested tile without enumerating a zoom
level. The raw PBF remains the canonical copy of the complete OSM graph.

## Native OSM detail conversion

```sh
cargo build --release --package makepad-map-tiles

target/release/makepad-map-tiles pbf-detail \
    local/maps/europe-260726.osm.pbf \
    local/maps/europe-osm-detail.partial.mbtiles \
    --store local/maps/native-detail-europe.store \
    --zoom 14 \
    --sort-memory-mib 256
```

The input must advertise the standard PBF `Sort.Type_then_ID` optional feature.
The converter performs bounded-memory sequential passes:

1. build a bounded relation-way bitset and persist it for the later passes;
2. project each node once and build a compressed, indexed coordinate store;
3. resolve ways, retain relation-member way references, and clip tagged ways;
4. assemble tagged relation geometry, including multipolygon rings; and
5. externally sort 256×256 tile-block spools, encode MVT, gzip each tile, and
   stream the tiles into MBTiles.

No feature/tag allow-list or geometry simplification is applied. Every tagged
node is emitted as a point. Every tagged way is emitted as a line; a closed way
is also emitted as a polygon candidate so the conversion does not make an
irreversible `area` interpretation. Tagged relation members are emitted as
relation point/line geometry, and `type=multipolygon` relations also receive
assembled polygon geometry.

The six MVT layers are:

- `osm_points`
- `osm_lines`
- `osm_polygons`
- `osm_relation_points`
- `osm_relation_lines`
- `osm_relation_polygons`

Each feature retains every original OSM tag as a string plus its MVT feature ID
and the internal fields `__makepad_osm_id`, `__makepad_osm_type`, and
`__makepad_osm_closed`.

Simple 3D Buildings tags are copied verbatim, including `building`,
`building:part`, `height`, `min_height`, `building:levels`,
`building:min_level`, roof shape/height/levels/direction/orientation/angle,
building and roof materials, and building and roof colours. Keeping the raw
strings avoids silently changing values with units; the renderer can normalize
them once when preparing a 2.5D building mesh and fall back from explicit
height to level counts.

The scratch directory is deliberately retained after a successful build. It
contains compressed node/way indexes, the relation-way bitset, source block
spools, `native-detail-audit.txt`, and an atomic `spool.complete.json` marker.
If final MBTiles writing is interrupted, move the partial MBTiles aside and
repeat the same command: the converter validates the source size and zoom, then
restarts only the external-sort/MBTiles pass. An incomplete earlier pass is
never guessed at or silently reused.

## Performance notes

The hot path is designed around the scale of the pinned Europe extract:
3.79 billion nodes, 464 million ways, and 4.79 billion way-node references.

- node/way group indexes use bounded direct lookup tables;
- compressed coordinate groups are decoded once into an O(1) cache;
- Web Mercator projection happens once per node, not once per way reference;
- ordinary ways resolve directly from the PBF iterator without allocating a
  second node-reference vector;
- source tags borrow the PBF string table and use inline storage for the common
  case, avoiding per-tag string copies and most small heap allocations;
- tile-block writers use hash lookup and retain enough buffered handles to
  cover the Europe block set without per-feature reopen churn;
- one reusable scratch buffer encodes records without allocating a payload
  vector for every tile record;
- relation-member way references are borrowed from decoded groups rather than
  cloned; and
- MVT dictionaries use deterministic insertion order with fast lookup.

On the same machine used for development, the release converter processed the
47,228,875-byte Luxembourg extract (4,208,128 nodes, 6,313,908 way-node
references, and 1,617,855 tile records) into 3,953 checked tiles in 12.3
seconds. Pass 3 sustained about 1.29 million resolved references per second.
Scaling the measured stages by the audited Europe object/reference counts gives
a practical preflight estimate of roughly 2–3 hours, rather than four-plus,
though final time depends on SSD throughput and tile density.

## Other commands

```sh
# Inspect source ordering and bounds.
target/release/makepad-map-tiles inspect-pbf source.osm.pbf

# Count all source tags plus benches, playgrounds, buildings and 2.5D fields.
target/release/makepad-map-tiles audit-pbf source.osm.pbf

# Verify direct MBTiles lookup, gzip/MVT validity, layers, and detail tag keys.
target/release/makepad-map-tiles probe-mbtiles map.mbtiles 14/8414/5384

# Extract the Europe rectangle from a VersaTiles Shortbread planet archive.
target/release/makepad-map-tiles versatiles \
    source.versatiles output.mbtiles \
    --bbox=-32.683233,29.635548,46.753480,81.472990 \
    --max-zoom 14
```

`tools/download_map.sh` pins and verifies the large upstream snapshots, builds the
release utility, validates SQLite, probes a real MVT tile, and only then
atomically renames a `.partial.mbtiles` output.
