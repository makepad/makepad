//! View-weapon presentation: how a pack's artwork is drawn as the player's
//! OWN held weapon — the frames of each state at their authored tics, the
//! muzzle flash drawn over them, the walking bob, the raise and lower on a
//! switch, and the sound each step makes.
//!
//! It rides inside the pack's stateful-billboard manifest as `view…` lines,
//! which every older reader ignores, so the engine never has to know which
//! game the artwork came from: Doom's `p_pspr.c` state table, Duke's
//! per-weapon screen positions and Quake's view-model frame runs all land
//! in these same lines at import time, and one reader plays them all.
//!
//! ```text
//! view v=1 key=billboards/doom1/shtg screen=320x200 aspect=1.2 rest=1,32 bottom=128 raise=6 bob=16,64 tics=35 flash=billboards/doom1/shtf
//! view-place A=122,140 B=44,79 C=31,49 D=30,69
//! view-clip ready loop=1 A:1
//! view-clip fire loop=0 A:3 A:7,flash=0,sound=sfx/doom1/dsshotgn B:5 C:5 D:4 C:5 B:5 A:3 A:7
//! view-clip flash loop=0 A:4 B:3
//! view-sound raise=sfx/doom1/dssawup idle=sfx/doom1/dssawidl
//! ```
//!
//! Coordinates: `screen` is the authored screen space the placements live
//! in (Doom, Duke and Quake: 320×200), y down; `aspect` is the height of
//! one of its pixels relative to its width (1.2 on the 4:3 CRT those games
//! drew for). `rest` is the hand origin at rest, and a placement is a
//! frame's top-left when the hand is there — so a bobbing or rising hand
//! moves every frame by one delta. `bottom` is the origin's y when the
//! weapon is fully lowered, `raise` how many units it climbs per tic, `bob`
//! the amplitude in units and the period in tics, `tics` how many tics a
//! second the clips count in.
//!
//! Clips: `ready` loops while the weapon is held, `fire` plays once per
//! shot, `flash` is the muzzle flash. A step is `LETTER:TICS` with optional
//! `,flash=N` (entering the step lights the flash clip from its step `N`;
//! `flash=rand` lights ONE random flash step) and `,sound=KEY`. The flash
//! clip's letters index the `flash` pack when one is named, else this
//! pack's own sheet (the super shotgun keeps its flash in its own lumps).
//!
//! Keys — `flash`, step `sound`s, `view-sound`s — are pack-relative asset
//! keys, the same shape the actor tables use; [`ViewWeapon::resolve_key`]
//! joins them to the root the manifest was fetched from.

use std::collections::BTreeMap;

pub const VIEW_SCHEMA_VERSION: u32 = 1;

