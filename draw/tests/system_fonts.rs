//! Installed fonts in the text stack: a family member named by system family
//! (`FontMember{system: ...}`), resolved on the task pool and never on the UI
//! thread, and the `System` font set, under which CJK and emoji come from
//! the OS first (the bundled faces only as the last resort). Prints what each
//! machine resolved; asserts only what every machine has.
#![cfg(not(target_arch = "wasm32"))]

use makepad_draw::*;
use std::rc::Rc;

fn new_cx(set: FontSet) -> Cx {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    assert!(cx.set_font_set(set));
    cx.freeze_font_set();
    cx
}

fn draw_text(cx: &mut Cx, member: &'static str) -> DrawText {
    cx.with_vm(|vm| {
        makepad_platform::script::script_mod(vm);
        makepad_draw::script_mod(vm);
        let source = match member {
            "plex" => script! {
                use mod.draw.*
                use mod.text.*
                use mod.res.*
                DrawText{
                    text_style: TextStyle{
                        font_size: 12
                        font_family: FontFamily{
                            latin := FontMember{res: crate_resource("self:../widgets/resources/IBMPlexSans-Text.ttf")}
                        }
                    }
                }
            },
            name => script! {
                use mod.draw.*
                use mod.text.*
                use mod.res.*
                DrawText{
                    text_style: TextStyle{
                        font_size: 12
                        font_family: FontFamily{
                            sys := FontMember{system: #(name) weight: 700.0}
                            latin := FontMember{res: crate_resource("self:../widgets/resources/IBMPlexSans-Text.ttf")}
                        }
                    }
                }
            },
        };
        let value = vm.eval(source);
        DrawText::script_from_value(vm, value)
    })
}

/// The family names of the faces `text` was laid out with, per glyph
/// (`?` for a missing glyph).
fn faces(laidout: &text::layouter::LaidoutText) -> Vec<String> {
    laidout
        .rows
        .iter()
        .flat_map(|row| &row.glyphs)
        .map(|glyph| {
            if glyph.id == 0 {
                return "?".to_string();
            }
            glyph.font.with_ttf_parser_face(|face| {
                face.names()
                    .into_iter()
                    .filter(|name| name.name_id == 1)
                    .find_map(|name| name.to_string().or_else(|| name.name.is_ascii().then(|| String::from_utf8_lossy(name.name).into_owned())))
                    .unwrap_or_default()
            })
        })
        .collect()
}

/// Lays `text` out until the background lookups have answered (the UI
/// thread would redraw on their signal; here the test asks again).
fn layout_settled(cx: &mut Cx, draw: &DrawText, text: &str) -> Rc<text::layouter::LaidoutText> {
    let started = std::time::Instant::now();
    let mut last = Vec::new();
    loop {
        let laidout = draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        let now = faces(&laidout);
        if (now == last && !now.is_empty()) || started.elapsed().as_secs() > 20 {
            return laidout;
        }
        last = now;
        std::thread::sleep(std::time::Duration::from_millis(50));
        // the index build the first frame would start
        makepad_font_core_prewarm();
    }
}

fn makepad_font_core_prewarm() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::thread::spawn(makepad_draw::text::system_fonts::prewarm);
    });
}

#[test]
fn a_system_family_member_resolves_in_the_background() {
    let mut cx = new_cx(FontSet::Latin);
    let ui = draw_text(&mut cx, "system-ui");
    let laidout = layout_settled(&mut cx, &ui, "Hello");
    let names = faces(&laidout);
    println!("system-ui bold: {names:?}");
    assert!(names.iter().all(|name| name != "?"), "{names:?}");

    // A family not installed anywhere: the member is left out, the family
    // draws with IBM Plex.
    let missing = draw_text(&mut cx, "No Such Family 7f3a");
    let laidout = layout_settled(&mut cx, &missing, "Hello");
    let names = faces(&laidout);
    println!("missing family: {names:?}");
    assert!(names.iter().all(|name| name.starts_with("IBM Plex")), "{names:?}");
}

#[test]
fn the_system_set_draws_cjk_and_emoji_from_the_os_first() {
    for set in [FontSet::Latin, FontSet::System] {
        let mut cx = new_cx(set);
        let draw = draw_text(&mut cx, "plex");
        let laidout = layout_settled(&mut cx, &draw, "Hi 中文 한국어 😀");
        let names = faces(&laidout);
        println!("{set:?}: {names:?}");
        // The Latin text is the family's own face under both sets.
        assert!(names[..2].iter().all(|name| name.starts_with("IBM Plex")), "{names:?}");
    }
}
