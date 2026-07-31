//! The `region.search` place/POI/street index: fixed-record doc table, a
//! sorted token table (binary search gives exact and prefix ranges), and
//! postings lists pre-sorted by static rank. Query = tokenize, intersect
//! postings (last token as prefix), score by match quality + static rank +
//! proximity. Category synonyms ("supermarkt", "pizza") expand to category
//! result sets so nearby instances win over name matches.

use crate::fmt::{ByteReader, ByteWriter, NavFmtError};
use crate::geo::{fixed_to_lon_lat, haversine_m, lon_lat_to_fixed, LonLat};
use std::collections::HashMap;

const SEARCH_MAGIC: u32 = 0x4d50_5346; // "FSPM"
const SEARCH_VERSION: u32 = 1;

// --- Categories ---

/// Stable u16 category codes stored in doc records. Add at the end only.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(u16)]
pub enum Category {
    Other = 0,
    City = 1,
    Town = 2,
    Village = 3,
    Suburb = 4,
    Neighbourhood = 5,
    Hamlet = 6,
    Street = 20,
    Address = 21,
    Station = 40,
    TramStop = 41,
    BusStop = 42,
    FerryTerminal = 43,
    Airport = 44,
    Supermarket = 60,
    Convenience = 61,
    Bakery = 62,
    Butcher = 63,
    Clothes = 64,
    Electronics = 65,
    DoItYourself = 66,
    Florist = 67,
    Books = 68,
    Hairdresser = 69,
    Shop = 70,
    Restaurant = 90,
    FastFood = 91,
    Cafe = 92,
    Bar = 93,
    Pub = 94,
    IceCream = 95,
    Pharmacy = 110,
    Hospital = 111,
    Doctor = 112,
    Dentist = 113,
    Veterinary = 114,
    School = 130,
    University = 131,
    Kindergarten = 132,
    Library = 133,
    Museum = 150,
    Attraction = 151,
    Gallery = 152,
    Zoo = 153,
    ThemePark = 154,
    Viewpoint = 155,
    Monument = 156,
    Castle = 157,
    Hotel = 170,
    Hostel = 171,
    GuestHouse = 172,
    CampSite = 173,
    Bank = 190,
    Atm = 191,
    PostOffice = 192,
    Police = 193,
    FireStation = 194,
    Townhall = 195,
    PlaceOfWorship = 196,
    CommunityCentre = 197,
    Cinema = 210,
    Theatre = 211,
    Nightclub = 212,
    SportsCentre = 213,
    SwimmingPool = 214,
    Playground = 215,
    Park = 216,
    Garden = 217,
    NatureReserve = 218,
    Beach = 219,
    Marina = 220,
    GolfCourse = 221,
    Stadium = 222,
    Fuel = 240,
    Parking = 241,
    ChargingStation = 242,
    BicycleRental = 243,
    CarRental = 244,
    CarWash = 245,
    Toilets = 260,
    DrinkingWater = 261,
}

impl Category {
    pub fn from_u16(v: u16) -> Category {
        ALL_CATEGORIES
            .iter()
            .copied()
            .find(|c| *c as u16 == v)
            .unwrap_or(Category::Other)
    }

