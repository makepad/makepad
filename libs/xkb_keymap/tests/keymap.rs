use xkb_keymap::*;
use xkb_keymap::text::*;

#[test]
fn text_is_well_formed() {
    let t = keymap_text(&US);
    assert_eq!(t.matches('{').count(), t.matches('}').count());
    assert!(t.starts_with("xkb_keymap {"));
    for k in US.all_keys() {
        assert!(t.contains(&format!("<I{}> = {};", k.evdev + 8, k.evdev + 8)));
        assert!(t.contains(&format!("key <I{}>", k.evdev + 8)));
    }
    let b = keymap_bytes(&US);
    assert_eq!(*b.last().unwrap(), 0);
    if let Ok(path) = std::env::var("XKB_DUMP") { std::fs::write(path, t).unwrap(); }
}
#[test]
fn keycodes_unique() {
    let mut seen = std::collections::HashSet::new();
    for k in US.all_keys() { assert!(seen.insert(k.evdev), "dup {}", k.evdev); assert!(k.evdev + 8 < 255); }
}
#[test]
fn typing_and_modifiers() {
    let mut t = Tracker::new(&US);
    assert_eq!(t.text(30), Some('a'));
    assert!(t.key(42, true));
    assert_eq!(t.state().depressed, mods::SHIFT);
    assert_eq!(t.text(30), Some('A'));
    assert_eq!(t.text(2), Some('!'));
    assert!(!t.key(42, true)); // repeat ignored
    assert!(!t.key(54, true)); // second shift: mask unchanged
    assert!(!t.key(42, false));
    assert_eq!(t.state().depressed, mods::SHIFT);
    t.key(54, false);
    assert_eq!(t.state().depressed, 0);
    t.key(58, true); t.key(58, false);
    assert!(t.caps());
    assert_eq!(t.text(30), Some('A'));
    t.key(42, true);
    assert_eq!(t.text(30), Some('a')); // shift cancels caps on letters
    t.key(42, false);
    assert_eq!(t.text(2), Some('1')); // caps does not affect digits
    t.key(29, true);
    assert_eq!(t.text(30), None); // ctrl held: no text
    t.key(29, false);
    // keypad follows numlock
    assert_eq!(t.text(79), Some('1'));
    t.key(69, true); t.key(69, false);
    assert_eq!(t.text(79), None); // KP_End
}

fn typed(layout: &'static Layout, keys: &[(u16, bool)], target: u16) -> Option<char> {
    let mut t = Tracker::new(layout);
    for (k, down) in keys { t.key(*k, *down); }
    t.text(target)
}
#[test]
fn layouts_type_their_own_characters() {
    // German: Z and Y swapped, umlauts on the right of P, AltGr gives @ and the euro sign
    assert_eq!(typed(&DE, &[], 21), Some('z'));
    assert_eq!(typed(&DE, &[], 44), Some('y'));
    assert_eq!(typed(&DE, &[], 26), Some('ü'));
    assert_eq!(typed(&DE, &[(42, true)], 40), Some('Ä'));
    assert_eq!(typed(&DE, &[(100, true)], 16), Some('@'));
    assert_eq!(typed(&DE, &[(100, true)], 18), Some('€'));
    assert_eq!(typed(&DE, &[(42, true)], 3), Some('"'));
    assert_eq!(typed(&DE, &[], 13), None); // dead acute
    assert_eq!(typed(&DE, &[(100, true)], 8), Some('{'));
    // French: digits need Shift, A and Q swapped
    assert_eq!(typed(&FR, &[], 2), Some('&'));
    assert_eq!(typed(&FR, &[(42, true)], 2), Some('1'));
    assert_eq!(typed(&FR, &[], 16), Some('a'));
    assert_eq!(typed(&FR, &[], 30), Some('q'));
    assert_eq!(typed(&FR, &[], 39), Some('m'));
    // UK and Dvorak
    assert_eq!(typed(&GB, &[(42, true)], 4), Some('£'));
    assert_eq!(typed(&GB, &[(42, true)], 40), Some('@'));
    assert_eq!(typed(&DVORAK, &[], 32), Some('e'));
    assert_eq!(typed(&DVORAK, &[], 53), Some('z'));
    // US keys pass through untouched, and the AltGr key is only a modifier where a layout has one
    assert_eq!(typed(&DE, &[], 30), Some('a'));
    assert_eq!(by_name("fr").unwrap().name, "fr");
    assert!(by_name("klingon").is_none());
}
#[test]
fn every_layout_compiles_to_balanced_text() {
    for l in [&US, &GB, &DE, &FR, &DVORAK] {
        let t = keymap_text(l);
        assert_eq!(t.matches('{').count(), t.matches('}').count(), "{}", l.name);
        assert!(t.contains(&format!("name[Group1] = \"{}\"", l.name)));
        if let Ok(dir) = std::env::var("XKB_DUMP_DIR") { std::fs::write(format!("{dir}/{}.xkb", l.name), t).unwrap(); }
    }
}
