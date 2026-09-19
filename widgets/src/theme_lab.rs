//! The theme lab: several themes mixed by weight, with the part a panel
//! would otherwise have to remember kept in one place.
//!
//! [`crate::theme_tokens`] already does the mixing. What it leaves to its
//! caller is everything around the mix: which theme was in force before the
//! lab was opened, which appearance group is on show, where the weights are,
//! which theme the mix started from, whether the answer has moved since it
//! was last installed, and the order the install has to run in to be seen at
//! all. Every one of those is a thing a panel would otherwise keep for
//! itself, and most of them are things a panel would get wrong.
//!
//! So a panel keeps one [`ThemeLab`] and reads as a sentence: make one,
//! [`ThemeLab::enter`], move a weight, [`ThemeLab::apply`],
//! [`ThemeLab::leave`]. It never names a cache, a blend, or a script.
//!
//! What a panel owes the lab in return is a word whenever something else
//! rebuilds the module underneath it -- a style reload, a live edit, a base
//! theme switched from elsewhere in the app. None of that is visible from
//! here, and without the word the lab goes on believing the mix it last
//! installed is still on the screen. That word is [`ThemeLab::invalidate`].
//!
//! # What the lab will not let a panel do
//!
//! Mix a dark theme with a light one. Half way between the two is a mid grey
//! page with mid grey text on it and nothing reads; the engine refuses such a
//! mix outright, so the lab shows one group at a time and the weights of the
//! other group do not exist while it does. See [`Appearance`].
//!
//! Normalise the weights itself. [`BlendCache::blend`] divides by the total
//! whatever the numbers are, so the only thing a mode changes is what the
//! OTHER weights do when one of them moves, and that is [`set_weight`]'s job
//! rather than the panel's.
//!
//! Install a blend token by token. A theme is one script that derives one
//! object, evaluated once; writing tokens one at a time mutates the object
//! every widget was built from, and leaves the panel re-scanning every draw
//! slot after every draw for as long as the app runs.

use crate::desktop_style::{self, DesktopStyle, StyleSheet};
use crate::makepad_platform::{ScriptMod, ScriptVm, ScriptVmCx};
use crate::theme_tokens::{
    random_weights, reads_on, set_weight, to_relative, Scheme, ThemeBlend, LEGIBLE, READABLE,
};
use crate::BaseTheme;

/// Everything the lab's own signatures name, so that a panel has one module to
/// import from and never reaches past the lab for a type the lab handed it.
/// [`BlendCache`] is here for the sake of a caller that cannot afford fifteen
/// real theme resolutions -- see [`ThemeLab::with_cache`] -- and not because a
/// panel has any business holding one.
pub use crate::theme_tokens::{
    Appearance, BlendCache, BlendError, BlendTheme, WeightMode, RELATIVE_TOTAL,
};

/// The key the mix is filed under in `mod.themes`. One name, re-used on every
/// apply, so a session leaves one derived object behind rather than one per
/// slider move.
const MIX_NAME: &str = "equalized";

/// How near two weights have to be to count as the same one. The relative
/// mode settles its total by moving the drift onto the largest weight, so an
/// untouched slider row can come back a few ulps from where it was, and a
/// dirty check on exact equality would reinstall the same theme every frame.
const SAME_WEIGHT: f64 = 1e-9;

/// The ground and ink pairs a mix is measured on, with the bar each is held
/// to. The surface ladder against the body ink and the four meanings against
/// their own ink are the pairs that carry running text, so they answer to
/// `READABLE`; the second voice carries menu labels, list detail and icons,
/// so it answers to `LEGIBLE`. These are the same pairs the library holds its
/// own themes and sheets to, which is what makes the number comparable: a mix
/// that passes here is as readable as a shipped theme, and no more.
const PAIRS: &[(&str, &str, f64)] = &[
    ("color_surface", "color_on_surface", READABLE),
    ("color_surface_container", "color_on_surface", READABLE),
    ("color_surface_container_low", "color_on_surface", READABLE),
    ("color_surface_container_high", "color_on_surface", READABLE),
    ("color_surface_container_highest", "color_on_surface", READABLE),
    ("color_surface_dim", "color_on_surface", READABLE),
    ("color_surface_bright", "color_on_surface", READABLE),
    ("color_surface", "color_on_surface_variant", LEGIBLE),
    ("color_surface_container", "color_on_surface_variant", LEGIBLE),
    ("color_surface_container_high", "color_on_surface_variant", LEGIBLE),
    ("color_surface_container_highest", "color_on_surface_variant", LEGIBLE),
    ("color_success", "color_on_success", READABLE),
    ("color_warning", "color_on_warning", READABLE),
    ("color_error", "color_on_error", READABLE),
    ("color_info", "color_on_info", READABLE),
    ("color_primary", "color_on_primary", READABLE),
];

/// One theme's control: what to call it, and where its weight is.
///
/// The index of the row IS the theme's name as far as the lab is concerned --
/// every call that moves a weight takes one -- so a panel needs nothing off a
/// row but these two. A caller that knows which theme it wants rather than
/// which row asks [`ThemeLab::index_of`] for the index and moves that: the
/// theme stays off the row, so a panel reading a row still cannot start
/// mixing on its own.
#[derive(Clone, Debug, PartialEq)]
pub struct LabRow {
    /// The display name, as the theme itself gives it: `Dark`, `macOS dark`,
    /// `Windows 2000`.
    pub label: String,
    /// Its weight, nought to `RELATIVE_TOTAL`.
    pub weight: f64,
}

/// How the mix in force reads.
///
/// Two readable themes can blend into an unreadable one: each chose its ink
/// against its own ground, and averaging moves the ink and the ground both,
/// with no promise that they move apart. The lab measures and says so; what
/// to do about it -- warn, refuse, or ship it anyway -- is the panel's call,
/// because it is the panel that knows whether somebody is watching.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Readability {
    /// How many pairs were measured. Nought means the mix could not be made
    /// at all, and nothing else here says anything.
    pub measured: usize,
    /// Every pair that falls short of the bar its kind is held to, as lines
    /// fit to show, worst margin first. A panel with room for one line shows
    /// the first, and the order is what makes that line worth the room: a
    /// near miss on the surface the table happens to list first is not the
    /// news when a second voice further down is at half the contrast it
    /// needs.
    pub failures: Vec<String>,
    /// How far the closest pair stands above its bar, negative when it is
    /// under. Nought when nothing was measured.
    pub margin: f64,
    /// Which pair that was: the worst pair measured, whether or not it fails.
    /// Where anything fails this is the same line as `failures[0]`, so a panel
    /// may show it either way and never has to ask which case it is in.
    pub tightest: String,
}

impl Readability {
    /// Whether every pair measured reaches its bar. A mix that could not be
    /// made measured nothing, and does not hold: there is no theme to ship.
    pub fn holds(&self) -> bool {
        self.measured > 0 && self.failures.is_empty()
    }
}

