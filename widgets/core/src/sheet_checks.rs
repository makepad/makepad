//! What every sheet in the catalogue is held to, one sheet at a time. The
//! library's tests run these over its own sheets; an app that registers its
//! own runs them over those, with `register` its own full registration (the
//! core's `script_mod`, or the front crate's with every family), so a family
//! it links is held to the sheet too. Each check panics with the sheet's id
//! and what is wrong.
//!
//! Built for the library's tests and, with the `sheet-checks` feature, for an
//! app's.
use crate::desktop_style::{install, uninstall, SheetEntry, StyleSheet};
use crate::*;

/// A fresh VM with the library registered, the sheet installed and the
/// library registered again under it, the errors of that second run checked.
fn under(register: fn(&mut ScriptVm), entry: &SheetEntry, check: impl FnOnce(&mut ScriptVm, &StyleSheet)) {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.init_cx_os();
    cx.with_vm(|vm| {
        register(vm);
        let sheet = StyleSheet::load(entry);
        install(vm, sheet.clone());
        vm.bx.captured_errors = Some(Vec::new());
        vm.with_reload(register);
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "{}: the sheet does not evaluate: {errors:?}", entry.id);
        check(vm, &sheet);
        uninstall(vm);
    });
}

/// The sheet evaluates without an error, names the base theme it writes into
/// on a line of its own, and ends both halves in a bare `true`: the last
/// statement of an evaluated script is swallowed, so a half that ends on an
/// assignment loses it silently.
pub fn evaluates(register: fn(&mut ScriptVm), entry: &SheetEntry) {
    assert!(
        crate::theme_tokens::sheet_base(entry).is_some(),
        "{}: the sheet names no base theme (`mod.theme = mod.themes.dark` or `.light`)",
        entry.id
    );
    for (half, text) in [("theme", entry.theme), ("widgets", entry.widgets)] {
        assert_eq!(text.lines().last().map(str::trim), Some("true"), "{}: the {half} half does not end in `true`", entry.id);
    }
    under(register, entry, |_, _| {});
}

/// A number the sheet leaves in `mod.theme`, read under it.
pub fn theme_number(register: fn(&mut ScriptVm), entry: &SheetEntry, key: &str) -> Option<f64> {
    let mut out = None;
    under(register, entry, |vm, _| {
        let theme = vm.module(id!(theme));
        out = vm.bx.heap.value(theme, LiveId::from_str(key).into(), NoTrap).as_f64();
    });
    out
}

/// The fields a person types or picks a value in that are laid out by a
/// TURTLE: they are Fit, so the vertical padding IS their height, and two
/// of them padded differently part company again as soon as the type
/// grows. `TextInput` heads the list because every sheet already reshaped
/// it, and it is the one the rest of the row has to match.
///
/// Left out, with reasons, so the next reader does not take the gaps for
/// oversights: `WellInput` is the inside of a field, not a field, and is
/// the one thing that must impose no height at all; `Select` wears a
/// `Button` for a face and follows the button rule; `TreeSelect` names its
/// corner `radius` and packs chips into whatever box its own Rust is
/// handed. `ComboBox`, `DropDown2`, `TagField` and `NumberField` are the
/// pickers family's, and are held to the row where that family is
/// registered (widgets/families/pickers).
const FIELD_FIT: &[&str] = &["TextInput", "DropDown", "FieldWell"];
/// The fields that state a FRAME and lay their own parts out inside it: a
/// number with a stepper, a number you drag, a date, a time, and the two
/// pickers that wrap a date. The row's height is all these can take from a
/// sheet. Each measures its parts against the turtle's whole rect and
/// draws them from the content origin, so a vertical padding displaces the
/// line instead of holding it off the box -- padded, the number sat on the
/// bottom edge of its box with its two arrows split around it. They must
/// therefore carry NO vertical padding, and that is asserted below rather
/// than skipped, because it is the thing the next sheet would get wrong.
/// Those of a family the registration left out are skipped.
const FIELD_FRAME: &[&str] = &["ValueInput", "DateField", "TimeField", "DatePicker", "DateRangePicker"];

/// A field's box metrics as the sheet leaves them: the minimum height,
/// and the padding above and below the line.
pub fn field_metrics(vm: &mut ScriptVm, name: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
    let widgets = vm.module(id!(widgets));
    let widget = vm
        .bx
        .heap
        .value(widgets, LiveId::from_str(name).into(), NoTrap)
        .as_object()
        .unwrap_or_else(|| panic!("the library has no widget named {name}"));
    let min = vm.bx.heap.value(widget, id!(min_height).into(), NoTrap).as_f64();
    let pad = vm.bx.heap.value(widget, id!(padding).into(), NoTrap).as_object();
    let side = |vm: &mut ScriptVm, key: LiveId| pad.and_then(|p| vm.bx.heap.value(p, key.into(), NoTrap).as_f64());
    (min, side(vm, id!(top)), side(vm, id!(bottom)))
}

