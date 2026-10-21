// diff_fast corpus program: `#[test] run` calls work() in rounds (the O3 tier installs between rounds); every
// result line is `D ...`. rustc --test -O and `makepad test` (baseline and O3) must print identical D lines.
// `&'static [T]` in static / const initialisers: `&CONST_ARRAY` unsized through a const fn argument, a struct
// field, nested statics, `&[]` / `&[..]` literals next to each other, and `include_bytes!` (Sandbox's race kit
// read every event payload as empty; its vector font slice failed the TTF magic check).
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind { Num, Clock }
#[derive(Clone, Copy, Debug)]
struct FieldDef { id: u64, text: &'static str, kind: Kind }
const fn hash(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut h = 0xcbf29ce484222325u64;
    let mut i = 0;
    while i < b.len() { h = (h ^ b[i] as u64).wrapping_mul(0x100000001b3); i += 1; }
    h
}
impl FieldDef { const fn new(text: &'static str, kind: Kind) -> FieldDef { FieldDef { id: hash(text), text, kind } } }
#[derive(Clone, Copy, Debug)]
struct EventDef { id: u64, text: &'static str, payload: &'static [FieldDef] }
impl EventDef { const fn new(text: &'static str, payload: &'static [FieldDef]) -> EventDef { EventDef { id: hash(text), text, payload } } }
struct KitDef { text: &'static str, fields: &'static [FieldDef], events: &'static [EventDef], masks: u32 }
const fn mask(ps: &[u8]) -> u32 { let mut m = 0; let mut i = 0; while i < ps.len() { m |= 1 << ps[i]; i += 1; } m }

const LAP: [FieldDef; 2] = [FieldDef::new("lap", Kind::Num), FieldDef::new("time", Kind::Clock)];
const SECTOR: [FieldDef; 2] = [FieldDef::new("sector", Kind::Num), FieldDef::new("time", Kind::Clock)];
const FINISH: [FieldDef; 3] = [FieldDef::new("rank", Kind::Num), FieldDef::new("time", Kind::Clock), FieldDef::new("total", Kind::Clock)];
static FIELDS: [FieldDef; 3] = [FieldDef::new("laps", Kind::Num), FieldDef::new("running", Kind::Num), FieldDef::new("best", Kind::Clock)];
static EVENTS: [EventDef; 5] = [
    EventDef::new("start", &[]),
    EventDef::new("lap", &LAP),
    EventDef::new("sector", &SECTOR),
    EventDef::new("finish", &FINISH),
    EventDef::new("inline", &[FieldDef::new("a", Kind::Num), FieldDef::new("b", Kind::Clock)]),
];
static KIT: KitDef = KitDef { text: "race", fields: &FIELDS, events: &EVENTS, masks: mask(&[1, 4, 6]) };
// the same as a const (each use is a fresh value) and through a direct field init
const KIT_C: KitDef = KitDef { text: "racec", fields: &FIELDS, events: &[EventDef::new("x", &LAP), EventDef::new("y", &[])], masks: mask(&[]) };
static DIRECT: EventDef = EventDef { id: 7, text: "direct", payload: &SECTOR };
static DIRECT_LIT: EventDef = EventDef { id: 8, text: "lit", payload: &[FieldDef::new("q", Kind::Num)] };
static NESTED: &[&[u8]] = &[&[], &[1], &[2, 3], &[]];
static BYTES: &[u8] = &[1, 2, 3];
static EMPTY: &[u8] = &[];
static STRS: &[&str] = &["a", "", "bc"];
static INC: &[u8] = include_bytes!("static_slices.rs");
const INC_C: &[u8] = include_bytes!("static_slices.rs");
static INC_ARR: &[u8; 10] = { let b = include_bytes!("static_slices.rs"); let mut a = [0u8; 10]; let mut i = 0; while i < 10 { a[i] = b[i]; i += 1; } &{ a } };
const ARR: [u16; 4] = [9, 8, 7, 6];
static FROM_CONST: &[u16] = &ARR;
static PAIR: (&[u16], &[u16]) = (&ARR, &[]);
fn sum(b: &[u8]) -> u64 { b.iter().map(|&x| x as u64).sum() }

#[inline(never)]
fn show_kit(k: &KitDef) -> String {
    let mut s = format!("{} fields {} [{}] masks {:#x}", k.text, k.fields.len(), k.fields.iter().map(|f| f.text).collect::<Vec<_>>().join(","), k.masks);
    for e in k.events {
        s += &format!("; {} {:x} {}:[{}]", e.text, e.id & 0xffff, e.payload.len(), e.payload.iter().map(|f| format!("{}/{:?}/{:x}", f.text, f.kind, f.id & 0xfff)).collect::<Vec<_>>().join(","));
    }
    s
}

fn work() {
    d!("{}", show_kit(&KIT));
    d!("{}", show_kit(&KIT_C));
    let k = std::hint::black_box(&KIT);
    d!("lap payload via black_box: {}", k.events[1].payload.len());
    d!("direct {} {} {:?}", DIRECT.text, DIRECT.payload.len(), DIRECT.payload.iter().map(|f| f.text).collect::<Vec<_>>());
    d!("direct lit {} {} {:?}", DIRECT_LIT.text, DIRECT_LIT.payload.len(), DIRECT_LIT.payload.first().map(|f| f.text));
    d!("nested {:?}", NESTED);
    d!("bytes {:?} empty {:?} strs {:?}", BYTES, EMPTY, STRS);
    d!("inc len {} head {:?} sum {}", INC.len(), &INC[..12], sum(INC));
    d!("inc const len {} head {:?} same {}", INC_C.len(), &INC_C[..6], INC_C == INC);
    d!("inc arr {:?}", INC_ARR);
    d!("from const {:?} pair {:?}", FROM_CONST, PAIR);
    let local: &'static [FieldDef] = &LAP;
    d!("local promoted {} {:?}", local.len(), local.iter().map(|f| f.text).collect::<Vec<_>>());
}