    /// Short human label for result lists.
    pub fn label(&self) -> &'static str {
        match self {
            Category::Other => "Place",
            Category::City => "City",
            Category::Town => "Town",
            Category::Village => "Village",
            Category::Suburb => "Suburb",
            Category::Neighbourhood => "Neighbourhood",
            Category::Hamlet => "Hamlet",
            Category::Street => "Street",
            Category::Address => "Address",
            Category::Station => "Station",
            Category::TramStop => "Tram stop",
            Category::BusStop => "Bus stop",
            Category::FerryTerminal => "Ferry",
            Category::Airport => "Airport",
            Category::Supermarket => "Supermarket",
            Category::Convenience => "Convenience store",
            Category::Bakery => "Bakery",
            Category::Butcher => "Butcher",
            Category::Clothes => "Clothing store",
            Category::Electronics => "Electronics",
            Category::DoItYourself => "DIY store",
            Category::Florist => "Florist",
            Category::Books => "Bookstore",
            Category::Hairdresser => "Hairdresser",
            Category::Shop => "Shop",
            Category::Restaurant => "Restaurant",
            Category::FastFood => "Fast food",
            Category::Cafe => "Cafe",
            Category::Bar => "Bar",
            Category::Pub => "Pub",
            Category::IceCream => "Ice cream",
            Category::Pharmacy => "Pharmacy",
            Category::Hospital => "Hospital",
            Category::Doctor => "Doctor",
            Category::Dentist => "Dentist",
            Category::Veterinary => "Veterinary",
            Category::School => "School",
            Category::University => "University",
            Category::Kindergarten => "Kindergarten",
            Category::Library => "Library",
            Category::Museum => "Museum",
            Category::Attraction => "Attraction",
            Category::Gallery => "Gallery",
            Category::Zoo => "Zoo",
            Category::ThemePark => "Theme park",
            Category::Viewpoint => "Viewpoint",
            Category::Monument => "Monument",
            Category::Castle => "Castle",
            Category::Hotel => "Hotel",
            Category::Hostel => "Hostel",
            Category::GuestHouse => "Guest house",
            Category::CampSite => "Camp site",
            Category::Bank => "Bank",
            Category::Atm => "ATM",
            Category::PostOffice => "Post office",
            Category::Police => "Police",
            Category::FireStation => "Fire station",
            Category::Townhall => "Town hall",
            Category::PlaceOfWorship => "Place of worship",
            Category::CommunityCentre => "Community centre",
            Category::Cinema => "Cinema",
            Category::Theatre => "Theatre",
            Category::Nightclub => "Nightclub",
            Category::SportsCentre => "Sports centre",
            Category::SwimmingPool => "Swimming pool",
            Category::Playground => "Playground",
            Category::Park => "Park",
            Category::Garden => "Garden",
            Category::NatureReserve => "Nature reserve",
            Category::Beach => "Beach",
            Category::Marina => "Marina",
            Category::GolfCourse => "Golf course",
            Category::Stadium => "Stadium",
            Category::Fuel => "Fuel station",
            Category::Parking => "Parking",
            Category::ChargingStation => "Charging station",
            Category::BicycleRental => "Bicycle rental",
            Category::CarRental => "Car rental",
            Category::CarWash => "Car wash",
            Category::Toilets => "Toilets",
            Category::DrinkingWater => "Drinking water",
        }
    }

    /// Static category weight for ranking (higher = more prominent).
    pub fn base_rank(&self) -> u8 {
        match self {
            Category::City => 255,
            Category::Airport => 230,
            Category::Town => 210,
            Category::Station => 190,
            Category::Village => 160,
            Category::Suburb => 130,
            Category::Hamlet => 110,
            Category::Neighbourhood => 105,
            Category::Hospital => 100,
            Category::Museum | Category::Attraction | Category::Zoo | Category::ThemePark => 95,
            Category::Street => 90,
            Category::University | Category::Stadium => 88,
            Category::Supermarket => 85,
            Category::Park | Category::NatureReserve | Category::Beach => 80,
            Category::Pharmacy => 78,
            Category::Fuel | Category::ChargingStation => 72,
            Category::TramStop | Category::FerryTerminal => 70,
            Category::Hotel | Category::Theatre | Category::Cinema => 65,
            Category::Restaurant | Category::Cafe => 60,
            Category::FastFood | Category::Bar | Category::Pub => 55,
            Category::BusStop => 45,
            Category::Address => 20,
            _ => 50,
        }
    }
}

const ALL_CATEGORIES: &[Category] = &[
    Category::Other,
    Category::City,
    Category::Town,
    Category::Village,
    Category::Suburb,
    Category::Neighbourhood,
    Category::Hamlet,
    Category::Street,
    Category::Address,
    Category::Station,
    Category::TramStop,
    Category::BusStop,
    Category::FerryTerminal,
    Category::Airport,
    Category::Supermarket,
    Category::Convenience,
    Category::Bakery,
    Category::Butcher,
    Category::Clothes,
    Category::Electronics,
    Category::DoItYourself,
    Category::Florist,
    Category::Books,
    Category::Hairdresser,
    Category::Shop,
    Category::Restaurant,
    Category::FastFood,
    Category::Cafe,
    Category::Bar,
    Category::Pub,
    Category::IceCream,
    Category::Pharmacy,
    Category::Hospital,
    Category::Doctor,
    Category::Dentist,
    Category::Veterinary,
    Category::School,
    Category::University,
    Category::Kindergarten,
    Category::Library,
    Category::Museum,
    Category::Attraction,
    Category::Gallery,
    Category::Zoo,
    Category::ThemePark,
    Category::Viewpoint,
    Category::Monument,
    Category::Castle,
    Category::Hotel,
    Category::Hostel,
    Category::GuestHouse,
    Category::CampSite,
    Category::Bank,
    Category::Atm,
    Category::PostOffice,
    Category::Police,
    Category::FireStation,
    Category::Townhall,
    Category::PlaceOfWorship,
    Category::CommunityCentre,
    Category::Cinema,
    Category::Theatre,
    Category::Nightclub,
    Category::SportsCentre,
    Category::SwimmingPool,
    Category::Playground,
    Category::Park,
    Category::Garden,
    Category::NatureReserve,
    Category::Beach,
    Category::Marina,
    Category::GolfCourse,
    Category::Stadium,
    Category::Fuel,
    Category::Parking,
    Category::ChargingStation,
    Category::BicycleRental,
    Category::CarRental,
    Category::CarWash,
    Category::Toilets,
    Category::DrinkingWater,
];