/// A row of fields is one height under the sheet.
///
/// The sheets used to reshape the text box alone: with the android sheet
/// on, the catalogue's search box became a 48 point pill and the number
/// field beside it stayed 24 tall, the drop down after it 26. Each sheet
/// states its field metrics once and hands them to the whole family, and
/// this is what holds that: a field added to the library, or a sheet added
/// to the catalogue, cannot quietly stand at a height of its own.
pub fn field_row(register: fn(&mut ScriptVm), entry: &SheetEntry) {
    let sheet = entry.id;
    under(register, entry, |vm, _| {
        let row = field_metrics(vm, "TextInput");
        assert!(row.0.is_some(), "{sheet} states no field height for the row to stand at");
        for name in FIELD_FIT {
            assert_eq!(field_metrics(vm, name), row, "{sheet}: {name} does not stand in the row its TextInput sets");
        }
        let widgets = vm.module(id!(widgets));
        for name in FIELD_FRAME {
            if vm.bx.heap.value(widgets, LiveId::from_str(name).into(), NoTrap).as_object().is_none() {
                continue;
            }
            let field = field_metrics(vm, name);
            assert_eq!(field.0, row.0, "{sheet}: {name} does not stand at the height of the row");
            assert!(
                field.1.unwrap_or(0.0) == 0.0 && field.2.unwrap_or(0.0) == 0.0,
                "{sheet}: {name} lays its own parts out, so a vertical padding on it moves the line off the box"
            );
        }
        // The chrome-less input inside a well inherits TextInput, and a
        // minimum as tall as the whole field, applied inside a well already
        // that tall, pushes the line out through the bottom.
        assert_eq!(field_metrics(vm, "WellInput").0, Some(0.0), "{sheet}: the input inside a well must impose no height");
    });
}

/// Every face the sheet gives the app keeps the app's fallbacks behind it,
/// and how many faces were read.
///
/// A family is the face a sheet picks FIRST and then the faces the font
/// policy puts after it -- the scripts the first face has no glyphs for,
/// the emoji -- so that text typed into a field is spelled whatever it
/// holds. A sheet that writes `FontFamily{latin := ..}` builds a family of
/// one face, and every one of those is lost; and one that adds a member
/// to the base's family (`font_family{latin := ..}`) puts it LAST, where
/// it is only a fallback. The way to change the face is to replace the
/// family's first member by its name, the family otherwise the base's:
/// `mod.theme.font_regular.font_family{ibm_plex_text := FontMember{..}}`.
///
/// Read: the theme's own faces, and every text style the sheet's widget
/// half writes onto a template the registration has. Each must end in the
/// fallbacks of one of the stock faces that has any -- the regular's or the
/// bold's, whichever it was built on. A template may also wear a stock face
/// as it is (the code face, which is one face in the stock theme too); the
/// theme's own faces may not, since everything the app writes is written in
/// them.
pub fn font_fallbacks(register: fn(&mut ScriptVm), entry: &SheetEntry) -> usize {
    const FACES: &[&str] = &["font_regular", "font_label", "font_bold", "font_italic", "font_bold_italic", "font_code"];
    let members = |vm: &mut ScriptVm, value: ScriptValue| -> Vec<String> {
        let text = TextStyle::script_from_value(vm, value);
        text.font_family.member_ids().map(|id| id.to_string()).collect()
    };
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut checked = 0;
    cx.with_vm(|vm| {
        register(vm);
        let theme = vm.module(id!(theme));
        let mut stock: Vec<Vec<String>> = Vec::new();
        let mut whole: Vec<Vec<String>> = Vec::new();
        let mut faces: Vec<&str> = Vec::new();
        for face in FACES {
            let value = vm.bx.heap.value(theme, LiveId::from_str(face).into(), NoTrap);
            let have = members(vm, value);
            if have.len() > 1 {
                stock.push(have[1..].to_vec());
                faces.push(face);
            }
            whole.push(have);
        }
        assert!(stock.len() >= 2, "the stock faces have no fallbacks to keep: {stock:?}");
        let sheet = StyleSheet::load(entry);
        install(vm, sheet.clone());
        vm.with_reload(register);
        assert!(vm.take_errors().is_empty(), "{} does not evaluate", sheet.name);
        let mut sites: Vec<(String, ScriptValue)> = Vec::new();
        let theme = vm.module(id!(theme));
        for face in &faces {
            let value = vm.bx.heap.value(theme, LiveId::from_str(face).into(), NoTrap);
            sites.push((format!("theme.{face}"), value));
        }
        let widgets = vm.module(id!(widgets));
        for line in sheet.widgets.lines() {
            let Some(path) = line
                .trim()
                .strip_prefix("mod.widgets.")
                .and_then(|rest| rest.split_once(" = "))
                .map(|(path, _)| path.trim())
                .filter(|path| path.ends_with(".text_style"))
            else {
                continue;
            };
            let ids: Vec<LiveId> = path.split('.').map(LiveId::from_str).collect();
            // A template of a family this registration left out: the sheet's
            // line for it was blanked at install.
            if vm.bx.heap.value(widgets, ids[0].into(), NoTrap).as_object().is_none() {
                continue;
            }
            let value = vm.bx.heap.value_path(widgets, &ids, NoTrap);
            sites.push((path.to_string(), value));
        }
        for (site, value) in sites {
            assert!(value.as_object().is_some(), "{}: `{site}` did not resolve", sheet.name);
            let have = members(vm, value);
            let stock_face = !site.starts_with("theme.") && whole.contains(&have);
            assert!(
                stock_face || stock.iter().any(|fallbacks| fallbacks.iter().all(|id| have[1..].contains(id))),
                "{}: `{site}` lost the fallbacks: it has {have:?}, and every stock face ends in one of {stock:?}",
                sheet.name
            );
            checked += 1;
        }
        uninstall(vm);
    });
    checked
}

