//! Static tables a dependent crate reads (relax-diffpkg `static_tables`): `&'static [T]` built in
//! static initialisers that leave the CTFE subset (`str::as_bytes` in a const fn hash, as
//! makepad's LiveId::from_str), so the initialiser runs as code and its `&CONST` / `&[..]`
//! borrows are promoted temporaries it fills. Every domain must see their contents (Sandbox's
//! race kit read each event payload's fields as zeros in the app's domain), and `include_bytes!`
//! slices their bytes (its vector font failed the TTF magic check).

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind { Num, Clock }

#[derive(Clone, Copy, Debug)]
pub struct FieldDef { pub id: u64, pub text: &'static str, pub kind: Kind }

pub const fn hash(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut h = 0xcbf29ce484222325u64;
    let mut i = 0;
    while i < b.len() {
        h = (h ^ b[i] as u64).wrapping_mul(0x100000001b3);
        i += 1;
    }
    h
}

impl FieldDef {
    pub const fn new(text: &'static str, kind: Kind) -> FieldDef { FieldDef { id: hash(text), text, kind } }
}

#[derive(Clone, Copy, Debug)]
pub struct EventDef { pub id: u64, pub text: &'static str, pub payload: &'static [FieldDef] }

impl EventDef {
    pub const fn new(text: &'static str, payload: &'static [FieldDef]) -> EventDef { EventDef { id: hash(text), text, payload } }
}

pub struct KitDef { pub id: u64, pub text: &'static str, pub fields: &'static [FieldDef], pub events: &'static [EventDef], pub mask: u32 }

const fn mask(ps: &[u8]) -> u32 {
    let mut m = 0;
    let mut i = 0;
    while i < ps.len() {
        m |= 1 << ps[i];
        i += 1;
    }
    m
}

const LAP: [FieldDef; 2] = [FieldDef::new("lap", Kind::Num), FieldDef::new("time", Kind::Clock)];
const FINISH: [FieldDef; 3] = [FieldDef::new("rank", Kind::Num), FieldDef::new("time", Kind::Clock), FieldDef::new("total", Kind::Clock)];
pub static FIELDS: [FieldDef; 3] = [FieldDef::new("laps", Kind::Num), FieldDef::new("running", Kind::Num), FieldDef::new("best", Kind::Clock)];
pub static EVENTS: [EventDef; 4] = [
    EventDef::new("start", &[]),
    EventDef::new("lap", &LAP),
    EventDef::new("finish", &FINISH),
    EventDef::new("inline", &[FieldDef::new("a", Kind::Num), FieldDef::new("b", Kind::Clock)]),
];
pub static KIT: KitDef = KitDef { id: hash("race"), text: "race", fields: &FIELDS, events: &EVENTS, mask: mask(&[1, 4, 6]) };
pub static DIRECT: EventDef = EventDef { id: hash("direct"), text: "direct", payload: &LAP };
pub static NESTED: &[&[u8]] = &[&[], &[1], &[2, 3], &[]];
pub static BYTES: &[u8] = &[1, 2, 3];
pub static INC: &[u8] = include_bytes!("lib.rs");
static PRIVATE_INC: &[u8] = include_bytes!("lib.rs");
pub fn private_inc() -> &'static [u8] { PRIVATE_INC }
/// The UI font (182 KB) as a static slice: read here and parsed by relax-diffpkg `font`.
pub static FONT: &[u8] = include_bytes!("../../../../../../widgets/resources/IBMPlexSans-SemiBold.ttf");
/// The same file through include_str!, a const include_bytes! and an array-typed static: relax-diffpkg
/// `zone_swap` first uses them (and FONT, INC) after the compiler-zone swap.
pub static TEXT: &str = include_str!("lib.rs");
pub const CINC: &[u8] = include_bytes!("lib.rs");
pub static ARR: &[u8; CINC.len()] = include_bytes!("lib.rs");
