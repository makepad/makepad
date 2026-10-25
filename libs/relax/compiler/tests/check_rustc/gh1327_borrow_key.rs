// GitHub #1327: `map.get(&(&string, w))` with `K = (&str, W)`. rustc learns `Q = (&str, W)` from
// `K: Borrow<Q>` (one impl applies) when the tuple's expectation is resolved with the pending
// obligations, so `&String` coerces to `&str` element-wise. rustc: no diagnostics.
use std::collections::{BTreeMap, HashMap};
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct FontWeight(u16);
pub struct TextProperties { pub font_family: String, pub font_weight: FontWeight }
pub struct Atlas<'a> {
    fonts: HashMap<(&'a str, FontWeight), (u32, f32)>,
    tree: BTreeMap<(&'a str, FontWeight), (u32, f32)>,
}
impl<'a> Atlas<'a> {
    fn lookup(&self, text_properties: &TextProperties) -> u32 {
        let (atlas_texture, _atlas_font_size) = self
            .fonts
            .get(&(&text_properties.font_family, text_properties.font_weight))
            .unwrap();
        let (t2, _) = self.tree.get(&(&text_properties.font_family, text_properties.font_weight)).unwrap();
        *atlas_texture + *t2
    }
}
fn a(m: &HashMap<(&str, FontWeight), u8>, s: String) -> bool { m.get(&(&s, FontWeight(1))).is_some() }
fn c(m: &HashMap<(&str, u8), u8>, s: String) -> bool { m.get(&(&s, 1)).is_some() }
fn h(m: &HashMap<(&str, u8), u8>, s: &String) -> bool { m.get(&(s, 1)).is_some() }
fn i(m: &HashMap<(&str,), u8>, s: &String) -> bool { m.get(&(s,)).is_some() }
fn main() {
    let mut fonts = HashMap::new();
    let mut tree = BTreeMap::new();
    fonts.insert(("Inter", FontWeight(400)), (7u32, 12.0f32));
    tree.insert(("Inter", FontWeight(400)), (5u32, 12.0f32));
    let atlas = Atlas { fonts, tree };
    let tp = TextProperties { font_family: "Inter".to_string(), font_weight: FontWeight(400) };
    let mut n = HashMap::new();
    n.insert(("x", 1u8), 1u8);
    let mut p = HashMap::new();
    p.insert(("x",), 1u8);
    let mut m = HashMap::new();
    m.insert(("x", FontWeight(1)), 1u8);
    println!("{} {} {} {} {}", atlas.lookup(&tp), a(&m, "x".into()), c(&n, "x".into()), h(&n, &"x".to_string()), i(&p, &"x".to_string()));
}