/// Map OSM tags to a category. Returns None for untagged/uninteresting
/// features (the caller decides whether a bare name is still worth a doc).
pub fn category_from_osm_tags(tags: &HashMap<String, String>) -> Option<Category> {
    let get = |k: &str| tags.get(k).map(|s| s.as_str());
    if let Some(place) = get("place") {
        return Some(match place {
            "city" => Category::City,
            "town" => Category::Town,
            "village" => Category::Village,
            "suburb" | "borough" | "quarter" => Category::Suburb,
            "neighbourhood" => Category::Neighbourhood,
            "hamlet" => Category::Hamlet,
            _ => return None,
        });
    }
    if let Some(amenity) = get("amenity") {
        return Some(match amenity {
            "restaurant" => Category::Restaurant,
            "fast_food" => Category::FastFood,
            "cafe" => Category::Cafe,
            "bar" => Category::Bar,
            "pub" => Category::Pub,
            "ice_cream" => Category::IceCream,
            "pharmacy" => Category::Pharmacy,
            "hospital" | "clinic" => Category::Hospital,
            "doctors" => Category::Doctor,
            "dentist" => Category::Dentist,
            "veterinary" => Category::Veterinary,
            "school" => Category::School,
            "university" | "college" => Category::University,
            "kindergarten" => Category::Kindergarten,
            "library" => Category::Library,
            "bank" => Category::Bank,
            "atm" => Category::Atm,
            "post_office" => Category::PostOffice,
            "police" => Category::Police,
            "fire_station" => Category::FireStation,
            "townhall" => Category::Townhall,
            "place_of_worship" => Category::PlaceOfWorship,
            "community_centre" => Category::CommunityCentre,
            "cinema" => Category::Cinema,
            "theatre" => Category::Theatre,
            "nightclub" => Category::Nightclub,
            "fuel" => Category::Fuel,
            "parking" => Category::Parking,
            "charging_station" => Category::ChargingStation,
            "bicycle_rental" => Category::BicycleRental,
            "car_rental" => Category::CarRental,
            "car_wash" => Category::CarWash,
            "toilets" => Category::Toilets,
            "drinking_water" => Category::DrinkingWater,
            "ferry_terminal" => Category::FerryTerminal,
            "marina" => Category::Marina,
            _ => return None,
        });
    }
    if let Some(shop) = get("shop") {
        return Some(match shop {
            "supermarket" => Category::Supermarket,
            "convenience" => Category::Convenience,
            "bakery" => Category::Bakery,
            "butcher" => Category::Butcher,
            "clothes" | "fashion" => Category::Clothes,
            "electronics" | "computer" | "mobile_phone" => Category::Electronics,
            "doityourself" | "hardware" => Category::DoItYourself,
            "florist" => Category::Florist,
            "books" => Category::Books,
            "hairdresser" => Category::Hairdresser,
            _ => Category::Shop,
        });
    }
    if let Some(tourism) = get("tourism") {
        return Some(match tourism {
            "museum" => Category::Museum,
            "attraction" => Category::Attraction,
            "gallery" => Category::Gallery,
            "zoo" => Category::Zoo,
            "theme_park" => Category::ThemePark,
            "viewpoint" => Category::Viewpoint,
            "hotel" => Category::Hotel,
            "hostel" => Category::Hostel,
            "guest_house" => Category::GuestHouse,
            "camp_site" => Category::CampSite,
            _ => return None,
        });
    }
    if let Some(leisure) = get("leisure") {
        return Some(match leisure {
            "park" => Category::Park,
            "garden" => Category::Garden,
            "nature_reserve" => Category::NatureReserve,
            "beach_resort" => Category::Beach,
            "marina" => Category::Marina,
            "golf_course" => Category::GolfCourse,
            "stadium" => Category::Stadium,
            "sports_centre" | "fitness_centre" => Category::SportsCentre,
            "swimming_pool" => Category::SwimmingPool,
            "playground" => Category::Playground,
            _ => return None,
        });
    }
    if let Some(historic) = get("historic") {
        return Some(match historic {
            "monument" | "memorial" => Category::Monument,
            "castle" => Category::Castle,
            _ => return None,
        });
    }
    if let Some(railway) = get("railway") {
        return Some(match railway {
            "station" | "halt" => Category::Station,
            "tram_stop" => Category::TramStop,
            _ => return None,
        });
    }
    if get("highway") == Some("bus_stop") {
        return Some(Category::BusStop);
    }
    if get("aeroway") == Some("aerodrome") {
        return Some(Category::Airport);
    }
    if get("natural") == Some("beach") {
        return Some(Category::Beach);
    }
    // Zoo enclosures / attraction buildings tag attraction=* with no
    // tourism key (Artis's Reptielenhuis).
    if tags.contains_key("attraction") || tags.contains_key("zoo") {
        return Some(Category::Attraction);
    }
    if tags.contains_key("addr:housenumber") {
        return Some(Category::Address);
    }
    None
}