/// Which flash step a `fire` step lights when it begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashStart {
    /// Play the flash clip from this step to its end.
    At(usize),
    /// Play exactly one flash step, chosen at random (Doom's plasma rifle).
    Random,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewStep {
    pub letter: char,
    pub tics: u32,
    pub flash: Option<FlashStart>,
    /// Pack-relative sound key played when this step begins.
    pub sound: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewClip {
    pub name: String,
    pub looping: bool,
    pub steps: Vec<ViewStep>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewWeapon {
    /// This pack's own key (`billboards/doom1/shtg`), so a reader can turn
    /// the alias it fetched into the pack root every other key hangs off.
    pub key: String,
    pub screen: (u32, u32),
    pub aspect: f32,
    pub rest: (f32, f32),
    pub bottom: f32,
    pub raise: f32,
    /// (amplitude in screen units, period in tics)
    pub bob: (f32, u32),
    pub tics: u32,
    /// The sibling pack whose sheet the `flash` clip's letters index.
    pub flash: Option<String>,
    /// Frame letter → top-left at rest, in `screen` space.
    pub places: BTreeMap<char, (f32, f32)>,
    pub clips: Vec<ViewClip>,
    /// Event (`raise`, `idle`, …) → pack-relative sound key.
    pub sounds: BTreeMap<String, String>,
}

impl Default for ViewWeapon {
    fn default() -> Self {
        Self {
            key: String::new(),
            screen: (320, 200),
            aspect: 1.2,
            rest: (1.0, 32.0),
            bottom: 128.0,
            raise: 6.0,
            bob: (16.0, 64),
            tics: 35,
            flash: None,
            places: BTreeMap::new(),
            clips: Vec::new(),
            sounds: BTreeMap::new(),
        }
    }
}

impl ViewWeapon {
    /// The `view…` lines of a manifest, or `None` when it carries none —
    /// the caller then knows the pack was imported before this schema (or
    /// is not a held weapon at all) and can fall back to the plain states.
    pub fn parse(text: &str) -> Option<Self> {
        let mut out = ViewWeapon::default();
        let mut seen = false;
        for line in text.lines() {
            let line = line.trim();
            let Some((keyword, rest)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let rest = rest.trim();
            match keyword {
                "view" => {
                    seen = true;
                    for (k, v) in kv_pairs(rest) {
                        match k {
                            "key" => out.key = v.to_string(),
                            "screen" => {
                                if let Some((w, h)) = v.split_once('x') {
                                    if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
                                        if w > 0 && h > 0 {
                                            out.screen = (w, h);
                                        }
                                    }
                                }
                            }
                            "aspect" => out.aspect = f(v).filter(|a| *a > 0.0).unwrap_or(out.aspect),
                            "rest" => out.rest = pair(v).unwrap_or(out.rest),
                            "bottom" => out.bottom = f(v).unwrap_or(out.bottom),
                            "raise" => out.raise = f(v).filter(|r| *r > 0.0).unwrap_or(out.raise),
                            "bob" => {
                                if let Some((a, p)) = pair(v) {
                                    if p >= 1.0 {
                                        out.bob = (a.max(0.0), p as u32);
                                    }
                                }
                            }
                            "tics" => {
                                out.tics = v.parse().ok().filter(|t| *t > 0).unwrap_or(out.tics)
                            }
                            "flash" => out.flash = non_dash(v),
                            _ => {}
                        }
                    }
                }
                "view-place" => {
                    seen = true;
                    for (k, v) in kv_pairs(rest) {
                        let Some(letter) = single_letter(k) else { continue };
                        if let Some(at) = pair(v) {
                            out.places.insert(letter, at);
                        }
                    }
                }
                "view-clip" => {
                    seen = true;
                    let mut words = rest.split_whitespace();
                    let Some(name) = words.next() else { continue };
                    let mut looping = false;
                    let mut steps = Vec::new();
                    for word in words {
                        if let Some(v) = word.strip_prefix("loop=") {
                            looping = v != "0";
                            continue;
                        }
                        if let Some(step) = parse_step(word) {
                            steps.push(step);
                        }
                    }
                    if !steps.is_empty() {
                        out.clips.retain(|c| c.name != name);
                        out.clips.push(ViewClip {
                            name: name.to_string(),
                            looping,
                            steps,
                        });
                    }
                }
                "view-sound" => {
                    seen = true;
                    for (k, v) in kv_pairs(rest) {
                        if let Some(key) = non_dash(v) {
                            out.sounds.insert(k.to_string(), key);
                        }
                    }
                }
                _ => {}
            }
        }
        seen.then_some(out)
    }

    /// The manifest lines, ready to append to a stateful-billboard text.
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "view v={} key={} screen={}x{} aspect={} rest={},{} bottom={} raise={} bob={},{} tics={} flash={}\n",
            VIEW_SCHEMA_VERSION,
            dash(&self.key),
            self.screen.0,
            self.screen.1,
            num(self.aspect),
            num(self.rest.0),
            num(self.rest.1),
            num(self.bottom),
            num(self.raise),
            num(self.bob.0),
            self.bob.1,
            self.tics,
            dash(self.flash.as_deref().unwrap_or("")),
        );
        if !self.places.is_empty() {
            out.push_str("view-place");
            for (letter, (x, y)) in &self.places {
                out.push_str(&format!(" {letter}={},{}", num(*x), num(*y)));
            }
            out.push('\n');
        }
        for clip in &self.clips {
            out.push_str(&format!("view-clip {} loop={}", clip.name, u8::from(clip.looping)));
            for step in &clip.steps {
                out.push_str(&format!(" {}:{}", step.letter, step.tics));
                match step.flash {
                    Some(FlashStart::At(i)) => out.push_str(&format!(",flash={i}")),
                    Some(FlashStart::Random) => out.push_str(",flash=rand"),
                    None => {}
                }
                if let Some(sound) = &step.sound {
                    out.push_str(&format!(",sound={sound}"));
                }
            }
            out.push('\n');
        }
        if !self.sounds.is_empty() {
            out.push_str("view-sound");
            for (event, key) in &self.sounds {
                out.push_str(&format!(" {event}={key}"));
            }
            out.push('\n');
        }
        out
    }

    pub fn clip(&self, name: &str) -> Option<&ViewClip> {
        self.clips.iter().find(|c| c.name == name)
    }

    /// Every pack-relative key this weapon can ask for, deduplicated: the
    /// flash pack and every sound. A host fetches them once, up front.
    pub fn referenced_keys(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |key: &str| {
            if !key.is_empty() && !out.iter().any(|k| k == key) {
                out.push(key.to_string());
            }
        };
        if let Some(flash) = &self.flash {
            push(flash);
        }
        for clip in &self.clips {
            for step in &clip.steps {
                if let Some(sound) = &step.sound {
                    push(sound);
                }
            }
        }
        for key in self.sounds.values() {
            push(key);
        }
        out
    }

    /// Does any `fire` step make a sound of its own? A host that otherwise
    /// plays a generic shot sound on the fire event stands down when it does.
    pub fn fire_has_sound(&self) -> bool {
        self.clip("fire")
            .is_some_and(|c| c.steps.iter().any(|s| s.sound.is_some()))
    }

    /// The catalog alias of a pack-relative key, given the alias THIS
    /// manifest was fetched under. The root is the alias with this pack's
    /// own key stripped (`doom/doom/billboards/doom1/shtg` − `billboards/
    /// doom1/shtg` = `doom/doom/`); a manifest that did not name its key
    /// falls back to everything before the `/billboards/` folder.
    pub fn resolve_key(&self, alias: &str, key: &str) -> String {
        let root = if !self.key.is_empty() && alias.ends_with(&self.key) {
            &alias[..alias.len() - self.key.len()]
        } else if let Some((head, _)) = alias.split_once("/billboards/") {
            return format!("{head}/{key}");
        } else {
            match alias.rsplit_once('/') {
                Some((head, _)) => return format!("{head}/{key}"),
                None => return key.to_string(),
            }
        };
        format!("{}{key}", root)
    }
}