/// What an [`ThemeLab::apply`] did, so that a caller can see its own cost.
///
/// An install is a module rebuild -- about 37 ms, two frames and a half -- and
/// the whole expense of the lab is in the calls that answer something other
/// than `Nothing`. A panel that applies on every drawn frame of a drag can
/// read that here, or in [`ThemeLab::rebuilds`], rather than in a profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Nothing was done: the mix on show is the one already in force, or the
    /// lab is not open. Free, and the common case on a drag that has settled.
    Nothing,
    /// A mix went in, at the cost of a rebuild.
    Mix,
    /// The mix came off and the theme the lab was entered on went back, at the
    /// cost of a rebuild.
    Entry,
}

impl Applied {
    /// Whether the call rebuilt the module, which is the thing a panel
    /// deciding when to apply is really asking about.
    pub fn rebuilt(self) -> bool {
        self != Applied::Nothing
    }
}

/// What was in force when the lab was entered, so that leaving can put it
/// back. The base theme and the sheet are two separate choices -- a sheet is
/// laid over a base rather than replacing it -- so both are taken, and the
/// theme the pair amounts to is worked out once here rather than at every
/// call that needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry {
    base: BaseTheme,
    sheet: Option<(DesktopStyle, bool)>,
    theme: BlendTheme,
}

/// A mix of the themes the library ships, and the panel's side of it.
#[derive(Clone, Debug)]
pub struct ThemeLab {
    /// Every theme resolved into its tokens. Kept across a leave: filling it
    /// is the expensive part of the whole lab and re-entering should be
    /// instant.
    cache: BlendCache,
    /// What to put back on the way out, and `None` when the lab is not open.
    entry: Option<Entry>,
    /// The group on show. A mix never crosses the two.
    appearance: Appearance,
    /// One per theme of `appearance`, in the group's own order.
    rows: Vec<LabRow>,
    /// The row the mix starts from, which relative mode takes from first: the
    /// entry theme where the group on show holds it, and otherwise the plain
    /// theme of the group, which is the first row of both groups.
    anchor: usize,
    mode: WeightMode,
    /// The weights the last apply installed, so that a second apply over an
    /// untouched mix is nothing at all. `None` before the lab is entered, and
    /// again after [`ThemeLab::invalidate`].
    applied: Option<(Appearance, Vec<f64>)>,
    /// The blend those weights came to, kept from the apply that installed it.
    /// A panel reads a mix twice on the frame it goes in -- once to install it
    /// and once to measure it -- and the second read is the same arithmetic
    /// over the same numbers. Only ever handed out while `applied` still
    /// matches the rows, which is what [`ThemeLab::is_dirty`] answers.
    blended: Option<ThemeBlend>,
    /// Whether a mix is in force right now, as opposed to the entry theme.
    /// Leaving an untouched lab must not reinstall anything.
    installed: bool,
    /// How many module rebuilds this lab has driven. Only ever goes up.
    rebuilds: u32,
}

impl Default for ThemeLab {
    /// A lab nobody has entered: no rows, no theme to go back to, and the
    /// dark group ready for the common case. `enter` decides the group for
    /// real, off the theme it finds in force.
    fn default() -> Self {
        Self {
            cache: BlendCache::new(),
            entry: None,
            appearance: Appearance::Dark,
            rows: Vec::new(),
            anchor: 0,
            mode: WeightMode::Absolute,
            applied: None,
            blended: None,
            installed: false,
            rebuilds: 0,
        }
    }
}

impl ThemeLab {
    pub fn new() -> Self {
        Self::default()
    }

    /// A lab whose themes are resolved already.
    ///
    /// [`ThemeLab::enter`] resolves every theme the library ships, which is
    /// sixteen module reloads and the whole cost of the lab. A second lab, or
    /// a test that opens one over and over, is handed the answers instead:
    /// `ThemeLab::with_cache(first.cache().clone())` enters in microseconds. A
    /// theme already in the cache is never resolved again, so a partial set is
    /// no trouble -- what is missing is resolved on the first enter.
    pub fn with_cache(cache: BlendCache) -> Self {
        Self { cache, ..Self::default() }
    }

    /// The resolved themes, to hand to another lab.
    pub fn cache(&self) -> &BlendCache {
        &self.cache
    }

    /// Whether the lab is open: [`ThemeLab::enter`] has been called and
    /// [`ThemeLab::leave`] has not.
    ///
    /// A closed lab has no rows, and every call that moves a weight is a no-op
    /// over no rows: `set_weight` and `clear_weight` find no index, `reset` and
    /// `randomize` have nothing to write, `index_of` has no row to point at,
    /// `apply` answers [`Applied::Nothing`] and `readability` measures nothing.
    /// That silence is deliberate -- a panel draws the folded section before it
    /// ever opens it, and should not have to guard each call -- but it also
    /// means a test that forgets to enter asserts nothing whatever it calls, so
    /// a test asks this first.
    pub fn is_open(&self) -> bool {
        self.entry.is_some()
    }

    /// How many module rebuilds this lab has driven since it was made.
    ///
    /// The lab has no frame clock and cannot police the settle
    /// [`ThemeLab::apply`] asks of its caller; what it can do is count, so
    /// that the cost of a drag is a number a test can hold rather than a shape
    /// in a profile. A drag that settles is one. One apply per drawn frame is
    /// forty, and the assertion that says so belongs in the panel's own tests.
    pub fn rebuilds(&self) -> u32 {
        self.rebuilds
    }

    /// Remember the theme in force and make a mix of it.
    ///
    /// This is where the whole cost of the lab is: every theme the library
    /// ships is resolved into its tokens, and resolving one means installing
    /// or taking off its sheet and rebuilding the module, so fifteen themes
    /// is sixteen module reloads -- about 0.6 s in release and five times
    /// that in a debug build. It happens once, here, on the click that opens
    /// the section; a second enter over a warm cache resolves nothing and
    /// costs microseconds. Never from a draw pass, and never from a drag.
    ///
    /// Entering twice without leaving does nothing: the second call would
    /// otherwise capture the mix as the theme to go back to, and leaving
    /// would never return anywhere.
    ///
    /// The panel calls this inside `cx.with_vm`.
    pub fn enter(&mut self, vm: &mut ScriptVm) {
        if self.entry.is_some() {
            return;
        }
        let base = crate::base_theme(vm.cx_mut());
        let sheet = desktop_style::current_name(vm).and_then(|name| {
            DesktopStyle::parse(&name).map(|style| (style, name.ends_with("-dark")))
        });
        let theme = match sheet {
            Some((style, dark)) => BlendTheme::Sheet(style, dark),
            None => BlendTheme::Base(match base {
                BaseTheme::Dark => Scheme::Dark,
                BaseTheme::Light => Scheme::Light,
                BaseTheme::Skeleton => Scheme::Skeleton,
            }),
        };
        let before = self.cache.len();
        self.cache.fill(vm, &BlendTheme::all());
        if self.cache.len() != before {
            // Filling rebuilt the module several times over and left a fresh
            // one behind, so the tree has to be walked over it again.
            vm.cx_mut().request_script_reapply();
        }
        self.entry = Some(Entry { base, sheet, theme });
        self.installed = false;
        self.show(theme.appearance());
        self.applied = Some((self.appearance, self.weights()));
    }

