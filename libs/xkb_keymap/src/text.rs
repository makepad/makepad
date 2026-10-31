//! Emits the `xkb_keymap` text (format XKB_V1) from a layout.
use crate::layout::{Layout, Level};
use std::fmt::Write;

fn keycode_name(evdev: u16) -> String { format!("<I{}>", evdev + 8) }

pub fn keymap_text(layout: &Layout) -> String {
    let mut o = String::new();
    o.push_str("xkb_keymap {\nxkb_keycodes \"host\" {\n    minimum = 8;\n    maximum = 255;\n");
    let keys = layout.all_keys();
    for k in &keys { let _ = writeln!(o, "    {} = {};", keycode_name(k.evdev), k.evdev + 8); }
    o.push_str("    indicator 1 = \"Caps Lock\";\n    indicator 2 = \"Num Lock\";\n    indicator 3 = \"Scroll Lock\";\n};\n");
    o.push_str(concat!(
        "xkb_types \"host\" {\n    virtual_modifiers NumLock,LevelThree;\n",
        "    type \"ONE_LEVEL\" {\n        modifiers = none;\n        map[none] = Level1;\n        level_name[Level1] = \"Any\";\n    };\n",
        "    type \"TWO_LEVEL\" {\n        modifiers = Shift;\n        map[Shift] = Level2;\n        level_name[Level1] = \"Base\";\n        level_name[Level2] = \"Shift\";\n    };\n",
        "    type \"ALPHABETIC\" {\n        modifiers = Shift+Lock;\n        map[Shift] = Level2;\n        map[Lock] = Level2;\n        level_name[Level1] = \"Base\";\n        level_name[Level2] = \"Caps\";\n    };\n",
        "    type \"FOUR_LEVEL\" {\n        modifiers = Shift+LevelThree;\n        map[None] = Level1;\n        map[Shift] = Level2;\n        map[LevelThree] = Level3;\n        map[Shift+LevelThree] = Level4;\n        level_name[Level1] = \"Base\";\n        level_name[Level2] = \"Shift\";\n        level_name[Level3] = \"Alt Base\";\n        level_name[Level4] = \"Shift Alt\";\n    };\n",
        "    type \"FOUR_LEVEL_SEMIALPHABETIC\" {\n        modifiers = Shift+Lock+LevelThree;\n        map[None] = Level1;\n        map[Shift] = Level2;\n        map[Lock] = Level2;\n        map[LevelThree] = Level3;\n        map[Shift+LevelThree] = Level4;\n        map[Lock+LevelThree] = Level3;\n        map[Shift+Lock+LevelThree] = Level4;\n        level_name[Level1] = \"Base\";\n        level_name[Level2] = \"Shift\";\n        level_name[Level3] = \"Alt Base\";\n        level_name[Level4] = \"Shift Alt\";\n    };\n",
        "    type \"KEYPAD\" {\n        modifiers = Shift+NumLock;\n        map[None] = Level1;\n        map[Shift] = Level2;\n        map[NumLock] = Level2;\n        level_name[Level1] = \"Base\";\n        level_name[Level2] = \"Number\";\n    };\n};\n",
    ));
    o.push_str(concat!(
        "xkb_compatibility \"host\" {\n    virtual_modifiers NumLock,LevelThree;\n",
        "    interpret Shift_L { action = SetMods(modifiers = Shift); };\n",
        "    interpret Shift_R { action = SetMods(modifiers = Shift); };\n",
        "    interpret Control_L { action = SetMods(modifiers = Control); };\n",
        "    interpret Control_R { action = SetMods(modifiers = Control); };\n",
        "    interpret Alt_L { action = SetMods(modifiers = Mod1); };\n",
        "    interpret Alt_R { action = SetMods(modifiers = Mod1); };\n",
        "    interpret Super_L { action = SetMods(modifiers = Mod4); };\n",
        "    interpret Super_R { action = SetMods(modifiers = Mod4); };\n",
        "    interpret ISO_Level3_Shift { virtualModifier = LevelThree; action = SetMods(modifiers = LevelThree); };\n",
        "    interpret Caps_Lock { action = LockMods(modifiers = Lock); };\n",
        "    interpret Num_Lock { virtualModifier = NumLock; action = LockMods(modifiers = NumLock); };\n",
        "    indicator \"Caps Lock\" { whichModState = Locked; modifiers = Lock; };\n",
        "    indicator \"Num Lock\" { whichModState = Locked; modifiers = NumLock; };\n};\n",
    ));
    let _ = write!(o, "xkb_symbols \"host\" {{\n    name[Group1] = \"{}\";\n", layout.name);
    for k in &keys {
        let ty = match k.kind { Level::One => None, Level::Two => Some("TWO_LEVEL"), Level::Alphabetic => Some("ALPHABETIC"), Level::Keypad => Some("KEYPAD"), Level::Four => Some("FOUR_LEVEL"), Level::FourAlpha => Some("FOUR_LEVEL_SEMIALPHABETIC") };
        let syms = k.syms.join(", ");
        match ty {
            Some(t) => { let _ = writeln!(o, "    key {} {{ type = \"{t}\", symbols[Group1] = [ {syms} ] }};", keycode_name(k.evdev)); }
            None => { let _ = writeln!(o, "    key {} {{ type = \"ONE_LEVEL\", symbols[Group1] = [ {syms} ] }};", keycode_name(k.evdev)); }
        }
    }
    let group = |o: &mut String, m: &str, names: &[&str]| {
        let keys: Vec<String> = keys.iter().filter(|k| names.contains(&k.syms[0])).map(|k| keycode_name(k.evdev)).collect();
        if !keys.is_empty() { let _ = writeln!(o, "    modifier_map {m} {{ {} }};", keys.join(", ")); }
    };
    group(&mut o, "Shift", &["Shift_L", "Shift_R"]);
    group(&mut o, "Lock", &["Caps_Lock"]);
    group(&mut o, "Control", &["Control_L", "Control_R"]);
    group(&mut o, "Mod1", &["Alt_L", "Alt_R"]);
    group(&mut o, "Mod2", &["Num_Lock"]);
    group(&mut o, "Mod4", &["Super_L", "Super_R"]);
    group(&mut o, "Mod5", &["ISO_Level3_Shift"]);
    o.push_str("};\n};\n");
    o
}
/// The bytes to put in the keymap fd: the text plus a terminating NUL (the size sent to clients includes it).
pub fn keymap_bytes(layout: &Layout) -> Vec<u8> {
    let mut b = keymap_text(layout).into_bytes();
    b.push(0);
    b
}