fn kv_pairs(rest: &str) -> impl Iterator<Item = (&str, &str)> {
    rest.split_whitespace().filter_map(|w| w.split_once('='))
}

fn f(v: &str) -> Option<f32> {
    v.parse::<f32>().ok().filter(|x| x.is_finite())
}

fn pair(v: &str) -> Option<(f32, f32)> {
    let (a, b) = v.split_once(',')?;
    Some((f(a)?, f(b)?))
}

fn non_dash(v: &str) -> Option<String> {
    (!v.is_empty() && v != "-").then(|| v.to_string())
}

fn dash(v: &str) -> &str {
    if v.is_empty() {
        "-"
    } else {
        v
    }
}

fn single_letter(k: &str) -> Option<char> {
    let mut chars = k.chars();
    let c = chars.next()?;
    (chars.next().is_none() && c.is_ascii_alphabetic()).then(|| c.to_ascii_uppercase())
}

/// A finite float without a trailing `.0` — manifests are read by people.
fn num(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1.0e9 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// `A:7,flash=0,sound=sfx/doom1/dsshotgn`
fn parse_step(word: &str) -> Option<ViewStep> {
    let mut parts = word.split(',');
    let head = parts.next()?;
    let (letter, tics) = head.split_once(':')?;
    let letter = single_letter(letter)?;
    let tics: u32 = tics.parse().ok()?;
    let mut step = ViewStep {
        letter,
        tics,
        flash: None,
        sound: None,
    };
    for flag in parts {
        match flag.split_once('=') {
            Some(("flash", "rand")) => step.flash = Some(FlashStart::Random),
            Some(("flash", i)) => step.flash = i.parse().ok().map(FlashStart::At),
            Some(("sound", key)) => step.sound = non_dash(key),
            _ => {}
        }
    }
    Some(step)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shotgun() -> ViewWeapon {
        let step = |l: char, t: u32| ViewStep {
            letter: l,
            tics: t,
            flash: None,
            sound: None,
        };
        let mut fire = vec![step('A', 3)];
        fire.push(ViewStep {
            letter: 'A',
            tics: 7,
            flash: Some(FlashStart::At(0)),
            sound: Some("sfx/doom1/dsshotgn".into()),
        });
        fire.extend([
            step('B', 5),
            step('C', 5),
            step('D', 4),
            step('C', 5),
            step('B', 5),
            step('A', 3),
            step('A', 7),
        ]);
        ViewWeapon {
            key: "billboards/doom1/shtg".into(),
            flash: Some("billboards/doom1/shtf".into()),
            places: [('A', (122.0, 140.0)), ('B', (44.0, 79.0))].into_iter().collect(),
            clips: vec![
                ViewClip {
                    name: "ready".into(),
                    looping: true,
                    steps: vec![step('A', 1)],
                },
                ViewClip {
                    name: "fire".into(),
                    looping: false,
                    steps: fire,
                },
                ViewClip {
                    name: "flash".into(),
                    looping: false,
                    steps: vec![step('A', 4), step('B', 3)],
                },
            ],
            sounds: [("raise".to_string(), "sfx/doom1/dssawup".to_string())]
                .into_iter()
                .collect(),
            ..ViewWeapon::default()
        }
    }

    /// Every field survives the trip through the text, and the text
    /// survives sitting inside a stateful-billboard manifest whose own
    /// lines it must not disturb.
    #[test]
    fn round_trips_inside_a_billboard_manifest() {
        let def = shotgun();
        let text = format!(
            "stateful-billboard 2\nprefix shtg\nrole weapon\nstate ready 0 1 1 6\nframe 0 A 0 79 60 shtg.png cell 0\n{}",
            def.to_text()
        );
        let back = ViewWeapon::parse(&text).expect("view lines present");
        assert_eq!(back, def);
        let bb = crate::stateful_billboard::StatefulBillboard::parse(&text).unwrap();
        assert_eq!(bb.prefix, "shtg");
        assert_eq!(bb.frames.len(), 1);
    }

    #[test]
    fn a_manifest_without_view_lines_is_none() {
        assert!(ViewWeapon::parse("stateful-billboard 2\nprefix shtg\nframe 0 A 0 1 1 x.png\n").is_none());
    }

    #[test]
    fn steps_carry_flash_and_sound_flags() {
        let text = "view v=1 key=k\nview-clip fire loop=0 A:3,flash=rand,sound=sfx/d/x B:20\n";
        let def = ViewWeapon::parse(text).unwrap();
        let fire = def.clip("fire").unwrap();
        assert_eq!(fire.steps[0].flash, Some(FlashStart::Random));
        assert_eq!(fire.steps[0].sound.as_deref(), Some("sfx/d/x"));
        assert_eq!(fire.steps[1].tics, 20);
        assert!(def.fire_has_sound());
        assert_eq!(def.referenced_keys(), vec!["sfx/d/x".to_string()]);
    }

    #[test]
    fn keys_resolve_against_the_fetched_alias() {
        let def = shotgun();
        assert_eq!(
            def.resolve_key("doom/doom/billboards/doom1/shtg", "sfx/doom1/dsshotgn"),
            "doom/doom/sfx/doom1/dsshotgn"
        );
        // No own key: the billboards folder is the anchor.
        let anon = ViewWeapon::default();
        assert_eq!(
            anon.resolve_key("freedoom/freedoom/billboards/freedoom1/shtg", "billboards/freedoom1/shtf"),
            "freedoom/freedoom/billboards/freedoom1/shtf"
        );
    }
}