    /// The group on show.
    pub fn appearance(&self) -> Appearance {
        self.appearance
    }

    /// Show the other group. The weights of the group being left are not kept:
    /// they weigh themes that cannot be in this mix, and a number a panel
    /// cannot see is a number nobody chose.
    pub fn set_appearance(&mut self, appearance: Appearance) {
        if appearance == self.appearance {
            return;
        }
        self.show(appearance);
    }

    /// One row per theme of the group on show.
    pub fn rows(&self) -> &[LabRow] {
        &self.rows
    }

    /// Where a theme's weight is, for a caller that knows the theme it wants
    /// and not the row it landed on.
    ///
    /// `None` where that theme is not among the rows on show, which is the
    /// honest answer to how much Light there is in a dark mix: none, and there
    /// is no slider to move. The alternative is a caller matching on a display
    /// name, which a theme is free to change, or building the group itself,
    /// which is the mixing the lab was drawn to keep out of a panel.
    pub fn index_of(&self, theme: BlendTheme) -> Option<usize> {
        BlendTheme::group(self.appearance)
            .iter()
            .position(|other| *other == theme)
            .filter(|index| *index < self.rows.len())
    }

    /// Move one weight, and settle the rest by the rule of the mode.
    ///
    /// In [`WeightMode::Absolute`] that is nothing: the others stay where they
    /// are and only their share of the total changes. In
    /// [`WeightMode::Relative`] the total is held at `RELATIVE_TOTAL`, so
    /// raising this one takes from the theme the mix started from and then,
    /// once that is spent, from the rest in proportion to what each still
    /// holds.
    ///
    /// An index past the last row is ignored, and a closed lab has no rows at
    /// all, so this is silent before [`ThemeLab::enter`]. See
    /// [`ThemeLab::is_open`].
    pub fn set_weight(&mut self, index: usize, weight: f64) {
        let mut weights = self.weights();
        set_weight(self.mode, &mut weights, self.anchor, index, weight);
        self.take_weights(weights);
    }

    /// Take a theme out of the mix. The gesture is clicking its name, which is
    /// why it is a call of its own and not a set to nought: the panel should
    /// not have to know that they are the same thing.
    pub fn clear_weight(&mut self, index: usize) {
        self.set_weight(index, 0.0);
    }

    pub fn mode(&self) -> WeightMode {
        self.mode
    }

