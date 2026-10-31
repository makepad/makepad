//! Mixer colours come from the active system theme. `mod.mpm` serves the
//! app's Splash modules; the one-line `let mp = {...}` preamble carries the
//! same token values into surface layout isolates.
//!
//! SAFETY: colours only. Nothing here can name an OSC address — see
//! `makepad_app_mixer::safety`.

use makepad_widgets::*;

/// The layout's colour names and their system theme roles.
const KEYS: &[(&str, &str)] = &[
    ("bg", "color_bg_app"),
    ("bg_dark", "color_bg_container"),
    ("bg_light", "color_fg_app"),
    ("fg", "color_text"),
    ("fg_bright", "color_text_hover"),
    ("fg_dim", "color_text_disabled"),
    ("muted", "color_bevel_outset_2"),
    ("accent", "color_focus"),
    ("red", "color_error"),
    ("green", "color_success"),
    ("yellow", "color_warning"),
    ("cyan", "color_info"),
];

/// Every colour the mixer paints with, as `#rrggbbaa` strings.
#[derive(Clone, Debug)]
pub struct Palette {
    /// `name` -> `#rrggbbaa`, in [`KEYS`] order.
    entries: Vec<(&'static str, String)>,
}

impl Palette {
    /// Read the owning VM again on each application/style reload.
    pub fn for_vm(vm: &mut ScriptVm) -> Self {
        Self {
            entries: KEYS.iter().map(|(name, role)| {
                let color = theme_tokens::color(vm, role)
                    .unwrap_or_else(|| panic!("missing system theme colour: {role}"));
                (*name, format!("#{:08x}", color.to_u32()))
            }).collect(),
        }
    }

    pub fn for_cx(cx: &mut Cx) -> Self { cx.with_vm(Self::for_vm) }

    /// One colour by its layout name.
    pub fn get(&self, name: &str) -> &str {
        self.entries
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
            .expect("a known mixer colour role")
    }

    /// One colour as linear-ish rgb components, for the handful of places
    /// Rust sets a colour directly.
    pub fn rgb3(&self, name: &str) -> [f32; 3] {
        let hex = self.get(name).trim_start_matches('#');
        let nib = |i: usize| -> f32 {
            match hex.as_bytes().get(i).copied().unwrap_or(b'0') {
                c @ b'0'..=b'9' => (c - b'0') as f32,
                c @ b'a'..=b'f' => (c - b'a' + 10) as f32,
                c @ b'A'..=b'F' => (c - b'A' + 10) as f32,
                _ => 0.0,
            }
        };
        let byte = |i: usize| (nib(i * 2) * 16.0 + nib(i * 2 + 1)) / 255.0;
        [byte(0), byte(1), byte(2)]
    }

    /// The palette as ONE line of splash source, to be prepended to a layout
    /// body before it is handed to the `Splash` widget. One line so a script
    /// error's reported line is the layout's own line plus exactly one.
    pub fn splash_preamble(&self) -> String {
        let body: Vec<String> = self
            .entries
            .iter()
            .map(|(name, hex)| format!("{name}: {hex}"))
            .collect();
        format!("let mp = {{{}}}\n", body.join(", "))
    }

    /// Publish `mod.mpm.*` after widget registration and before this crate's
    /// modules, using the same token values as the isolate preamble.
    pub fn install(&self, vm: &mut ScriptVm) {
        let mut code = String::from("mod.mpm = {\n");
        for (name, hex) in &self.entries {
            code.push_str(&format!("    {name}: {hex}\n"));
        }
        code.push_str("}\n");

        code.push_str("true\n");
        vm.eval(ScriptMod::new(env!("CARGO_MANIFEST_DIR"), "mixer_theme", "mixer_theme.splash", 0, 0, &code, &[]));
        for e in vm.take_errors() {
            log!("mixer theme: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_preamble_is_exactly_one_line() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let p = Palette::for_cx(&mut cx);
        let pre = p.splash_preamble();
        assert_eq!(pre.lines().count(), 1, "preamble must stay one line: {pre}");
        assert!(pre.ends_with('\n'));
        assert!(pre.starts_with("let mp = {"));
        // Every colour a layout can ask for is bound.
        for (name, hex) in &p.entries {
            assert!(pre.contains(&format!("{name}: {hex}")), "missing {name}");
        }
    }

    #[test]
    fn rgb_components_follow_the_system_tokens() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        cx.with_vm(|vm| {
            let p = Palette::for_vm(vm);
            for (name, role) in KEYS {
                let c = theme_tokens::color(vm, role).unwrap();
                assert_eq!(p.rgb3(name), [c.x, c.y, c.z]);
            }
        });
    }
}
