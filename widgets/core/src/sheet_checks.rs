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
        vm.capture_errors();
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
    let widget = vm.value(widgets, LiveId::from_str(name))
        .as_object()
        .unwrap_or_else(|| panic!("the library has no widget named {name}"));
    let min = vm.value(widget, id!(min_height)).as_f64();
    let pad = vm.value(widget, id!(padding)).as_object();
    let side = |vm: &mut ScriptVm, key: LiveId| pad.and_then(|p| vm.value(p, key).as_f64());
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
            if vm.value(widgets, LiveId::from_str(name)).as_object().is_none() {
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
            let value = vm.value(theme, LiveId::from_str(face));
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
            let value = vm.value(theme, LiveId::from_str(face));
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
            if vm.value(widgets, ids[0]).as_object().is_none() {
                continue;
            }
            let value = vm.heap().value_path(widgets, &ids, NoTrap);
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
        let shows = vm.value(obj, id!(show_bg)).as_bool();
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
        let val = |key: &str| vm.value(theme, LiveId::from_str(key)).as_color();
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

/// Every shader the sheet writes compiles, and still compiles when an app
/// sets the numbers it reads: each `mod.widgets.<Template>.<draw>.pixel` or
/// `.vertex` the sheet replaces is built into a draw the way a widget builds
/// its own, first with every number the sheet set on that draw set again by
/// an instance (as Stage's `LevelMeter{draw_bg +: {lamp_size: 7}}` does), then as
/// the template stands. A face that fails to compile is not drawn at all --
/// the widget is simply missing under that sheet -- and nothing else would
/// say so short of looking. The instance half catches a sheet that sets a
/// uniform by path with a bare number (`draw_bg.lamp_size = 6.0`): that
/// turns the uniform into a plain value, which compiles as the template has
/// it and fails as soon as an instance sets it; `uniform(6.0)` keeps it one.
/// Templates of a family the registration left out are skipped. Returns how
/// many draws were compiled.
pub fn replaced_shaders_compile(register: fn(&mut ScriptVm), entry: &SheetEntry) -> usize {
    let mut draws: Vec<(&str, &str, Vec<&str>)> = Vec::new();
    for line in entry.widgets.lines() {
        let Some(path) = line.strip_prefix("mod.widgets.") else { continue };
        let Some((path, value)) = path.split_once(" = ") else { continue };
        let parts: Vec<&str> = path.split('.').collect();
        if let [template, draw, "pixel" | "vertex"] = parts[..] {
            if value.starts_with("fn") && !draws.iter().any(|(t, d, _)| (*t, *d) == (template, draw)) {
                draws.push((template, draw, Vec::new()));
            }
        }
    }
    for line in entry.widgets.lines() {
        let Some(path) = line.strip_prefix("mod.widgets.") else { continue };
        let Some((path, value)) = path.split_once(" = ") else { continue };
        let parts: Vec<&str> = path.split('.').collect();
        if let [template, draw, field] = parts[..] {
            let value = value.trim();
            let bare = value.strip_prefix("uniform(").and_then(|v| v.strip_suffix(')')).unwrap_or(value);
            if bare.parse::<f64>().is_ok() {
                if let Some((_, _, numbers)) = draws.iter_mut().find(|(t, d, _)| (*t, *d) == (template, draw)) {
                    numbers.push(field);
                }
            }
        }
    }
    // Only the numbers the library declares as uniforms: a field it keeps a
    // plain value (a Rust draw's own field) is the library's as it stands.
    {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            register(vm);
            let widgets = vm.module(id!(widgets));
            for (template, draw, numbers) in draws.iter_mut() {
                let proto = vm.value(widgets, LiveId::from_str(template));
                let stock = proto.as_object().map(|proto| vm.value(proto, LiveId::from_str(draw)));
                let Some(stock) = stock.and_then(|value| value.as_object()) else { continue };
                numbers.retain(|field| vm.value(stock, LiveId::from_str(field)).as_f64().is_none());
            }
        });
    }
    let mut compiled = 0;
    under(register, entry, |vm, _| {
        let widgets = vm.module(id!(widgets));
        for (template, draw, numbers) in &draws {
            let proto = vm.value(widgets, LiveId::from_str(template));
            let Some(proto) = proto.as_object() else { continue };
            let value = vm.value(proto, LiveId::from_str(draw));
            let Some(draw_obj) = value.as_object() else { continue };
            let mut builds = Vec::new();
            if !numbers.is_empty() {
                let derived = vm.new_with_proto(value);
                for field in numbers {
                    let key: ScriptValue = LiveId::from_str(field).into();
                    let at = vm.value(draw_obj, key).as_f64().unwrap_or(0.0);
                    // A whole number, as `lamp_size: 7` writes it: a uniform takes it
                    // as its float, a bare value keeps it an integer.
                    vm.set_value_def(derived, key, ScriptValue::from_i32(at.round() as i32 + 1));
                }
                builds.push(("an instance setting its numbers", ScriptValue::from(derived)));
            }
            builds.push(("the template", value));
            for (how, build) in builds {
                vm.capture_errors();
                let quad = DrawQuad::script_from_value(vm, build);
                let errors = vm.take_errors();
                assert!(
                    quad.draw_vars.draw_shader_id.is_some(),
                    "{}: {template}.{draw}, built as {how}, does not compile, so it is not drawn: {errors:?}",
                    entry.id
                );
            }
            compiled += 1;
        }
    });
    compiled
}