    /// Change what the other weights do when one of them moves.
    ///
    /// Going relative scales what is there to `RELATIVE_TOTAL` first, so the
    /// mix on screen does not change -- only the numbers under it, which now
    /// read as shares. Going absolute leaves them exactly as they are: a set
    /// of weights adding to a hundred is a perfectly good absolute mix, and
    /// re-scaling it would move a theme nobody touched.
    pub fn set_mode(&mut self, mode: WeightMode) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        if mode == WeightMode::Relative {
            let weights = to_relative(&self.weights(), self.anchor);
            self.take_weights(weights);
        }
    }

    /// A mix off a bell curve over a shuffled order, so one theme dominates
    /// and two or three stand behind it. Pure and seeded: the same seed is the
    /// same mix on every machine, which is what lets a panel offer the same
    /// surprise twice and a test pin one. A closed lab draws no weights,
    /// having nothing to draw them for.
    pub fn randomize(&mut self, seed: u64) {
        let drawn = random_weights(seed, self.rows.len());
        // The draw sums to one, and both modes want a slider's worth of
        // number; relative mode additionally requires the total, so scaling
        // to it is the one answer that serves both.
        let weights = to_relative(&drawn, self.anchor);
        self.take_weights(weights);
    }

    /// The entry theme at `RELATIVE_TOTAL` and everything else at nought.
    pub fn reset(&mut self) {
        let anchor = self.anchor;
        for (index, row) in self.rows.iter_mut().enumerate() {
            row.weight = if index == anchor { RELATIVE_TOTAL } else { 0.0 };
        }
    }

    /// Whether [`ThemeLab::apply`] would change anything.
    ///
    /// The weights against the weights that went in, and nothing else: the lab
    /// holds no handle on the module it installed them into. Something else
    /// rebuilding that module leaves this answering `false` over a mix that is
    /// no longer on the screen, which is what [`ThemeLab::invalidate`] is for.
    pub fn is_dirty(&self) -> bool {
        let Some((appearance, weights)) = &self.applied else {
            return true;
        };
        *appearance != self.appearance
            || weights.len() != self.rows.len()
            || weights
                .iter()
                .zip(self.rows.iter())
                .any(|(was, row)| (was - row.weight).abs() >= SAME_WEIGHT)
    }

    /// Say that the module was rebuilt underneath the lab, so that the next
    /// [`ThemeLab::apply`] puts the mix back.
    ///
    /// Every caller of `cx.request_style_reload()` or `cx.request_live_edit()`
    /// owes the lab this call, and so does anything else that re-runs
    /// `script_mod`: switching the base theme, putting a style sheet on or
    /// taking one off, a live edit landing from a file watcher. None of it is
    /// visible from here -- the lab holds the weights it installed, not the
    /// module it installed them into -- so a rebuild it was not told about
    /// leaves [`ThemeLab::is_dirty`] answering `false` over an app that has
    /// snapped back to its bare base theme, with the panel still showing the
    /// mix at its weights and no draw that will ever put it back. The button
    /// that reloads the style sits thirty pixels above the mix; this is not a
    /// corner.
    ///
    /// What is dropped is the memory of what was installed, which makes the
    /// lab dirty again. What is NOT dropped is that the lab still owes the
    /// entry theme back: a rebuild puts up the base theme and whatever sheet
    /// is installed, and installing a mix took the entry sheet off, so after
    /// one the app wears neither the mix nor what the lab found -- and
    /// [`ThemeLab::leave`] is the only thing that can put the second one on.
    /// Clearing that debt here would leave somebody in a theme nobody chose
    /// with no way back but to find their sheet by hand.
    ///
    /// Cheap, and safe where nothing happened: the worst it costs is one
    /// rebuild that was not strictly needed, on the next apply.
    pub fn invalidate(&mut self) {
        self.applied = None;
        self.blended = None;
    }

    /// Put the mix in force, and say what that took.
    ///
    /// Safe to call on any frame in the sense that an untouched mix is a
    /// return and nothing else. When the mix HAS moved this costs a module
    /// rebuild -- about 37 ms, two frames and a half -- so a panel owes it a
    /// settle: apply on the control's own end-of-drag action, or on a timeout
    /// re-armed by every edit. Applying once per drawn frame of a 500 ms drag
    /// is not a slower drag, it is 480 ms of blocked main thread. The lab has
    /// no frame clock and cannot make the settle happen; what it can do is
    /// hand back what it did, so that a caller can count its own rebuilds --
    /// see [`Applied`] and [`ThemeLab::rebuilds`]. What a panel must not do is
    /// reach for a style reload per slider move; this call is the whole
    /// install, and a style reload throws it away (see
    /// [`ThemeLab::invalidate`]).
    ///
    /// One blend, one script, one evaluation, one re-apply. The script is
    /// evaluated BETWEEN the theme module and the widget module, because a
    /// widget template bakes `theme.color_x` into a literal when its own
    /// module block runs and a re-apply does not evaluate expressions again:
    /// a mix installed after the widgets have been built moves `mod.theme`
    /// and not one thing drawn from it. The trailing `true` is there because
    /// the last statement of an evaluated script is swallowed, and the last
    /// statement of this one is what makes the mix current.
    ///
    /// The sheet comes off. A sheet's own script re-points `mod.theme` back
    /// at its base and mutates it, from the first line of the widget module,
    /// which is after the mix has been evaluated and therefore over the top of
    /// it. The mix already carries every token the sheet set, by weight; what
    /// is given up is the sheet's widget re-skins and its font fallbacks.
    ///
    /// The roles are not derived again here. [`BlendCache::blend`] has already
    /// done it over the blended tokens, growing the brand families from what
    /// the mix carries rather than from the accent; running the sheet's
    /// version over a mix would regrow those families from the focus blue the
    /// base themes use for a selected control, and turn a mix of two orange
    /// themes blue.
    ///
    /// The panel calls this inside `cx.with_vm`, and shows the error: a mix
    /// that refused itself in the log is a mix that failed silently.
    pub fn apply(&mut self, vm: &mut ScriptVm) -> Result<Applied, BlendError> {
        let Some(entry) = self.entry else {
            return Ok(Applied::Nothing);
        };
        if !self.is_dirty() {
            return Ok(Applied::Nothing);
        }
        let mut did = Applied::Nothing;
        let mut blended = None;
        if self.is_entry_mix() {
            // Back where the lab started. The entry theme itself goes on,
            // rather than a blend that carries its tokens, so the sheet's
            // re-skins and font fallbacks come back with it -- and where no
            // mix was ever installed there is nothing to put back, so the
            // weights having wandered and returned costs a tree walk of
            // nothing.
            if self.installed {
                self.install_entry(vm, entry);
                self.installed = false;
                vm.cx_mut().request_script_reapply();
                did = Applied::Entry;
            }
        } else {
            let blend = self.cache.blend(&self.mix())?;
            self.install_blend(vm, &blend);
            self.installed = true;
            vm.cx_mut().request_script_reapply();
            blended = Some(blend);
            did = Applied::Mix;
        }
        self.applied = Some((self.appearance, self.weights()));
        // Only the install path leaves a blend to measure; the entry theme
        // went on as itself and was never blended into anything.
        self.blended = blended;
        Ok(did)
    }

    /// Put back the theme [`ThemeLab::enter`] found, and close the lab.
    ///
    /// A lab that never installed anything restores nothing: there is nothing
    /// to undo, and a rebuild for the sake of it is 37 ms of nothing.
    ///
    /// The panel calls this inside `cx.with_vm`.
    pub fn leave(&mut self, vm: &mut ScriptVm) {
        let Some(entry) = self.entry.take() else {
            return;
        };
        if self.installed {
            self.install_entry(vm, entry);
            vm.cx_mut().request_script_reapply();
        }
        self.installed = false;
        self.rows.clear();
        self.applied = None;
        self.blended = None;
        self.anchor = 0;
    }

    /// How the mix in force reads, pair by pair.
    ///
    /// Free on the frame a mix went in -- the blend the install was made from
    /// is still here -- and under a millisecond otherwise, so this may be
    /// asked every frame of a drag. It installs nothing.
    pub fn readability(&self) -> Readability {
        let mut out = Readability::default();
        let fresh;
        let blend = match &self.blended {
            // The resolved themes cannot change while the lab is open --
            // `enter` is the only thing that fills them, and entering twice
            // does nothing -- so the weights not having moved is the whole of
            // what keeps the blend from the last apply true.
            Some(blend) if !self.is_dirty() => blend,
            _ => match self.cache.blend(&self.mix()) {
                Ok(blend) => {
                    fresh = blend;
                    &fresh
                }
                Err(_) => return out,
            },
        };
        let mut margin: Option<f64> = None;
        let mut failures: Vec<(f64, String)> = Vec::new();
        for (ground, ink, need) in PAIRS {
            let (Some(g), Some(i)) = (blend.color(ground), blend.color(ink)) else {
                continue;
            };
            // The ink is laid over its ground before it is measured: most of
            // a theme's text carries an alpha, and white at 65 percent on a
            // mid grey is not white.
            let stands = reads_on(g | 0xFF, i);
            let line = format!("{ink} on {ground} = {stands:.2}, wants {need}");
            out.measured += 1;
            if margin.is_none_or(|best| stands - need < best) {
                margin = Some(stands - need);
                out.tightest = line.clone();
            }
            if stands < *need {
                failures.push((stands - need, line));
            }
        }
        // Worst first, so that a panel with room for one line shows the pair
        // that is furthest under rather than the pair the table names first.
        // Stable, so that equally bad pairs keep the table's order and the
        // line does not change under a redraw.
        failures.sort_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        out.failures = failures.into_iter().map(|(_, line)| line).collect();
        out.margin = margin.unwrap_or(0.0);
        out
    }

    /// Draw the group's rows and start the mix on its one theme.
    fn show(&mut self, appearance: Appearance) {
        let group = BlendTheme::group(appearance);
        self.anchor = self
            .entry
            .and_then(|entry| group.iter().position(|theme| *theme == entry.theme))
            .unwrap_or(0);
        self.appearance = appearance;
        self.rows =
            group.into_iter().map(|theme| LabRow { label: theme.label(), weight: 0.0 }).collect();
        self.reset();
    }

    /// The mix the rows on show amount to. The theme each row stands for is
    /// held by position rather than on the row, so that a panel reading a row
    /// cannot start mixing on its own.
    fn mix(&self) -> Vec<(BlendTheme, f64)> {
        BlendTheme::group(self.appearance)
            .into_iter()
            .zip(self.rows.iter().map(|row| row.weight))
            .collect()
    }

    fn weights(&self) -> Vec<f64> {
        self.rows.iter().map(|row| row.weight).collect()
    }

    fn take_weights(&mut self, weights: Vec<f64>) {
        for (row, weight) in self.rows.iter_mut().zip(weights) {
            row.weight = weight;
        }
    }

    /// Whether the mix on show is just the theme the lab was entered on.
    fn is_entry_mix(&self) -> bool {
        let Some(entry) = self.entry else {
            return false;
        };
        if entry.theme.appearance() != self.appearance {
            return false;
        }
        let group = BlendTheme::group(self.appearance);
        self.rows.iter().enumerate().all(|(index, row)| {
            let want = if group[index] == entry.theme { RELATIVE_TOTAL } else { 0.0 };
            (row.weight - want).abs() < SAME_WEIGHT
        })
    }

    /// The module, rebuilt with the mix evaluated at the one point it is seen
    /// from: after the themes exist and before the widgets are baked off them.
    /// This is the library's own module run with a single line spliced into
    /// the seam.
    fn install_blend(&mut self, vm: &mut ScriptVm, blend: &ThemeBlend) {
        self.rebuilds = self.rebuilds.saturating_add(1);
        desktop_style::uninstall(vm);
        let code = format!("{}true\n", blend.script(MIX_NAME));
        vm.with_reload(|vm| {
            crate::theme_mod(vm);
            vm.eval(ScriptMod {
                cargo_manifest_path: env!("CARGO_MANIFEST_DIR").into(),
                module_path: "theme_lab".to_string(),
                file: format!("{MIX_NAME}.splash"),
                line: 0,
                column: 0,
                code,
                values: vec![],
            });
            crate::widgets_mod(vm);
            desktop_style::apply_widgets(vm);
        });
    }

    /// The base theme and the sheet the lab was entered on, back on, and the
    /// module rebuilt off the pair of them.
    fn install_entry(&mut self, vm: &mut ScriptVm, entry: Entry) {
        self.rebuilds = self.rebuilds.saturating_add(1);
        crate::set_base_theme(vm.cx_mut(), entry.base);
        match entry.sheet {
            Some((style, dark)) => {
                desktop_style::install(vm, StyleSheet::load_with_appearance(style, dark))
            }
            None => desktop_style::uninstall(vm),
        }
        vm.with_reload(crate::script_mod);
    }
}