/// A sheet lays a window ground with two lines, and a window under a sheet
/// that writes neither keeps its ground off, so it only clears as it always
/// did. Read off a window made from the template the way an app makes one,
/// since that is where the sheet's writes have to land.
pub fn window_ground(register: fn(&mut ScriptVm), entry: &SheetEntry) {
    under(register, entry, |vm, sheet| {
        let window = script_eval!(vm, {mod.widgets.Window{}});
        let obj = window.as_object().expect("a window object");
        let shows = vm.bx.heap.value(obj, id!(show_bg).into(), NoTrap).as_bool();
        let asks = sheet.widgets.contains("mod.widgets.Window.show_bg = true");
        assert_eq!(shows, Some(asks), "{}: the window's ground", entry.id);
    });
}

/// Text reads on its own ground under the sheet: every meaning family, every
/// rung of the surface ladder and the inverse page at `READABLE`, the
/// variants at `LEGIBLE`. A sheet's grounds are its own, and what reads on
/// the base theme's may not read on them.
pub fn contrast(register: fn(&mut ScriptVm), entry: &SheetEntry) {
    use crate::theme_tokens::{reads_on, INVERSE, LEGIBLE, MEANING, READABLE, SURFACES, VARIANTS};
    under(register, entry, |vm, _| {
        let theme = vm.module(id!(theme));
        let val = |key: &str| vm.bx.heap.value(theme, LiveId::from_str(key).into(), NoTrap).as_color();
        let mut bad = Vec::new();
        for (pairs, need) in [(MEANING, READABLE), (SURFACES, READABLE), (INVERSE, READABLE), (VARIANTS, LEGIBLE)] {
            for (ground, ink) in pairs {
                if let (Some(g), Some(i)) = (val(ground), val(ink)) {
                    let contrast = reads_on(g | 0xFF, i);
                    if contrast < need {
                        bad.push(format!("{ink} on {ground} = {contrast:.2}, wanted {need}"));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{}: ink that does not read on its ground:\n{}", entry.id, bad.join("\n"));
    });
}

/// A button face the sheet draws itself still shows the colour the app
/// paints on it: an app lights a latch (the page it is on, a mute that is
/// down) by setting the face's `color`, and a face that fills with a colour
/// of its own hides which latches are on. Every `Button*` face the sheet
/// replaces reads the host's colour, through `host_over` or `host_face`
/// (the sheet's own look, moved to the host's colour where it asks for one)
/// or through `face_fill` (the library's state mix).
pub fn button_faces_take_the_host_colour(entry: &SheetEntry) {
    let text = entry.widgets;
    let mut rest = text;
    while let Some(at) = rest.find("mod.widgets.Button") {
        let line_end = rest[at..].find('\n').map_or(rest.len(), |end| at + end);
        let line = &rest[at..line_end];
        rest = &rest[line_end..];
        if !line.contains(".draw_bg.pixel = fn") {
            continue;
        }
        // The body runs to the first line that closes it at column 0.
        let body_end = rest.find("\n}").unwrap_or(rest.len());
        let body = &rest[..body_end];
        let name = line.split(".draw_bg").next().unwrap_or(line);
        assert!(
            ["host_over(", "host_face(", "face_fill("].iter().any(|reads| body.contains(reads)),
            "{}: {name}'s face does not take the colour the app paints on it (route it through self.host_over)",
            entry.id
        );
    }
}