fn shader_color(vm: &ScriptVm, value: ScriptValue) -> Option<u32> {
    if let Some(c) = value.as_color() {
        return Some(c);
    }
    let obj = value.as_object()?;
    vm.heap().as_shader_io(obj)?;
    vm.heap().proto(obj).as_color()
}

fn max_channel_delta(a: u32, b: u32) -> f64 {
    [24u32, 16, 8]
        .into_iter()
        .map(|s| (((a >> s) & 0xff) as i32 - ((b >> s) & 0xff) as i32).abs() as f64 / 255.0)
        .fold(0.0_f64, f64::max)
}

fn host_over_opaque(face: u32, host: u32, default: u32) -> u32 {
    let ch = |c: u32, shift: u32| ((c >> shift) & 0xff) as i32;
    [24u32, 16, 8].into_iter().fold(0xff, |out, shift| {
        let value = ch(face, shift) + ch(host, shift) - ch(default, shift);
        out | ((value.clamp(0, 255) as u32) << shift)
    })
}

/// `ink`, or the smallest mix toward white/black that reaches `need` on `ground`.
/// Mirrors Fab's `ink_reading` (16 bisections via `theme_tokens::mix_rgba`) so
/// the static check exercises the same zero-margin bisection floor when a
/// sheet's `color_text_on_accent` does not natively read on `color_focus`.
fn fit_ink_on(ground: u32, ink: u32, need: f64) -> u32 {
    use crate::theme_tokens::{mix_rgba, reads_on};
    if reads_on(ground, ink) >= need {
        return ink;
    }
    let end = if reads_on(ground, 0xffffffff) >= reads_on(ground, 0x000000ff) {
        0xffffffff
    } else {
        0x000000ff
    };
    if reads_on(ground, end) < need {
        return end;
    }
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..16 {
        let mid = 0.5 * (lo + hi);
        if reads_on(ground, mix_rgba(ink, end, mid)) >= need {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    mix_rgba(ink, end, hi)
}

/// Every `ButtonFlat`, `ButtonFlatter`, and `RadioButtonTabFlat` reads cleanly
/// on its own face across rest, hover, down, focus, and (for tabs) active:
/// `READABLE` (4.5:1) for words (`draw_text`), `LEGIBLE` (3.0:1) for tinted
/// icons (`draw_icon`).
///
/// Structural guard: any sheet that replaces `draw_bg.pixel` relative to
/// `mod.stock_widgets` must declare `draw_bg.face` (and `ButtonFlatter` must
/// declare its own `draw_bg.face` whenever its `pixel` differs from
/// `ButtonFlat`'s), so a new sheet with a custom face cannot slip past.
/// Opaque `@host_over` `ButtonFlat` faces must also align `color` and
/// `color_default` within 0.06 per channel of `face.rest` and keep fitted
/// `color_text_on_accent` readable (`READABLE`, 4.5:1) on
/// `host_over(face.rest, color_focus, color_default)` so app-painted accents
/// shift from the true baseline.
pub fn button_and_tab_contrast(register: fn(&mut ScriptVm), entry: &SheetEntry) {
    use crate::desktop_style::{try_widget_face, widget_pixel_key, widget_pixel_overridden, FaceMode};
    use crate::theme_tokens::{over, reads_on, LEGIBLE, READABLE};

    under(register, entry, |vm, _| {
        let theme = vm.module(id!(theme));
        let surface = vm
            .value(theme, id!(color_surface_container))
            .as_color()
            .or_else(|| vm.value(theme, id!(color_bg_app)).as_color())
            .expect("sheet has a container or app ground")
            | 0xff;
        let widgets = vm.module(id!(widgets));
        let mut bad: Vec<String> = Vec::new();

        let specs: [(LiveId, &str, bool); 3] = [
            (id!(RadioButtonTabFlat), "RadioButtonTabFlat", true),
            (id!(ButtonFlat), "ButtonFlat", false),
            (id!(ButtonFlatter), "ButtonFlatter", false),
        ];

        let bf_pixel = widget_pixel_key(vm, id!(widgets), id!(ButtonFlat));

        for (wid, wname, has_active) in specs {
            let w = vm.value(widgets, wid).as_object().unwrap_or_else(|| panic!("{}: missing {wname}", entry.id));
            let bg = vm.value(w, id!(draw_bg)).as_object().unwrap_or_else(|| panic!("{}: {wname} missing draw_bg", entry.id));
            let dt = vm.value(w, id!(draw_text)).as_object().unwrap_or_else(|| panic!("{}: {wname} missing draw_text", entry.id));
            let di = vm.value(w, id!(draw_icon)).as_object();

            let face = try_widget_face(vm, wid).unwrap_or_else(|err| panic!("{}: {err}", entry.id));
            let overridden = widget_pixel_overridden(vm, wid);

            // 1. Structural guard: custom pixel shader must declare `draw_bg.face`.
            if overridden {
                if face.is_none() {
                    bad.push(format!("{wname}: overrides draw_bg.pixel without declaring draw_bg.face"));
                    continue;
                }
                if wid == id!(ButtonFlatter) && widget_pixel_key(vm, id!(widgets), wid) != bf_pixel {
                    let has_own_face = vm.heap().map_own_value(bg, id!(face).into()).is_some();
                    if !has_own_face {
                        bad.push(format!("{wname}: overrides draw_bg.pixel separately from ButtonFlat without its own draw_bg.face"));
                        continue;
                    }
                }
            }

            // 2. Resolve per-state face colour, composited over `surface`.
            let read_bg = |vm: &mut ScriptVm, key: LiveId| shader_color(vm, vm.value(bg, key));
            let (rest, hover, down, focus, active) = match face {
                None => {
                    let c = read_bg(vm, id!(color)).unwrap_or(0);
                    if has_active {
                        // Stock RadioButtonTabFlat mixes only `color` and `color_active`.
                        let act = read_bg(vm, id!(color_active)).unwrap_or(c);
                        (c, c, c, c, Some(act))
                    } else {
                        let h = read_bg(vm, id!(color_hover)).unwrap_or(c);
                        let d = read_bg(vm, id!(color_down)).unwrap_or(c);
                        let f = read_bg(vm, id!(color_focus)).unwrap_or(c);
                        (c, h, d, f, None)
                    }
                }
                Some(f) if f.mode == FaceMode::SelfColor => {
                    let c = f.rest.or_else(|| read_bg(vm, id!(color))).unwrap_or(0);
                    let h = f.hover.or_else(|| read_bg(vm, id!(color_hover))).unwrap_or(c);
                    let d = f.down.or_else(|| read_bg(vm, id!(color_down))).unwrap_or(c);
                    let fo = f.focus.or_else(|| read_bg(vm, id!(color_focus))).unwrap_or(c);
                    let act = has_active.then(|| f.active.or_else(|| read_bg(vm, id!(color_active))).unwrap_or(c));
                    (c, h, d, fo, act)
                }
                Some(f) => {
                    let Some(r) = f.rest else {
                        bad.push(format!("{wname}: draw_bg.face missing `rest`"));
                        continue;
                    };
                    let Some(h) = f.hover else {
                        bad.push(format!("{wname}: draw_bg.face missing `hover`"));
                        continue;
                    };
                    let Some(d) = f.down else {
                        bad.push(format!("{wname}: draw_bg.face missing `down`"));
                        continue;
                    };
                    let fo = f.focus.unwrap_or(r);
                    let act = if has_active {
                        match f.active {
                            Some(a) => Some(a),
                            None => {
                                bad.push(format!("{wname}: draw_bg.face missing `active`"));
                                continue;
                            }
                        }
                    } else {
                        None
                    };
                    // 3. `host_over` baseline parity and accent readability on opaque ButtonFlat faces.
                    if wid == id!(ButtonFlat) && f.mode == FaceMode::HostOver && (r & 0xff) == 0xff {
                        let col = read_bg(vm, id!(color)).unwrap_or(0);
                        let def = read_bg(vm, id!(color_default)).unwrap_or(0);
                        if col != def {
                            bad.push(format!(
                                "{wname}: draw_bg.color #{col:08x} != draw_bg.color_default #{def:08x} on opaque @host_over face"
                            ));
                        }
                        let delta = max_channel_delta(r, def);
                        if delta > 0.06 {
                            bad.push(format!(
                                "{wname}: draw_bg.color_default #{def:08x} drifts {delta:.2} from face.rest #{r:08x} (> 0.06)"
                            ));
                        }
                        let accent = vm
                            .value(theme, id!(color_focus))
                            .as_color()
                            .or_else(|| vm.value(theme, id!(color_ctrl_selected)).as_color())
                            .map_or(surface, |c| over(surface, c));
                        let raw_on_accent = vm
                            .value(theme, id!(color_text_on_accent))
                            .as_color()
                            .unwrap_or(0xffffffff);
                        let painted = host_over_opaque(r, accent, def);
                        let on_accent = fit_ink_on(accent, raw_on_accent, READABLE);
                        let ratio = reads_on(painted, on_accent);
                        if ratio < READABLE {
                            bad.push(format!(
                                "{wname}: host_over(face.rest #{r:08x}, color_focus #{accent:08x}, color_default #{def:08x}) = #{painted:08x} under fitted accent ink #{on_accent:08x} = {ratio:.2}:1 (need {READABLE:.1})"
                            ));
                        }
                    }
                    (r, h, d, fo, act)
                }
            };

            let mut states: Vec<(&str, u32, LiveId)> = vec![
                ("rest", over(surface, rest), id!(color)),
                ("hover", over(surface, hover), id!(color_hover)),
                ("down", over(surface, down), id!(color_down)),
                ("focus", over(surface, focus), id!(color_focus)),
            ];
            if let Some(act) = active {
                states.push(("active", over(surface, act), id!(color_active)));
            }

            // 4. Word contrast (`draw_text`, 4.5:1 READABLE) across all states.
            let dt_rest = shader_color(vm, vm.value(dt, id!(color)));
            for &(st, bg_rgb, key) in &states {
                let ink = shader_color(vm, vm.value(dt, key)).or(dt_rest);
                if let Some(ink) = ink {
                    let ratio = reads_on(bg_rgb, ink);
                    if ratio < READABLE {
                        bad.push(format!(
                            "{wname}.draw_text.{st}: #{ink:08x} on #{bg_rgb:08x} = {ratio:.2}:1 (need {READABLE:.1})"
                        ));
                    }
                }
            }

            // 5. Icon contrast (`draw_icon`, 3.0:1 LEGIBLE) whenever tinted with a Splash color
            //    (stock `DrawSvg` uses `vec4(-1,-1,-1,-1)` sentinel -> `shader_color` returns `None`).
            if let Some(di_obj) = di {
                for &(st, bg_rgb, key) in &states {
                    if let Some(ink) = shader_color(vm, vm.value(di_obj, key)) {
                        let ratio = reads_on(bg_rgb, ink);
                        if ratio < LEGIBLE {
                            bad.push(format!(
                                "{wname}.draw_icon.{st}: #{ink:08x} on #{bg_rgb:08x} = {ratio:.2}:1 (need {LEGIBLE:.1})"
                            ));
                        }
                    }
                }
            }
        }

        assert!(
            bad.is_empty(),
            "{}: button/tab ink does not read on its own face:\n{}",
            entry.id,
            bad.join("\n")
        );
    });
}