/// Query-word to category expansion (NL + EN). A token matching an entry
/// returns nearby instances of the categories rather than name matches.
const CATEGORY_SYNONYMS: &[(&str, &[Category])] = &[
    ("supermarkt", &[Category::Supermarket, Category::Convenience]),
    ("supermarket", &[Category::Supermarket, Category::Convenience]),
    ("groceries", &[Category::Supermarket, Category::Convenience]),
    ("boodschappen", &[Category::Supermarket, Category::Convenience]),
    ("restaurant", &[Category::Restaurant, Category::FastFood]),
    ("eten", &[Category::Restaurant, Category::FastFood, Category::Cafe]),
    ("food", &[Category::Restaurant, Category::FastFood]),
    ("pizza", &[Category::Restaurant, Category::FastFood]),
    ("snackbar", &[Category::FastFood]),
    ("cafe", &[Category::Cafe, Category::Bar, Category::Pub]),
    ("koffie", &[Category::Cafe]),
    ("coffee", &[Category::Cafe]),
    ("bar", &[Category::Bar, Category::Pub]),
    ("kroeg", &[Category::Bar, Category::Pub]),
    ("apotheek", &[Category::Pharmacy]),
    ("pharmacy", &[Category::Pharmacy]),
    ("ziekenhuis", &[Category::Hospital]),
    ("hospital", &[Category::Hospital]),
    ("dokter", &[Category::Doctor]),
    ("huisarts", &[Category::Doctor]),
    ("tandarts", &[Category::Dentist]),
    ("tankstation", &[Category::Fuel]),
    ("fuel", &[Category::Fuel]),
    ("gas", &[Category::Fuel]),
    ("benzine", &[Category::Fuel]),
    ("laadpaal", &[Category::ChargingStation]),
    ("charger", &[Category::ChargingStation]),
    ("charging", &[Category::ChargingStation]),
    ("parkeren", &[Category::Parking]),
    ("parking", &[Category::Parking]),
    ("hotel", &[Category::Hotel, Category::Hostel, Category::GuestHouse]),
    ("station", &[Category::Station]),
    ("trein", &[Category::Station]),
    ("train", &[Category::Station]),
    ("tram", &[Category::TramStop]),
    ("bushalte", &[Category::BusStop]),
    ("bus", &[Category::BusStop]),
    ("pont", &[Category::FerryTerminal]),
    ("ferry", &[Category::FerryTerminal]),
    ("veerpont", &[Category::FerryTerminal]),
    ("geldautomaat", &[Category::Atm]),
    ("atm", &[Category::Atm]),
    ("pinnen", &[Category::Atm]),
    ("bank", &[Category::Bank]),
    ("bakker", &[Category::Bakery]),
    ("bakery", &[Category::Bakery]),
    ("brood", &[Category::Bakery]),
    ("park", &[Category::Park, Category::Garden, Category::NatureReserve]),
    ("museum", &[Category::Museum, Category::Gallery]),
    ("school", &[Category::School]),
    ("bibliotheek", &[Category::Library]),
    ("library", &[Category::Library]),
    ("bioscoop", &[Category::Cinema]),
    ("cinema", &[Category::Cinema]),
    ("theater", &[Category::Theatre]),
    ("zwembad", &[Category::SwimmingPool]),
    ("toilet", &[Category::Toilets]),
    ("wc", &[Category::Toilets]),
    ("water", &[Category::DrinkingWater]),
    ("fietsverhuur", &[Category::BicycleRental]),
    ("ijs", &[Category::IceCream]),
    ("icecream", &[Category::IceCream]),
    ("kerk", &[Category::PlaceOfWorship]),
    ("church", &[Category::PlaceOfWorship]),
    ("strand", &[Category::Beach]),
    ("beach", &[Category::Beach]),
    ("haven", &[Category::Marina]),
    ("politie", &[Category::Police]),
    ("police", &[Category::Police]),
    ("kapper", &[Category::Hairdresser]),
];

// --- Normalization / tokenization ---

/// Lowercase, fold common Latin diacritics, keep alphanumerics, split the
/// rest. Mirrors the renderer's label normalization so index and query agree.
pub fn normalize_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        let folded = fold_char(ch);
        match folded {
            Some(c) => current.push(c),
            None => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn fold_char(ch: char) -> Option<char> {
    if ch.is_ascii_alphanumeric() {
        return Some(ch.to_ascii_lowercase());
    }
    // Common European diacritics; enough for NL/DE/FR names.
    let folded = match ch {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'Á' | 'À' | 'Â' | 'Ä' | 'Ã' | 'Å' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' | 'Ó' | 'Ò' | 'Ô' | 'Ö' | 'Õ' | 'Ø' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'u',
        'ý' | 'ÿ' | 'Ý' => 'y',
        'ç' | 'Ç' => 'c',
        'ñ' | 'Ñ' => 'n',
        'ß' => 's',
        _ if ch.is_alphanumeric() => ch.to_lowercase().next().unwrap_or(ch),
        _ => return None,
    };
    Some(folded)
}

// --- Index ---

#[derive(Clone, Debug)]
struct DocRecord {
    x: u32,
    y: u32,
    category: u16,
    rank: u8,
    name_start: u32,
    name_len: u16,
    secondary_start: u32,
    secondary_len: u16,
}

pub struct SearchIndex {
    docs: Vec<DocRecord>,
    strings: String,
    // token strings sorted; parallel postings ranges into `postings`
    token_strings: Vec<String>,
    token_postings: Vec<(u32, u32)>, // start, len
    postings: Vec<u32>,
    // doc ids per category, sorted by rank desc
    category_docs: HashMap<u16, Vec<u32>>,
}

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub doc_id: u32,
    pub name: String,
    pub secondary: String,
    pub category: Category,
    pub pos: LonLat,
    pub distance_m: Option<f64>,
    pub score: f64,
}

pub struct SearchIndexBuilder {
    docs: Vec<(String, String, LonLat, Category, u8)>,
}

impl Default for SearchIndexBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchIndexBuilder {
    pub fn new() -> Self {
        Self { docs: Vec::new() }
    }