#[cfg(test)]
mod theme_lab_tests {
    use super::*;
    use crate::makepad_platform::{Cx, LiveId, NoTrap};
    use crate::theme_tokens::{mix_rgb, BlendValue, ThemeValues};
    use std::collections::BTreeMap;

    /// A theme invented for the maths: enough tokens to blend and to measure,
    /// and no VM, so the lab's own arithmetic can be checked at speed.
    fn made_up(theme: BlendTheme, bg: u32, ink: u32, radius: f64) -> ThemeValues {
        let mut values = BTreeMap::new();
        for (key, rgba) in [
            ("color_bg_app", bg),
            ("color_surface", bg),
            ("color_surface_container", bg),
            ("color_on_surface", ink),
            ("color_on_surface_variant", ink),
            ("color_primary", 0xFF5C39FF),
        ] {
            values.insert(key.to_string(), BlendValue::Color(rgba));
        }
        values.insert("radius_m".to_string(), BlendValue::Num(radius));
        ThemeValues::new(theme, values)
    }

    /// Every theme the library ships, made up, so that `enter` resolves none
    /// of them and a test that is about the lab does not pay for a reload per
    /// theme. The values differ by theme so a mix of two is visibly a mix.
    fn bench() -> BlendCache {
        let mut cache = BlendCache::new();
        for (step, theme) in BlendTheme::all().into_iter().enumerate() {
            let shade = (step as u32) * 0x10;
            let (bg, ink) = match theme.appearance() {
                Appearance::Dark => (0x101010FF + (shade << 24) + (shade << 16) + (shade << 8), 0xEEEEEEFF),
                Appearance::Light => (0xF0F0F0FF - (shade << 24) - (shade << 16) - (shade << 8), 0x101010FF),
            };
            cache.insert(made_up(theme, bg, ink, 2.0 + step as f64));
        }
        cache
    }

