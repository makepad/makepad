/// Detail-layer keys consumed by the renderer's way/polygon paths.
pub const DETAIL_WAY_KEYS: &[&str] = &[
    "layer",
    "bridge",
    "tunnel",
    "highway",
    "railway",
    "width",
    "barrier",
    "area",
    "name",
    "attraction",
    "zoo",
    "tourism",
    "public_transport",
    "landuse",
    "leisure",
    "natural",
    "building",
    "building:part",
    "height",
    "building:levels",
    "min_height",
    "building:min_level",
    "location",
    "place",
    "parking",
    "surface",
    "access",
    "service",
    "link",
    "rail",
    "waterway",
    "ref",
];

/// Additional keys consumed by point icon/routing code.
pub const DETAIL_POINT_EXTRA_KEYS: &[&str] = &[
    "amenity",
    "brand",
    "craft",
    "entrance",
    "historic",
    "max_kw",
    "office",
    "operator",
    "shop",
    "osm_layer",
    "kerb",
    "bus",
    "shelter",
];

/// Metadata key/value an archive carries once its detail layers follow
/// [`detail_feature_used`]; bump the value whenever that contract drops more.
pub const DETAIL_CONTRACT_METADATA_KEY: &str = "makepad_detail_contract";
pub const DETAIL_CONTRACT: &str = "features-v1";

/// True when a detail-layer tag key survives the archive contract.
pub fn detail_key_allowed(key: &str) -> bool {
    DETAIL_WAY_KEYS.contains(&key) || DETAIL_POINT_EXTRA_KEYS.contains(&key)
}

/// Tag access for the detail-feature contract. The source-layer name and
/// OSM's own `layer=*` stacking tag share the key "layer" in raw tiles; the
/// renderer moves the OSM value to "osm_layer" on parse, so each side says
/// where it keeps it.
pub trait DetailTags {
    fn tag(&self, key: &str) -> Option<&str>;
    fn osm_layer(&self) -> Option<&str>;
    fn has(&self, key: &str) -> bool {
        self.tag(key).is_some()
    }
}

impl<K: AsRef<str>, V: AsRef<str>> DetailTags for [(K, V)] {
    fn tag(&self, key: &str) -> Option<&str> {
        self.iter()
            .find(|(k, _)| k.as_ref() == key)
            .map(|(_, v)| v.as_ref())
    }
    fn osm_layer(&self) -> Option<&str> {
        self.tag("layer")
    }
}

/// Label colour family of a micro icon; the renderer maps it to its own
/// label classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicroIconClass {
    Tree,
    Muted,
    Transport,
    Green,
    Culture,
}

/// The micro-POI icon a detail point (or polygon centroid) draws, if any.
pub fn micro_icon(tags: &(impl DetailTags + ?Sized)) -> Option<(&'static str, MicroIconClass)> {
    use MicroIconClass::*;
    if tags.tag("natural") == Some("tree") {
        return Some(("tree", Tree));
    }
    if let Some(amenity) = tags.tag("amenity") {
        return match amenity {
            "bench" => Some(("bench", Muted)),
            "waste_basket" | "waste_disposal" => Some(("waste_basket", Muted)),
            "recycling" => Some(("recycling", Muted)),
            "bicycle_parking" => Some(("bicycle", Transport)),
            "parking" | "parking_entrance" => Some(("parking", Transport)),
            "charging_station" => Some(("charger", Transport)),
            _ => None,
        };
    }
    if tags.tag("highway") == Some("traffic_signals") {
        return Some(("traffic_signals", Muted));
    }
    // Offices only exist in the detail layers; carto shows them as a
    // small dot + name from street-level zoom.
    if tags.has("office") && tags.has("name") {
        return Some(("dot", Muted));
    }
    if let Some(leisure) = tags.tag("leisure") {
        return match leisure {
            "playground" => Some(("playground", Green)),
            "picnic_table" => Some(("bench", Green)),
            _ => None,
        };
    }
    if let Some(tourism) = tags.tag("tourism") {
        return match tourism {
            "artwork" => Some(("statue", Culture)),
            "information" => Some(("information", Culture)),
            _ => None,
        };
    }
    if tags.tag("railway") == Some("subway_entrance") {
        return Some(("entrance", Transport));
    }
    if let Some(entrance) = tags.tag("entrance") {
        return match entrance {
            "no" => None,
            _ => Some(("entrance", Muted)),
        };
    }
    if let Some(historic) = tags.tag("historic") {
        return match historic {
            "memorial" | "monument" | "statue" => Some(("statue", Culture)),
            _ => None,
        };
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailGeom {
    Point,
    Line,
    Polygon,
}

fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|value| !matches!(value, "" | "0" | "false" | "False" | "no"))
}

fn named_attraction(tags: &(impl DetailTags + ?Sized)) -> bool {
    tags.has("name")
        && (tags.has("attraction") || tags.has("zoo") || tags.tag("tourism") == Some("attraction"))
}

/// Whether the renderer's detail pass can ever draw a feature of an `osm_*`
/// layer. This is the archive data contract: the base bake and the
/// repacker drop exactly the features that fail it, and the renderer
/// skips them at parse time, so an archive with and without them renders
/// the same. It mirrors `merge_detail_features_from_collector` in
/// widgets/families/maps/src/map/tile.rs; change both together.
/// Tags must already be cut to [`detail_key_allowed`].
pub fn detail_feature_used(
    layer: &str,
    geom: DetailGeom,
    tags: &(impl DetailTags + ?Sized),
) -> bool {
    match layer {
        // Micro-POI icons and named attraction nodes.
        "osm_points" => {
            micro_icon(tags).is_some()
                || (tags.has("name") && (tags.has("attraction") || tags.has("zoo")))
        }
        // No renderer path reads relation points.
        "osm_relation_points" => false,
        "osm_lines" | "osm_relation_lines" => {
            // Heuristic bridge corridors (open osm_lines only).
            if layer == "osm_lines"
                && geom != DetailGeom::Polygon
                && matches!(tags.tag("bridge"), Some("yes" | "viaduct"))
                && (tags.has("highway") || tags.has("railway"))
            {
                return true;
            }
            if let Some(barrier) = tags.tag("barrier") {
                return matches!(
                    barrier,
                    "wall" | "fence" | "retaining_wall" | "city_wall" | "hedge"
                );
            }
            (truthy(tags.tag("area"))
                && matches!(tags.tag("highway"), Some("pedestrian" | "footway")))
                || named_attraction(tags)
        }
        "osm_polygons" | "osm_relation_polygons" => {
            if micro_icon(tags).is_some() {
                return true;
            }
            if tags.tag("railway") == Some("platform")
                || tags.tag("public_transport") == Some("platform")
            {
                return true;
            }
            let green_patch = matches!(
                tags.tag("landuse"),
                Some("grass" | "village_green" | "flowerbed" | "meadow")
            ) || tags.tag("leisure") == Some("garden")
                || matches!(
                    tags.tag("natural"),
                    Some("scrub" | "heath" | "shrubbery" | "sand" | "beach" | "shingle")
                );
            if green_patch
                || matches!(tags.tag("tourism"), Some("zoo" | "theme_park"))
                || named_attraction(tags)
                || matches!(tags.tag("highway"), Some("pedestrian" | "footway"))
                || tags.tag("place") == Some("square")
            {
                return true;
            }
            let building = tags.tag("building").is_some_and(|value| value != "no")
                || tags.tag("building:part").is_some_and(|value| value != "no");
            building
                && tags.tag("location") != Some("underground")
                && !tags.osm_layer().is_some_and(|value| value.starts_with('-'))
        }
        _ => true,
    }
}