    pub fn add(
        &mut self,
        name: &str,
        secondary: &str,
        pos: LonLat,
        category: Category,
        rank: u8,
    ) {
        if name.trim().is_empty() {
            return;
        }
        self.docs
            .push((name.trim().to_string(), secondary.trim().to_string(), pos, category, rank));
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    pub fn build(mut self) -> SearchIndex {
        // Dedupe by (normalized name, category, ~350m quantized location):
        // the same feature can arrive from multiple sources/tiles.
        let mut seen = HashMap::<(String, u16, i64, i64), usize>::new();
        let mut kept = Vec::with_capacity(self.docs.len());
        for doc in self.docs.drain(..) {
            let key_name = normalize_tokens(&doc.0).join(" ");
            let quant = 350.0 / 111_320.0; // ~350m in degrees lat
            let qx = (doc.2.lon / quant) as i64;
            let qy = (doc.2.lat / quant) as i64;
            let key = (key_name, doc.3 as u16, qx, qy);
            match seen.get(&key) {
                Some(&idx) => {
                    // Keep the higher-ranked duplicate (e.g. one has secondary)
                    let existing: &(String, String, LonLat, Category, u8) = &kept[idx];
                    if doc.4 > existing.4
                        || (doc.1.len() > existing.1.len() && doc.4 >= existing.4)
                    {
                        kept[idx] = doc;
                    }
                }
                None => {
                    seen.insert(key, kept.len());
                    kept.push(doc);
                }
            }
        }

        let mut strings = String::new();
        let mut docs = Vec::with_capacity(kept.len());
        let mut token_map = HashMap::<String, Vec<u32>>::new();
        for (name, secondary, pos, category, rank) in &kept {
            let doc_id = docs.len() as u32;
            let name_start = strings.len() as u32;
            strings.push_str(name);
            let name_len = (strings.len() as u32 - name_start) as u16;
            let secondary_start = strings.len() as u32;
            strings.push_str(secondary);
            let secondary_len = (strings.len() as u32 - secondary_start) as u16;
            let (x, y) = lon_lat_to_fixed(*pos);
            docs.push(DocRecord {
                x,
                y,
                category: *category as u16,
                rank: *rank,
                name_start,
                name_len,
                secondary_start,
                secondary_len,
            });
            for token in normalize_tokens(name) {
                token_map.entry(token).or_default().push(doc_id);
            }
        }

        let mut token_strings: Vec<String> = token_map.keys().cloned().collect();
        token_strings.sort_unstable();
        let mut token_postings = Vec::with_capacity(token_strings.len());
        let mut postings = Vec::new();
        for token in &token_strings {
            let mut ids = token_map.remove(token).unwrap();
            ids.sort_unstable();
            ids.dedup();
            // Pre-sort by static rank desc so top-k reads can short-circuit.
            ids.sort_by(|a, b| docs[*b as usize].rank.cmp(&docs[*a as usize].rank));
            let start = postings.len() as u32;
            postings.extend_from_slice(&ids);
            token_postings.push((start, ids.len() as u32));
        }

        let mut category_docs = HashMap::<u16, Vec<u32>>::new();
        for (i, doc) in docs.iter().enumerate() {
            category_docs.entry(doc.category).or_default().push(i as u32);
        }
        for ids in category_docs.values_mut() {
            ids.sort_by(|a, b| docs[*b as usize].rank.cmp(&docs[*a as usize].rank));
        }

        SearchIndex {
            docs,
            strings,
            token_strings,
            token_postings,
            postings,
            category_docs,
        }
    }
}

impl SearchIndex {
    pub fn doc_count(&self) -> usize {
        self.docs.len()
    }

    fn doc_name(&self, doc: &DocRecord) -> &str {
        &self.strings[doc.name_start as usize..(doc.name_start + doc.name_len as u32) as usize]
    }

    fn doc_secondary(&self, doc: &DocRecord) -> &str {
        &self.strings[doc.secondary_start as usize
            ..(doc.secondary_start + doc.secondary_len as u32) as usize]
    }

    fn postings_for(&self, token_idx: usize) -> &[u32] {
        let (start, len) = self.token_postings[token_idx];
        &self.postings[start as usize..(start + len) as usize]
    }