    /// A lab whose themes are already resolved, entered on a bare dark theme.
    fn entered() -> ThemeLab {
        let mut lab = ThemeLab::new();
        lab.cache = bench();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            desktop_style::uninstall(vm);
            lab.enter(vm);
        });
        lab
    }

    /// The two themes these tests mix, by identity rather than by display
    /// name: what a caller outside the crate would have to name, and what
    /// `index_of` is for.
    const DARK: BlendTheme = BlendTheme::Base(Scheme::Dark);
    const OMARCHY: BlendTheme = BlendTheme::Sheet(DesktopStyle::Omarchy, false);

    /// A theme whose inks do not stand off its grounds, with the misses in a
    /// deliberate order: the pair `PAIRS` names first is a near miss at 4.37
    /// against a bar of 4.5, and the worst of the four is the eleventh pair in
    /// the table, at 1.40 against a bar of 3. Neither ground is derived again
    /// by a blend -- both are roles a blend leaves alone where the theme
    /// already carries them, and the ink derivation wants a `color_fg_app`
    /// this theme does not have -- so these numbers reach `readability`
    /// exactly as they are written here.
    fn unreadable(theme: BlendTheme) -> ThemeValues {
        let mut values = BTreeMap::new();
        for (key, rgba) in [
            ("color_surface", 0x000000FFu32),
            ("color_on_surface", 0x727272FF),
            ("color_surface_container_highest", 0x1A1A1AFF),
            ("color_on_surface_variant", 0x343434FF),
        ] {
            values.insert(key.to_string(), BlendValue::Color(rgba));
        }
        ThemeValues::new(theme, values)
    }

    /// The two groups are the whole library between them, with nothing in
    /// both and nothing in neither. A theme added to the library and not
    /// classified would arrive here as a row the lab never offers, which is a
    /// theme nobody can reach rather than an error anybody sees.
    #[test]
    fn every_theme_the_library_ships_is_on_exactly_one_group() {
        let mut seen: Vec<String> = Vec::new();
        for appearance in Appearance::ALL {
            for theme in BlendTheme::group(appearance) {
                assert_eq!(theme.appearance(), appearance, "{} is on the wrong group", theme.name());
                assert!(!seen.contains(&theme.name()), "{} is on both groups", theme.name());
                seen.push(theme.name());
            }
        }
        let mut all: Vec<String> = BlendTheme::all().iter().map(|t| t.name()).collect();
        all.sort();
        seen.sort();
        assert_eq!(seen, all, "a theme the library ships is on neither group");
        // And the lab draws its rows from those groups, one per theme.
        let mut lab = entered();
        for appearance in Appearance::ALL {
            lab.set_appearance(appearance);
            assert_eq!(lab.rows().len(), BlendTheme::group(appearance).len());
        }
    }

    /// One slider at the top with every other at nought is not a mix of
    /// anything. It has to be the theme itself -- and where that theme is the
    /// one the lab was entered on, there is nothing to install at all: the
    /// app is already wearing it.
    #[test]
    fn one_theme_at_full_weight_is_that_theme_and_nothing_to_do() {
        let lab = entered();
        assert!(!lab.is_dirty(), "entering is not a change to anything");
        let dark = lab.index_of(DARK).unwrap();
        assert_eq!(lab.rows()[dark].weight, RELATIVE_TOTAL);
        assert!(lab.rows().iter().enumerate().all(|(i, r)| i == dark || r.weight == 0.0));
        // Applying an untouched lab touches neither the VM nor the state.
        let mut lab = lab;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        assert_eq!(
            cx.with_vm(|vm| lab.apply(vm)).unwrap(),
            Applied::Nothing,
            "the entry theme is already on; there was nothing to do"
        );
        assert!(!lab.installed, "nothing was installed over the entry theme");
        assert_eq!(lab.rebuilds(), 0, "and nothing was rebuilt to do it");
        // And the mix that weight stands for is that theme, token for token.
        let values = lab.cache.get(DARK).unwrap().clone();
        let blend = lab.cache.blend(&[(DARK, RELATIVE_TOTAL)]).unwrap();
        for key in values.keys() {
            let want = values.get(key).unwrap();
            let got = match (blend.color(key), blend.num(key)) {
                (Some(c), _) => BlendValue::Color(c),
                (_, Some(n)) => BlendValue::Num(n),
                _ => panic!("{key} is missing from the mix"),
            };
            assert_eq!(got, want, "{key}");
        }
    }

    /// Half of one and half of the other is the midpoint of every channel,
    /// whichever mode the weights are being read in: the mode says what the
    /// OTHER sliders do when one moves, and nothing about the answer.
    #[test]
    fn half_of_one_and_half_of_the_other_is_the_midpoint() {
        for mode in [WeightMode::Absolute, WeightMode::Relative] {
            let mut lab = entered();
            lab.set_mode(mode);
            let dark = lab.index_of(DARK).unwrap();
            let omarchy = lab.index_of(OMARCHY).unwrap();
            lab.set_weight(omarchy, 50.0);
            lab.set_weight(dark, 50.0);
            assert_eq!(lab.rows()[dark].weight, 50.0, "{mode:?}");
            assert_eq!(lab.rows()[omarchy].weight, 50.0, "{mode:?}");
            let mix = [(DARK, 50.0), (OMARCHY, 50.0)];
            let blend = lab.cache.blend(&mix).unwrap();
            let ends: Vec<u32> = mix
                .iter()
                .map(|(theme, _)| lab.cache.get(*theme).unwrap().color("color_bg_app").unwrap())
                .collect();
            assert_eq!(blend.color("color_bg_app"), Some(mix_rgb(ends[0], ends[1], 0.5)), "{mode:?}");
            assert_eq!(blend.num("radius_m"), Some(3.5), "{mode:?}");
        }
    }

    /// A relative mix is a hundred parts shared out, so every single move has
    /// to leave a hundred parts -- including the move that spends the theme
    /// the mix started from, after which there is nothing left to take from
    /// but the other sliders, in proportion to what each still holds.
    #[test]
    fn a_relative_mix_still_adds_to_a_hundred_after_any_one_move() {
        let mut lab = entered();
        lab.set_mode(WeightMode::Relative);
        let total = |lab: &ThemeLab| lab.rows().iter().map(|r| r.weight).sum::<f64>();
        let anchor = lab.anchor;
        // Spend the anchor outright, then keep asking for more.
        lab.set_weight((anchor + 1) % lab.rows().len(), RELATIVE_TOTAL);
        assert!((total(&lab) - RELATIVE_TOTAL).abs() < 1e-9);
        assert_eq!(lab.rows()[anchor].weight, 0.0, "the anchor is spent first");
        for step in 0..200u64 {
            let index = (step as usize * 7) % lab.rows().len();
            let value = ((step * 37) % 130) as f64;
            lab.set_weight(index, value);
            let sum = total(&lab);
            assert!((sum - RELATIVE_TOTAL).abs() < 1e-9, "step {step} left {sum}");
            assert!(lab.rows().iter().all(|r| r.weight >= 0.0), "step {step} went negative");
        }
    }

    /// The same seed is the same mix, here and on any other machine, which is
    /// what lets somebody keep a mix they liked by keeping the number that
    /// made it. And the weights land where a slider row can show them.
    #[test]
    fn the_same_seed_is_the_same_mix() {
        for mode in [WeightMode::Absolute, WeightMode::Relative] {
            let mut first = entered();
            let mut second = entered();
            first.set_mode(mode);
            second.set_mode(mode);
            first.randomize(0x5EED);
            second.randomize(0x5EED);
            assert_eq!(first.rows(), second.rows(), "{mode:?}");
            let sum: f64 = first.rows().iter().map(|r| r.weight).sum();
            assert!((sum - RELATIVE_TOTAL).abs() < 1e-9, "{mode:?} summed to {sum}");
            assert!(first.rows().iter().all(|r| r.weight > 0.0), "{mode:?} dropped a theme");
            let mut other = entered();
            other.set_mode(mode);
            other.randomize(0x5EED + 1);
            assert_ne!(other.rows(), first.rows(), "{mode:?} gives one mix whatever the seed");
        }
    }

    /// Clicking the name of the last theme carrying any weight leaves a mix
    /// of nothing. There is nothing to divide by, and the lab has to say so
    /// rather than hand back a colour it made up.
    #[test]
    fn clearing_the_last_weight_is_a_refusal_and_not_a_crash() {
        let mut lab = entered();
        lab.set_mode(WeightMode::Absolute);
        let anchor = lab.anchor;
        lab.clear_weight(anchor);
        assert!(lab.rows().iter().all(|r| r.weight == 0.0));
        assert_eq!(lab.readability().measured, 0, "there is nothing to measure");
        assert!(!lab.readability().holds(), "and nothing that holds");
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let refused = cx.with_vm(|vm| lab.apply(vm));
        assert_eq!(refused, Err(BlendError::NoWeight));
        // A refusal is not an install, so the lab is still owed one.
        assert!(lab.is_dirty(), "a mix that was refused has not been applied");
        // In relative mode the same click hands the weight to the others
        // instead, so the hundred parts are still shared out somewhere.
        let mut lab = entered();
        lab.set_mode(WeightMode::Relative);
        lab.clear_weight(lab.anchor);
        let sum: f64 = lab.rows().iter().map(|r| r.weight).sum();
        assert!((sum - RELATIVE_TOTAL).abs() < 1e-9, "clearing the anchor left {sum}");
    }

    /// A mix is only worth anything if it is seen, and it is only seen if it
    /// is evaluated before the widgets are built off it: a template bakes its
    /// colour when the widget module runs. So this reads the theme the widget
    /// module was handed, not just `mod.theme`, and it reads it after a real
    /// apply. It is also the guard for the swallowed last statement -- without
    /// the trailing `true` the line that makes the mix current never runs, and
    /// the theme stays exactly where it was, silently.
    #[test]
    fn a_mix_reaches_the_module_the_widgets_are_built_from() {
        let mut lab = ThemeLab::new();
        lab.cache = bench();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            vm.bx.captured_errors = Some(Vec::new());
            desktop_style::uninstall(vm);
            lab.enter(vm);
            let omarchy = lab.index_of(OMARCHY).unwrap();
            lab.set_mode(WeightMode::Absolute);
            lab.set_weight(omarchy, RELATIVE_TOTAL);
            assert!(lab.is_dirty());
            assert_eq!(lab.apply(vm).unwrap(), Applied::Mix);
            assert!(!lab.is_dirty(), "an applied mix is not owed another apply");
            // Raising one weight in absolute mode leaves the theme the lab
            // opened on exactly where it was, so what went on is half of
            // each: a colour neither ingredient carries, which is the only
            // kind that proves a mix rather than a switch.
            let mix = [(DARK, RELATIVE_TOTAL), (OMARCHY, RELATIVE_TOTAL)];
            let want = lab.cache.blend(&mix).unwrap().color("color_bg_app");
            for (theme, _) in mix {
                let ingredient = lab.cache.get(theme).unwrap().color("color_bg_app");
                assert_ne!(want, ingredient, "{} is not a mix of anything", theme.name());
            }
            assert_eq!(read_theme(vm, "color_bg_app"), want, "the mix did not become the theme");
            assert_eq!(
                read_widget_theme(vm, "color_bg_app"),
                want,
                "the widgets were built off the theme the mix replaced"
            );
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "{errors:?}");
        });
    }

    /// Leaving puts back both halves of what entering found -- the base theme
    /// and the sheet are separate choices -- and a lab that installed nothing
    /// puts back nothing, because there is nothing to undo.
    #[test]
    fn leaving_puts_back_what_entering_found() {
        let mut lab = ThemeLab::new();
        lab.cache = bench();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            desktop_style::install(vm, StyleSheet::load_with_appearance(DesktopStyle::Omarchy, false));
            crate::set_base_theme(vm.cx_mut(), BaseTheme::Dark);
            lab.enter(vm);
            assert_eq!(lab.entry.unwrap().theme, OMARCHY);
            assert_eq!(lab.appearance(), Appearance::Dark, "a dark sheet opens the dark group");
            // A lab that was only looked at restores nothing.
            let mut untouched = lab.clone();
            untouched.leave(vm);
            assert!(!untouched.installed);
            assert_eq!(desktop_style::current_name(vm).as_deref(), Some("omarchy"));

            let dark = lab.index_of(DARK).unwrap();
            lab.set_weight(dark, RELATIVE_TOTAL);
            assert_eq!(lab.apply(vm).unwrap(), Applied::Mix);
            assert!(lab.installed);
            assert_eq!(desktop_style::current_name(vm), None, "a mix stands on its base, not on a sheet");
            lab.leave(vm);
            assert_eq!(
                desktop_style::current_name(vm).as_deref(),
                Some("omarchy"),
                "leaving did not put the sheet back"
            );
            assert_eq!(crate::base_theme(vm.cx_mut()), BaseTheme::Dark);
            assert!(lab.entry.is_none() && lab.rows().is_empty(), "the lab is closed");
            // The cache survives, so opening it again is free.
            assert_eq!(lab.cache.len(), BlendTheme::all().len());
        });
    }

    /// The app can be rebuilt out from under the lab -- a style reload, a live
    /// edit, a base theme switched elsewhere -- and the lab cannot see it
    /// happen. What it holds is the weights it installed, which still match
    /// the rows, so nothing is dirty and no draw will ever put the mix back:
    /// the app sits on its bare base theme with the panel showing a mix at its
    /// weights. Being told is the only way out, and this is the sequence that
    /// needs it -- the button that reloads the style is thirty pixels above
    /// the section.
    #[test]
    fn a_rebuild_under_the_lab_comes_back_only_once_the_lab_is_told() {
        let mut lab = ThemeLab::new();
        lab.cache = bench();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            desktop_style::uninstall(vm);
            lab.enter(vm);
            let omarchy = lab.index_of(OMARCHY).unwrap();
            lab.set_mode(WeightMode::Absolute);
            lab.set_weight(omarchy, RELATIVE_TOTAL);
            assert_eq!(lab.apply(vm).unwrap(), Applied::Mix);
            let mix = [(DARK, RELATIVE_TOTAL), (OMARCHY, RELATIVE_TOTAL)];
            let want = lab.cache.blend(&mix).unwrap().color("color_bg_app");
            assert_eq!(read_theme(vm, "color_bg_app"), want, "the mix did not go on");

            // What a `request_style_reload` lands on the next tick: the
            // library's own module, built from the base theme, over the top of
            // the mix.
            vm.with_reload(crate::script_mod);
            assert_ne!(read_theme(vm, "color_bg_app"), want, "the reload left the mix standing");
            assert!(!lab.is_dirty(), "the lab cannot see a reload, which is the whole trap");
            assert_eq!(
                lab.apply(vm).unwrap(),
                Applied::Nothing,
                "and will not put the mix back of its own accord"
            );
            assert_ne!(read_theme(vm, "color_bg_app"), want);

            lab.invalidate();
            assert!(lab.is_dirty(), "a lab that has been told is owed an apply");
            assert_eq!(lab.apply(vm).unwrap(), Applied::Mix);
            assert_eq!(read_theme(vm, "color_bg_app"), want, "the mix did not come back");
            assert_eq!(
                read_widget_theme(vm, "color_bg_app"),
                want,
                "the widgets were not built off the mix that came back"
            );
        });
    }

    /// Being told about a rebuild is not being told the lab owes nothing. A
    /// rebuild puts up the base theme and whatever sheet is installed, and
    /// installing a mix took the entry sheet off, so the app is wearing
    /// neither the mix nor what the lab found -- and leaving is still the only
    /// thing that can hand the sheet back. A lab that read an invalidation as
    /// "nothing of mine is in force any more" would fold the section away and
    /// leave somebody's sheet off for good.
    #[test]
    fn an_invalidated_lab_still_owes_the_entry_theme_back() {
        let mut lab = ThemeLab::new();
        lab.cache = bench();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            desktop_style::install(vm, StyleSheet::load_with_appearance(DesktopStyle::Omarchy, false));
            crate::set_base_theme(vm.cx_mut(), BaseTheme::Dark);
            lab.enter(vm);
            let dark = lab.index_of(DARK).unwrap();
            lab.set_weight(dark, RELATIVE_TOTAL);
            assert_eq!(lab.apply(vm).unwrap(), Applied::Mix);
            assert_eq!(desktop_style::current_name(vm), None, "a mix stands on its base");
            vm.with_reload(crate::script_mod);
            lab.invalidate();
            assert!(lab.installed, "the sheet is still off, so the lab still owes it back");
            lab.leave(vm);
            assert_eq!(
                desktop_style::current_name(vm).as_deref(),
                Some("omarchy"),
                "leaving after a reload did not put the sheet back"
            );
        });
    }

    /// The reading taken on the frame a mix went in is the blend that went in,
    /// and not the same arithmetic run a second time over the same numbers.
    /// The only thing that can pin that is a change to the resolved themes
    /// underneath, which a second blend would show and a kept one cannot.
    /// (Nothing does that in an app -- `enter` fills the cache and entering
    /// twice does nothing -- so the kept blend is not a stale answer, it is
    /// the same answer.)
    #[test]
    fn the_reading_on_an_applied_frame_is_the_blend_that_went_in() {
        let mut lab = ThemeLab::new();
        lab.cache = bench();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            desktop_style::uninstall(vm);
            lab.enter(vm);
            let dark = lab.index_of(DARK).unwrap();
            let omarchy = lab.index_of(OMARCHY).unwrap();
            lab.set_mode(WeightMode::Absolute);
            lab.clear_weight(dark);
            lab.set_weight(omarchy, RELATIVE_TOTAL);
            assert_eq!(lab.apply(vm).unwrap(), Applied::Mix);
            let installed = lab.readability();
            assert!(installed.holds(), "the made-up themes read: {installed:?}");

            // The one theme in the mix is replaced with one that does not
            // read. No weight has moved, so the lab still has the blend it
            // installed and the reading must not have changed.
            lab.cache.insert(unreadable(OMARCHY));
            assert_eq!(lab.readability(), installed, "the mix was blended a second time");

            // Any move at all and it is measured again. The same single theme
            // at a different weight is the same blend, so what shows through
            // is the replacement and nothing else.
            lab.set_weight(omarchy, RELATIVE_TOTAL - 1.0);
            let again = lab.readability();
            assert_eq!(again.measured, 4);
            assert_eq!(again.failures.len(), 4, "{again:?}");
        });
    }

    /// A panel has room for one line, so the line has to name the worst pair
    /// and not the first pair. The near miss at the top of the table is the
    /// one nobody would notice; the second voice eleven rows down, at 1.40
    /// against a bar of 3, is the one that has gone.
    #[test]
    fn the_worst_failure_is_the_one_a_panel_shows_first() {
        let mut lab = entered();
        lab.cache.insert(unreadable(DARK));
        let reading = lab.readability();
        assert_eq!(reading.measured, 4, "{reading:?}");
        assert_eq!(reading.failures.len(), 4, "{reading:?}");
        assert!(
            reading.failures[0]
                .starts_with("color_on_surface_variant on color_surface_container_highest"),
            "the worst pair is not the one shown: {:?}",
            reading.failures
        );
        assert!(
            reading.failures.last().unwrap().starts_with("color_on_surface on color_surface ="),
            "the near miss is not last: {:?}",
            reading.failures
        );
        assert_eq!(reading.tightest, reading.failures[0], "the worst pair is named twice over");
        assert!((reading.margin - (1.398 - 3.0)).abs() < 0.01, "{}", reading.margin);
        assert!(!reading.holds());
    }

    /// Every call that moves a weight is a no-op on a lab nobody has entered,
    /// which is deliberate -- a panel draws the folded section before it opens
    /// it -- and is also the trap that makes a test of an un-entered lab
    /// assert nothing whatever it calls. `is_open` is how either side tells.
    #[test]
    fn nothing_a_closed_lab_is_asked_to_do_happens() {
        let mut lab = ThemeLab::new();
        assert!(!lab.is_open());
        assert!(lab.rows().is_empty());
        assert_eq!(lab.index_of(DARK), None, "there is no row to point at");
        lab.set_weight(0, 50.0);
        lab.clear_weight(0);
        lab.randomize(0x5EED);
        lab.reset();
        assert!(lab.rows().is_empty(), "a closed lab grew a row");
        assert_eq!(lab.readability().measured, 0, "and measured a mix of nothing");
        let mut cx = Cx::new(Box::new(|_, _| {}));
        assert_eq!(cx.with_vm(|vm| lab.apply(vm)), Ok(Applied::Nothing));
        assert_eq!(lab.rebuilds(), 0, "a closed lab rebuilt the module");
        let lab = entered();
        assert!(lab.is_open());
        assert_eq!(lab.index_of(DARK), Some(0), "the dark theme is the first of its group");
    }

    /// A caller says which theme it wants, not what the theme is called and
    /// not where the library happens to keep it. Asking for a theme of the
    /// other group is answered rather than guessed at.
    #[test]
    fn a_theme_is_found_by_what_it_is_and_not_by_what_it_is_called() {
        let mut lab = entered();
        let dark = lab.index_of(DARK).unwrap();
        assert_eq!(lab.rows()[dark].label, DARK.label());
        assert_eq!(lab.index_of(BlendTheme::Base(Scheme::Light)), None, "the other group");
        lab.set_weight(dark, 40.0);
        assert_eq!(lab.rows()[dark].weight, 40.0, "the index did not point at the theme");
        lab.set_appearance(Appearance::Light);
        assert_eq!(lab.index_of(DARK), None, "the dark group is not on show");
        for (index, theme) in BlendTheme::group(lab.appearance()).into_iter().enumerate() {
            assert_eq!(lab.index_of(theme), Some(index), "{}", theme.name());
            assert_eq!(lab.rows()[index].label, theme.label());
        }
    }

    /// Resolving the themes is the whole cost of the lab, so a second lab --
    /// or a caller outside this crate, who would otherwise pay 0.6 s of real
    /// resolutions per enter -- is handed the answers instead of paying for
    /// them again.
    #[test]
    fn a_second_lab_enters_on_the_first_lab_s_themes() {
        let first = entered();
        assert_eq!(first.cache().len(), BlendTheme::all().len());
        let mut second = ThemeLab::with_cache(first.cache().clone());
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            desktop_style::uninstall(vm);
            second.enter(vm);
        });
        assert!(second.is_open());
        assert_eq!(second.rows(), first.rows());
        // The made-up themes, still: a real resolution over the top of them
        // would not leave this radius behind.
        assert_eq!(
            second.cache().get(DARK).unwrap().num("radius_m"),
            Some(2.0),
            "a theme was resolved over the one the lab was handed"
        );
    }

    /// A drag is forty moves and one install. The lab has no frame clock and
    /// cannot make that true, but it counts and it says what it did, so a
    /// panel that installs per drawn frame is a number in a test rather than a
    /// stutter somebody has to notice.
    #[test]
    fn forty_moves_and_one_apply_is_one_rebuild() {
        let mut lab = ThemeLab::new();
        lab.cache = bench();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            desktop_style::uninstall(vm);
            lab.enter(vm);
            assert_eq!(lab.rebuilds(), 0, "entering a warm lab installs nothing");
            let omarchy = lab.index_of(OMARCHY).unwrap();
            for step in 0..40u32 {
                lab.set_weight(omarchy, step as f64);
            }
            let did = lab.apply(vm).unwrap();
            assert_eq!(did, Applied::Mix);
            assert!(did.rebuilt());
            assert_eq!(lab.rebuilds(), 1, "the drag cost more than the one install it needed");
            let again = lab.apply(vm).unwrap();
            assert_eq!(again, Applied::Nothing, "a settled mix went in a second time");
            assert!(!again.rebuilt());
            assert_eq!(lab.rebuilds(), 1);
            // Back to the theme the lab opened on, which is a rebuild of its
            // own and the other thing `Applied` tells apart.
            lab.reset();
            assert_eq!(lab.apply(vm).unwrap(), Applied::Entry);
            assert_eq!(lab.rebuilds(), 2);
        });
    }

    fn read_theme(vm: &mut ScriptVm, key: &str) -> Option<u32> {
        let theme = vm.module(LiveId::from_str("theme"));
        vm.bx.heap.value(theme, LiveId::from_str(key).into(), NoTrap).as_color()
    }

    /// The theme object the widget module was handed, which is what every
    /// `theme.color_x` in a template was baked from.
    fn read_widget_theme(vm: &mut ScriptVm, key: &str) -> Option<u32> {
        let prelude = vm.module(LiveId::from_str("prelude"));
        let internal = vm
            .bx
            .heap
            .value(prelude, LiveId::from_str("widgets_internal").into(), NoTrap)
            .as_object()?;
        let theme = vm.bx.heap.value(internal, LiveId::from_str("theme").into(), NoTrap).as_object()?;
        vm.bx.heap.value(theme, LiveId::from_str(key).into(), NoTrap).as_color()
    }
}