    /// Doc ids whose name contains `token` exactly (`prefix`=false) or a
    /// token starting with it (`prefix`=true). Capped to keep worst-case
    /// single-letter prefixes bounded.
    fn candidate_docs(&self, token: &str, prefix: bool, cap: usize) -> Vec<u32> {
        let start = self.token_strings.partition_point(|t| t.as_str() < token);
        let mut out = Vec::new();
        let mut idx = start;
        while idx < self.token_strings.len() {
            let t = &self.token_strings[idx];
            let matches = if prefix {
                t.starts_with(token)
            } else {
                t == token
            };
            if !matches {
                break;
            }
            out.extend_from_slice(self.postings_for(idx));
            if out.len() >= cap {
                break;
            }
            idx += 1;
            if !prefix {
                break;
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Token-table indices within a small edit distance of `token` —
    /// speech-to-text anglicizes names ("Harlem" for Haarlem), so near-miss
    /// tokens become retrieval candidates and the tiered scorer arbitrates.
    /// First letter must match; the sorted table bounds the scan.
    fn fuzzy_token_indices(&self, token: &str) -> Vec<usize> {
        let max_ed = if token.len() >= 9 { 2 } else { 1 };
        let Some(first) = token.chars().next() else {
            return Vec::new();
        };
        let head = &token[..first.len_utf8()];
        let start = self.token_strings.partition_point(|t| t.as_str() < head);
        let mut out = Vec::new();
        for idx in start..self.token_strings.len() {
            let cand = &self.token_strings[idx];
            if !cand.starts_with(head) {
                break;
            }
            if cand.len().abs_diff(token.len()) <= max_ed
                && cand.as_str() != token
                && edit_distance_at_most(token.as_bytes(), cand.as_bytes(), max_ed)
            {
                out.push(idx);
                if out.len() >= 24 {
                    break;
                }
            }
        }
        out
    }

    /// Ranked search. `near` enables the proximity boost ("pizza" means
    /// "pizza near me"). The last token is treated as a prefix
    /// (autocomplete-style).
    pub fn query(&self, text: &str, near: Option<LonLat>, limit: usize) -> Vec<SearchResult> {
        let tokens = normalize_tokens(text);
        if tokens.is_empty() {
            return Vec::new();
        }

        const CANDIDATE_CAP: usize = 20_000;
        let last = tokens.len() - 1;
        let mut text_candidates: Option<Vec<u32>> = None;
        for (i, token) in tokens.iter().enumerate() {
            let ids = self.candidate_docs(token, i == last, CANDIDATE_CAP);
            text_candidates = Some(match text_candidates {
                None => ids,
                Some(prev) => intersect_sorted(&prev, &ids),
            });
            if text_candidates.as_ref().unwrap().is_empty() {
                break;
            }
        }
        let text_candidates = text_candidates.unwrap_or_default();

        // Transcription correction: if the literal match surfaced no
        // settlement at all (e.g. "harlem" only hits a parking spot), redo
        // the intersection with near-miss tokens unioned in per query
        // token. The tiered scorer then prefers the town of Haarlem over
        // an exactly-named minor POI on its own.
        let is_settlement = |doc_id: u32| {
            matches!(
                Category::from_u16(self.docs[doc_id as usize].category),
                Category::City
                    | Category::Town
                    | Category::Village
                    | Category::Suburb
                    | Category::Hamlet
                    | Category::Neighbourhood
            )
        };
        let mut fuzzy_candidates: Vec<u32> = Vec::new();
        if !text_candidates.iter().any(|&id| is_settlement(id)) {
            let mut acc: Option<Vec<u32>> = None;
            for (i, token) in tokens.iter().enumerate() {
                let mut ids = self.candidate_docs(token, i == last, CANDIDATE_CAP);
                if token.len() >= 4 {
                    for idx in self.fuzzy_token_indices(token) {
                        ids.extend_from_slice(self.postings_for(idx));
                    }
                    ids.sort_unstable();
                    ids.dedup();
                }
                acc = Some(match acc {
                    None => ids,
                    Some(prev) => intersect_sorted(&prev, &ids),
                });
                if acc.as_ref().unwrap().is_empty() {
                    break;
                }
            }
            fuzzy_candidates = acc.unwrap_or_default();
            fuzzy_candidates.retain(|&id| text_candidates.binary_search(&id).is_err());
        }

        // Category expansion: tokens hitting the synonym table pull in the
        // top-ranked docs of those categories (proximity re-ranks them).
        let mut category_candidates: Vec<u32> = Vec::new();
        if tokens.len() <= 2 {
            for token in &tokens {
                for (word, cats) in CATEGORY_SYNONYMS {
                    if word == token {
                        for cat in cats.iter() {
                            if let Some(ids) = self.category_docs.get(&(*cat as u16)) {
                                category_candidates
                                    .extend_from_slice(&ids[..ids.len().min(4000)]);
                            }
                        }
                    }
                }
            }
            category_candidates.sort_unstable();
            category_candidates.dedup();
        }

        let normalized_query = tokens.join(" ");
        let query_has_number = tokens.iter().any(|t| t.chars().all(|c| c.is_ascii_digit()));
        let mut results: Vec<SearchResult> = Vec::new();
        let push_result = |doc_id: u32, via_category: bool, results: &mut Vec<SearchResult>| {
            let doc = &self.docs[doc_id as usize];
            let pos = fixed_to_lon_lat(doc.x, doc.y);
            let name = self.doc_name(doc);
            let name_norm = normalize_tokens(name).join(" ");
            let category = Category::from_u16(doc.category);
            let distance_m = near.map(|n| haversine_m(n, pos));
            let score = score_search_hit(
                category,
                doc.rank,
                &name_norm,
                &normalized_query,
                via_category,
                query_has_number,
                distance_m,
            );
            results.push(SearchResult {
                doc_id,
                name: name.to_string(),
                secondary: self.doc_secondary(doc).to_string(),
                category,
                pos,
                distance_m,
                score,
            });
        };

        for &doc_id in &text_candidates {
            push_result(doc_id, false, &mut results);
        }
        for &doc_id in &fuzzy_candidates {
            push_result(doc_id, false, &mut results);
        }
        for &doc_id in &category_candidates {
            if text_candidates.binary_search(&doc_id).is_err()
                && fuzzy_candidates.binary_search(&doc_id).is_err()
            {
                push_result(doc_id, true, &mut results);
            }
        }

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);
        results
    }

    // --- Serialization ---

    pub fn serialize(&self) -> Vec<u8> {
        let mut w = ByteWriter::new();
        w.u32(SEARCH_MAGIC);
        w.u32(SEARCH_VERSION);
        w.u32(self.docs.len() as u32);
        for doc in &self.docs {
            w.u32(doc.x);
            w.u32(doc.y);
            w.u16(doc.category);
            w.u8(doc.rank);
            w.u32(doc.name_start);
            w.u16(doc.name_len);
            w.u32(doc.secondary_start);
            w.u16(doc.secondary_len);
        }
        w.str32(&self.strings);
        w.u32(self.token_strings.len() as u32);
        for (token, (start, len)) in self.token_strings.iter().zip(&self.token_postings) {
            w.str32(token);
            w.u32(*start);
            w.u32(*len);
        }
        w.u32(self.postings.len() as u32);
        for id in &self.postings {
            w.u32(*id);
        }
        w.buf
    }

    pub fn deserialize(data: &[u8]) -> Result<SearchIndex, NavFmtError> {
        let mut r = ByteReader::new(data);
        if r.u32()? != SEARCH_MAGIC {
            return Err(NavFmtError::BadMagic);
        }
        let version = r.u32()?;
        if version != SEARCH_VERSION {
            return Err(NavFmtError::BadVersion(version));
        }
        let doc_count = r.u32()? as usize;
        let mut docs = Vec::with_capacity(doc_count);
        for _ in 0..doc_count {
            docs.push(DocRecord {
                x: r.u32()?,
                y: r.u32()?,
                category: r.u16()?,
                rank: r.u8()?,
                name_start: r.u32()?,
                name_len: r.u16()?,
                secondary_start: r.u32()?,
                secondary_len: r.u16()?,
            });
        }
        let strings = r.str32()?;
        let token_count = r.u32()? as usize;
        let mut token_strings = Vec::with_capacity(token_count);
        let mut token_postings = Vec::with_capacity(token_count);
        for _ in 0..token_count {
            token_strings.push(r.str32()?);
            let start = r.u32()?;
            let len = r.u32()?;
            token_postings.push((start, len));
        }
        let postings_count = r.u32()? as usize;
        let mut postings = Vec::with_capacity(postings_count);
        for _ in 0..postings_count {
            postings.push(r.u32()?);
        }
        for (start, len) in &token_postings {
            if (*start as usize + *len as usize) > postings.len() {
                return Err(NavFmtError::Corrupt("token postings range"));
            }
        }
        for doc in &docs {
            if (doc.name_start + doc.name_len as u32) as usize > strings.len()
                || (doc.secondary_start + doc.secondary_len as u32) as usize > strings.len()
            {
                return Err(NavFmtError::Corrupt("doc string range"));
            }
        }
        let mut category_docs = HashMap::<u16, Vec<u32>>::new();
        for (i, doc) in docs.iter().enumerate() {
            category_docs.entry(doc.category).or_default().push(i as u32);
        }
        for ids in category_docs.values_mut() {
            ids.sort_by(|a, b| docs[*b as usize].rank.cmp(&docs[*a as usize].rank));
        }
        Ok(SearchIndex {
            docs,
            strings,
            token_strings,
            token_postings,
            postings,
            category_docs,
        })
    }
}

/// Google-style tiered ranking. Settlements are near-immune to distance
/// ("brussels" from Amsterdam means Brussels, not the closest
/// Brusselsestraat), POIs and streets stay locally biased, and a number in
/// the query signals address intent.
#[allow(clippy::too_many_arguments)]
pub fn score_search_hit(
    category: Category,
    rank: u8,
    name_norm: &str,
    normalized_query: &str,
    via_category: bool,
    query_has_number: bool,
    distance_m: Option<f64>,
) -> f64 {
    // Entity tier + "reach": how far away this kind of thing stays
    // relevant. The penalty is log2(1 + d/reach), so a zoo 3km away loses
    // ~2 points while a same-named hamlet 900km away loses ~7 — the old
    // locality MULTIPLIER did the opposite (punished the nearby POI harder
    // than the distant settlement, which is how searching "artis" in
    // Amsterdam flew to a hamlet in France).
    let (tier, reach_m) = match category {
        Category::City => (5.0, 50_000.0),
        Category::Town => (4.6, 20_000.0),
        Category::Airport => (4.3, 30_000.0),
        Category::Village | Category::Suburb | Category::Hamlet | Category::Neighbourhood => {
            (4.0, 8_000.0)
        }
        Category::Station => (3.8, 5_000.0),
        Category::Street => (2.0, 800.0),
        Category::Address => (1.6, 500.0),
        _ => (3.0, 1_500.0),
    };
    let mut score = tier * 30.0 + rank as f64 * 0.15;
    if via_category {
        // Category hits rank on prominence + proximity only; a small match
        // deficit so an exact name match wins over a category expansion.
        score += 6.0;
    } else if name_norm == normalized_query {
        score += 40.0;
    } else if name_norm.starts_with(normalized_query) {
        score += 18.0;
    } else {
        score += 8.0;
    }
    if query_has_number && category == Category::Address {
        // "prinsengracht 263" — the number token is address intent.
        score += 55.0;
    }
    if let Some(d) = distance_m {
        // log2 falloff against the tier's reach.
        score -= 7.0 * (1.0 + d / reach_m).log2();
    }
    score
}

/// Levenshtein distance <= k (k is 1 or 2), banded and early-exiting.
fn edit_distance_at_most(a: &[u8], b: &[u8], k: usize) -> bool {
    if a.len().abs_diff(b.len()) > k {
        return false;
    }
    // Band of width 2k+1 around the diagonal.
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        let lo = i.saturating_sub(k).max(1);
        let hi = (i + k).min(b.len());
        if lo > 1 {
            curr[lo - 1] = usize::MAX / 2;
        }
        let mut row_min = usize::MAX;
        for j in lo..=hi {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            curr[j] = (prev[j] + 1)
                .min(curr[j - 1] + 1)
                .min(prev[j - 1] + cost);
            row_min = row_min.min(curr[j]);
        }
        if row_min > k {
            return false;
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()] <= k
}

fn intersect_sorted(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut out = Vec::with_capacity(small.len());
    for &id in small {
        if large.binary_search(&id).is_ok() {
            out.push(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_index() -> SearchIndex {
        let mut b = SearchIndexBuilder::new();
        b.add("Amsterdam", "", LonLat::new(4.9041, 52.3676), Category::City, 255);
        b.add("Amsterdam Centraal", "Amsterdam", LonLat::new(4.9003, 52.3791), Category::Station, 190);
        b.add("Amstelveen", "", LonLat::new(4.8570, 52.3114), Category::Town, 200);
        b.add("Damstraat", "Amsterdam", LonLat::new(4.8957, 52.3722), Category::Street, 90);
        b.add("Dam", "Amsterdam", LonLat::new(4.8932, 52.3731), Category::Attraction, 120);
        b.add("Albert Heijn", "Nieuwezijds Voorburgwal", LonLat::new(4.8920, 52.3750), Category::Supermarket, 85);
        b.add("Albert Heijn", "Vijzelstraat", LonLat::new(4.8910, 52.3620), Category::Supermarket, 85);
        b.add("Café de Prins", "Prinsengracht", LonLat::new(4.8840, 52.3760), Category::Cafe, 60);
        b.build()
    }

    #[test]
    fn tokenizer_folds_diacritics() {
        assert_eq!(normalize_tokens("Café de Prins"), vec!["cafe", "de", "prins"]);
        assert_eq!(normalize_tokens("'s-Gravenhage"), vec!["s", "gravenhage"]);
        assert_eq!(normalize_tokens("  A10/E22  "), vec!["a10", "e22"]);
    }

    #[test]
    fn transcription_error_prefers_settlement() {
        // Whisper anglicizes "Haarlem" to "Harlem"; an exactly-named minor
        // POI must not beat the fuzzy-matched town.
        let mut b = SearchIndexBuilder::new();
        b.add("Haarlem", "", LonLat::new(4.6462, 52.3874), Category::Town, 230);
        b.add(
            "Harlem",
            "parking",
            LonLat::new(4.9000, 52.3700),
            Category::Parking,
            20,
        );
        let index = b.build();
        let results = index.query("harlem", Some(LonLat::new(4.9041, 52.3676)), 10);
        assert_eq!(results[0].name, "Haarlem");
        assert!(results.iter().any(|r| r.name == "Harlem"));
    }

    #[test]
    fn edit_distance_bounds() {
        assert!(edit_distance_at_most(b"harlem", b"haarlem", 1));
        assert!(!edit_distance_at_most(b"harlem", b"haarlemmermeer", 2));
        assert!(edit_distance_at_most(b"utrecht", b"utrecht", 1));
        assert!(!edit_distance_at_most(b"harlem", b"arnhem", 1));
    }

    #[test]
    fn prefix_search_finds_city_first() {
        let index = test_index();
        let results = index.query("amst", None, 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "Amsterdam");
        assert!(results.iter().any(|r| r.name == "Amstelveen"));
    }

    #[test]
    fn exact_match_outranks_prefix() {
        let index = test_index();
        let results = index.query("dam", None, 10);
        assert_eq!(results[0].name, "Dam");
        assert!(results.iter().any(|r| r.name == "Damstraat"));
    }

    #[test]
    fn multi_token_intersection() {
        let index = test_index();
        let results = index.query("amsterdam cen", None, 10);
        assert_eq!(results[0].name, "Amsterdam Centraal");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn proximity_prefers_nearby_supermarket() {
        let index = test_index();
        let near = LonLat::new(4.8918, 52.3752); // next to the NZ Voorburgwal one
        let results = index.query("albert heijn", Some(near), 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].secondary, "Nieuwezijds Voorburgwal");
    }

    #[test]
    fn category_synonym_returns_supermarkets() {
        let index = test_index();
        let near = LonLat::new(4.8918, 52.3752);
        let results = index.query("supermarkt", Some(near), 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].category, Category::Supermarket);
    }

    #[test]
    fn serialization_roundtrip() {
        let index = test_index();
        let bytes = index.serialize();
        let loaded = SearchIndex::deserialize(&bytes).unwrap();
        assert_eq!(loaded.doc_count(), index.doc_count());
        let a = index.query("amsterdam", None, 5);
        let b = loaded.query("amsterdam", None, 5);
        assert_eq!(a.len(), b.len());
        assert_eq!(a[0].name, b[0].name);
    }

    #[test]
    fn category_from_tags() {
        let mut tags = HashMap::new();
        tags.insert("shop".to_string(), "supermarket".to_string());
        assert_eq!(category_from_osm_tags(&tags), Some(Category::Supermarket));
        let mut tags = HashMap::new();
        tags.insert("place".to_string(), "city".to_string());
        assert_eq!(category_from_osm_tags(&tags), Some(Category::City));
        let tags = HashMap::new();
        assert_eq!(category_from_osm_tags(&tags), None);
    }
}
