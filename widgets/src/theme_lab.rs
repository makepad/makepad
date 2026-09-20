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
//! An install is a choice written onto the `Cx` and a module run asked for:
//! `cx.request_style_reload()`. It has to be. A widget does not read
//! `mod.theme` when it draws -- its colour was baked into the template it was
//! built from, as a literal, when that template's module block ran -- and an
//! app's templates are built in the app's OWN module tail. So nothing short
//! of re-running that tail moves an app, and the tail is `script_mod`, and
//! re-running `script_mod` is a module run. A mix evaluated once into a
//! module would be thrown away by the very run that was meant to carry it;
//! held on the Cx and re-emitted by `theme_mod`, it survives, and the run
//! that makes an app wear it is the same run that puts it back up.
//!
//! What a panel owes the lab in return is a word whenever something else
//! rebuilds the module underneath it -- a style reload, a live edit, a base
//! theme switched from elsewhere in the app. That word is
//! [`ThemeLab::invalidate`], and it is cheap: the mix stands through a
//! rebuild, so what the word buys is a fresh reading, not a reinstall.
//!
//! The other thing it owes is the part of the theme in force that a base
//! theme and a style sheet do not add up to. A theme somebody saved is those
//! two with a set of tokens pinned over them, and the pins are the one part
//! the lab cannot see for itself: `mod.theme` holds what the tokens came to
//! rather than which of them were pinned, and the module rebuild that puts
//! the base and the sheet back is the very thing that throws them away. So a
//! panel opening the lab over a saved theme says so on the way in --
//! [`ThemeLab::enter_pinned`] rather than [`ThemeLab::enter`] -- and gets its
//! own tokens back on the way out and not only the base underneath them.
//!
//! The pins go back the way the mix goes on: held on the Cx and emitted at
//! the seam. There is one theme they do not reach that way, and it is worth
//! naming rather than discovering -- a saved theme with a style SHEET under
//! it. A sheet's own script runs AFTER that seam and points `mod.theme` back
//! at its own base as its first line, so it goes over a set of pins as
//! readily as it goes over a mix; a mix answers that by taking the sheet off,
//! and a theme being handed back cannot, because the sheet is half of what is
//! being handed back. Over a sheet the lab returns the base and the sheet,
//! and the tokens pinned over the pair of them are the panel's to land, on
//! the tick after the reload, exactly as the panel landed them when the theme
//! was picked. See `ThemeLab::install_entry`.
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
use crate::makepad_platform::{ScriptVm, ScriptVmCx};
use crate::theme_tokens::{
    held_pairs, random_weights, reads_on, set_weight, to_relative, Scheme, ThemeBlend,
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
pub(crate) const MIX_NAME: &str = "equalized";

/// How near two weights have to be to count as the same one. The relative
/// mode settles its total by moving the drift onto the largest weight, so an
/// untouched slider row can come back a few ulps from where it was, and a
/// dirty check on exact equality would reinstall the same theme every frame.
const SAME_WEIGHT: f64 = 1e-9;

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
/// An install is a module rebuild -- about 52 ms, three frames and a half -- and
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

/// The part of a theme that a base theme and a style sheet do not add up to.
///
/// A theme somebody saved is a base, a sheet, and a set of tokens pinned over
/// the pair of them. [`ThemeLab::enter`] can read the first two off the vm
/// and cannot read the third: what it would find there is `mod.theme`, which
/// holds what the tokens came to rather than which of them were pinned, and
/// the module rebuild that puts the base and the sheet back is exactly what
/// throws the pins away. Only the panel knows them, because it is the panel
/// that loaded them and put them on.
///
/// So it hands them over as the script it installed them with, which is the
/// same shape the lab installs a mix with and the same shape
/// `crate::theme_store` writes a saved theme in. A panel holding a
/// `SavedTheme` has both halves of one already:
/// `PinnedTheme::new(&saved.name, &saved.script())`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedTheme {
    name: String,
    script: String,
}

impl PinnedTheme {
    /// `name` labels the script in a log and a diagnostic and does nothing
    /// else; what the script files itself under in `mod.themes` is the
    /// script's own business.
    ///
    /// The trailing `true` the language wants is added here rather than asked
    /// of the caller. The last statement of an evaluated script is swallowed,
    /// and the last statement of an override script is the assignment that
    /// makes the theme current, so a script handed over as it was written
    /// would pin every token and then leave the theme it pinned them into
    /// sitting in `mod.themes` unworn -- silently, and with nothing in the
    /// log. A script that already ends in one is none the worse for a second.
    pub fn new(name: &str, script: &str) -> Self {
        let mut script = script.to_string();
        if !script.ends_with('\n') {
            script.push('\n');
        }
        script.push_str("true\n");
        Self { name: name.to_string(), script }
    }

    /// The name this was made under.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The script as it will be evaluated, trailing `true` and all.
    pub fn script(&self) -> &str {
        &self.script
    }
}

/// What was in force when the lab was entered, so that leaving can put it
/// back. The base theme and the sheet are two separate choices -- a sheet is
/// laid over a base rather than replacing it -- so both are taken, and the
/// theme the pair amounts to is worked out once here rather than at every
/// call that needs it.
///
/// A saved theme is those two with tokens pinned over them, and the pins ride
/// here too: see [`PinnedTheme`]. Without them, leaving restored a theme by
/// its name only -- the right base, the right sheet, and none of what the
/// person had actually chosen -- and the picker went on naming a theme the
/// screen was not.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    base: BaseTheme,
    sheet: Option<(DesktopStyle, bool)>,
    theme: BlendTheme,
    /// The tokens the entry theme pins over its base and its sheet, and
    /// `None` for a theme that is just those two: every built-in, and every
    /// style sheet.
    pinned: Option<PinnedTheme>,
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
    /// A closed lab is inert. It has no rows, so `set_weight` and
    /// `clear_weight` find no index, `reset` and `randomize` have nothing to
    /// write, `index_of` has no row to point at, and `readability` has no mix
    /// to measure; `set_appearance` draws no group, and `apply` answers
    /// [`Applied::Nothing`].
    ///
    /// The rows are part of that rather than an exception to it, which is
    /// worth saying because they look cheap enough to draw early. Entering
    /// starts the mix from the theme in force, and the theme in force is the
    /// one thing only [`ThemeLab::enter`] knows -- so entering resets every
    /// weight, and a weight moved before then is a weight entering throws
    /// away. A panel offering rows over a closed lab would be offering a
    /// control whose every move is lost.
    ///
    /// What a closed lab does still do is remember. [`ThemeLab::set_mode`]
    /// keeps the mode for the next entry, [`ThemeLab::rebuilds`] counts for
    /// the whole life of the lab, and the resolved themes are kept across a
    /// leave so that opening again is instant -- see [`ThemeLab::cache`].
    ///
    /// The silence is deliberate: a panel draws the folded section before it
    /// ever opens it, and should not have to guard each call. But it also
    /// means a test that forgets to enter asserts nothing whatever it calls,
    /// so a test asks this first.
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
    /// This is the door for a theme that IS a base theme and a style sheet:
    /// every built-in the library ships, and every desktop sheet. A theme
    /// with tokens pinned over those -- one somebody saved -- comes in
    /// through [`ThemeLab::enter_pinned`] instead, because the pins are the
    /// one part of it the lab cannot see from here.
    ///
    /// The panel calls this inside `cx.with_vm`.
    pub fn enter(&mut self, vm: &mut ScriptVm) {
        self.enter_on(vm, None);
    }

    /// Remember a theme that pins tokens over its base and its sheet, and
    /// make a mix of it.
    ///
    /// [`ThemeLab::enter`] in every other respect, and the same cost. What it
    /// adds is the only part of a saved theme the lab cannot read off the vm
    /// for itself, so that leaving is the inverse of entering rather than two
    /// thirds of it. See [`PinnedTheme`], and `ThemeLab::install_entry` for
    /// the one theme the pins do not reach on the way back out.
    ///
    /// The panel calls this inside `cx.with_vm`.
    pub fn enter_pinned(&mut self, vm: &mut ScriptVm, pinned: &PinnedTheme) {
        self.enter_on(vm, Some(pinned.clone()));
    }

    fn enter_on(&mut self, vm: &mut ScriptVm, pinned: Option<PinnedTheme>) {
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
        self.entry = Some(Entry { base, sheet, theme, pinned });
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
    ///
    /// A closed lab takes no group: it has no theme for the mix to start
    /// from, so every weight this would draw is one that entering would throw
    /// away again. See [`ThemeLab::is_open`].
    pub fn set_appearance(&mut self, appearance: Appearance) {
        if !self.is_open() || appearance == self.appearance {
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
    /// rebuilding that module leaves this answering `false` over a reading
    /// taken off a module that has gone -- the mix itself stands, because it
    /// is held on the Cx and emitted again by the very run that rebuilt
    /// everything else -- and that stale reading is what
    /// [`ThemeLab::invalidate`] is for.
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
    /// [`ThemeLab::apply`] reads it afresh.
    ///
    /// Every caller of `cx.request_style_reload()` or `cx.request_live_edit()`
    /// owes the lab this call, and so does anything else that re-runs
    /// `script_mod`: switching the base theme, putting a style sheet on or
    /// taking one off, a live edit landing from a file watcher. None of it is
    /// visible from here -- the lab holds the weights it installed, not the
    /// module it installed them into.
    ///
    /// What is dropped is the memory of what was installed, which makes the
    /// lab dirty again, so the next [`ThemeLab::apply`] takes a fresh reading
    /// over a module that has just been rebuilt. What that apply will NOT do
    /// is install again: the mix is held on the Cx and `theme_mod` re-emits
    /// it, so it came back up with the module. A word about a rebuild costs a
    /// blend, not a rebuild.
    ///
    /// What is NOT dropped is that the lab still owes the entry theme back.
    /// Installing a mix took the entry sheet off, and only [`ThemeLab::leave`]
    /// can put it back; clearing that debt here would leave somebody in a
    /// theme nobody chose with no way back but to find their sheet by hand.
    pub fn invalidate(&mut self) {
        self.applied = None;
        self.blended = None;
    }

    /// Put the mix in force, and say what that took.
    ///
    /// Safe to call on any frame in the sense that an untouched mix is a
    /// return and nothing else. The call itself is half a millisecond -- it
    /// blends, writes the blend onto the Cx and asks for a style reload --
    /// but the reload it asks for is a module rebuild, about 52 ms on the
    /// tick that follows. So a panel still owes it a settle: apply on the
    /// control's own end-of-drag action, or on a timeout re-armed by every
    /// edit. Forty applies across a 500 ms drag is not a slower drag, it is
    /// forty module rebuilds queued behind it. The lab has no frame clock and
    /// cannot make the settle happen; what it can do is hand back what it did,
    /// so that a caller can count its own rebuilds -- see [`Applied`] and
    /// [`ThemeLab::rebuilds`].
    ///
    /// The style reload is not an alternative to an install, it IS the
    /// install. `request_script_reapply` would re-apply the widget tree from
    /// the value captured at startup, over templates that were baked off the
    /// theme this mix replaced, and write the old colours faithfully back
    /// across the whole screen; it does not re-run `script_mod`, and
    /// `script_mod` is where an app's templates are made. So the blend is put
    /// where `theme_mod` will find it on every run -- see
    /// [`crate::set_theme_mix`] -- at the one seam it is seen from, after the
    /// themes exist and before a widget template bakes `theme.color_x` into a
    /// literal it will not evaluate again.
    ///
    /// The mix that is already in force does not go in twice. A panel is told
    /// about every rebuild, including the one this call asked for, so an
    /// apply that installed unconditionally would ask for the reload that
    /// told it.
    ///
    /// A theme chosen from outside the lab wins, and this is the call that
    /// notices. A mix that has gone from the Cx under a lab that believes it
    /// installed one is somebody else's pick -- the app's own picker sets the
    /// base theme, and setting the base theme stands a mix down -- so the lab
    /// does not put it back: it opens again on the theme now in force and
    /// answers [`Applied::Nothing`]. The section stays open and the weights
    /// start from what was picked, so the only thing a panel has to do about
    /// it is draw its rows again.
    ///
    /// The sheet comes off, and stays off, because that choice is a Cx global
    /// too. A sheet's own script re-points `mod.theme` back at its base and
    /// mutates it, from the first line of the widget module, which is after
    /// the mix has been emitted and therefore over the top of it. The mix
    /// already carries every token the sheet set, by weight; what is given up
    /// is the sheet's widget re-skins and its font fallbacks.
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
        // A closed lab refuses first. Everything below this asks something
        // about the theme the lab was opened on, and a closed lab has none.
        if !self.is_open() {
            return Ok(Applied::Nothing);
        }
        // Then a lab that has been stood down from outside. `set_base_theme`
        // takes a standing mix off, so a lab that believes it installed one
        // and finds none on the Cx has had the theme chosen out from under it
        // -- the app's own picker, an appearance that followed the desktop.
        // The pick WINS: putting the mix back would make that picker do
        // nothing for as long as the section was open, which is the same
        // complaint in other clothes.
        //
        // Asked BEFORE the dirty check, because a pick moves no weight: the
        // rows are exactly where the lab left them, so a call that asked what
        // had moved first would answer `Nothing` and never look.
        //
        // What the lab does instead is open again on the theme now in force,
        // so the weights start from what was picked and `leave` hands THAT
        // back rather than a theme last asked for three clicks ago. No
        // install, no rebuild: the pick is already on its way up on the
        // reload that told us. The entry's pins go with the entry, because
        // they are the tokens of the theme that has just been replaced; a
        // panel that knows the pick was a saved theme enters the lab on it
        // again itself, pins and all.
        if self.installed && crate::theme_mix(vm.cx_mut()).is_none() {
            self.installed = false;
            self.entry = None;
            self.enter(vm);
            return Ok(Applied::Nothing);
        }
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
                // Cloned rather than borrowed, because putting it back is a
                // call on `self`; and only here, because a saved theme's
                // pins are a script of some size and every other path
                // through this call has no use for them.
                if let Some(entry) = self.entry.clone() {
                    self.install_entry(vm, &entry);
                }
                self.installed = false;
                did = Applied::Entry;
            }
        } else {
            let blend = self.cache.blend(&self.mix())?;
            let code = Self::mix_script(&blend);
            // A mix that is already the one in force does not go in again.
            // The module can be rebuilt under the lab -- a style reload, a
            // live edit, a base theme switched elsewhere -- and the rebuild
            // re-emits this very script, so a lab told about that rebuild
            // ([`ThemeLab::invalidate`]) is owed a fresh blend to measure and
            // nothing else. Installing would ask for the reload that told us,
            // and that is a loop with a module rebuild in it.
            if crate::theme_mix(vm.cx_mut()).as_deref() == Some(code.as_str()) {
                did = Applied::Nothing;
            } else {
                self.install_blend(vm, code);
                self.installed = true;
                did = Applied::Mix;
            }
            blended = Some(blend);
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
    /// to undo, and a rebuild for the sake of it is 52 ms of nothing.
    ///
    /// The panel calls this inside `cx.with_vm`.
    pub fn leave(&mut self, vm: &mut ScriptVm) {
        let Some(entry) = self.entry.take() else {
            return;
        };
        if self.installed {
            // Borrowed rather than taken: the entry carries the script its
            // pins are written in, and is no longer the two words a `Copy`
            // was made of. Nothing follows it, either -- `install_entry` asks
            // for the style reload, and that reload IS the install. A script
            // reapply on top of it would walk the tree from the value
            // captured at startup, over templates the reload is about to
            // rebuild, and write the mix's colours faithfully back.
            self.install_entry(vm, &entry);
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
        for (ground, ink, need) in held_pairs() {
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
            .as_ref()
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
        let Some(entry) = self.entry.as_ref() else {
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

    /// The mix put where the run that carries it will find it, and that run
    /// asked for.
    ///
    /// Nothing is evaluated here. A blend evaluated into the module moves
    /// `mod.theme` and not one thing an app drew from it -- a widget's colour
    /// was baked into its template as a literal when the app's own module
    /// block ran -- and the rebuild that remakes those templates is the very
    /// thing that would throw an evaluated blend away. So the blend goes onto
    /// the Cx, where `theme_mod` emits it at the one seam it is seen from, on
    /// every run it makes; and the run is asked for. See
    /// [`crate::set_theme_mix`].
    ///
    /// The sheet comes off first and stays off, for the reason
    /// [`ThemeLab::apply`] gives: a sheet's own script runs after that seam
    /// and would point `mod.theme` back at its own base over the top of the
    /// mix.
    fn install_blend(&mut self, vm: &mut ScriptVm, code: String) {
        self.rebuilds = self.rebuilds.saturating_add(1);
        desktop_style::uninstall(vm);
        crate::set_theme_mix(vm.cx_mut(), Some(code));
        vm.cx_mut().request_style_reload();
    }

    /// The script that makes a blend the theme: the blend's own module
    /// script, and nothing added to it.
    ///
    /// The last statement of an evaluated script is swallowed, and the last
    /// statement of this one is the assignment that makes the mix current --
    /// but the `true` that takes that fall is
    /// [`crate::theme_tokens::theme_module_script`]'s own now, and every
    /// caller gets it. A second terminator here would be a third place that
    /// knows about the first.
    ///
    /// A function rather than a line inside [`ThemeLab::install_blend`],
    /// because [`ThemeLab::apply`] asks for the script BEFORE it installs
    /// anything, to compare it with the one already on the Cx. One generator,
    /// so a mix already in force is recognised by the very text it would have
    /// gone in as.
    fn mix_script(blend: &ThemeBlend) -> String {
        blend.script(MIX_NAME)
    }

    /// The theme the lab was entered on back onto the Cx -- its base, its
    /// sheet, and, where no sheet is in the way of them, its own pinned
    /// tokens -- and the module run that makes an app wear what went back
    /// asked for.
    ///
    /// The pins ride where the mix rides, and for the same reason. They are a
    /// script that makes a theme current, and a script evaluated here would
    /// be thrown away by the very rebuild that puts the base and the sheet
    /// back; held on the Cx, `theme_mod` emits them on every run it makes, so
    /// the run that hands the theme back is the run that hands back the part
    /// of it only the panel knew. See [`PinnedTheme`].
    ///
    /// Written AFTER the base theme and not before: `set_base_theme` takes a
    /// standing mix off -- it is the call a picker makes, and a picker's
    /// whole point is that the mix stands down in front of it -- so pins
    /// written first are pins that call throws away.
    ///
    /// An entry with no pins writes `None` there, which is the same line
    /// taking the mix off, so every way out of a mix leaves by one door.
    ///
    /// What this does NOT put back is a saved theme's pins where a style
    /// sheet sits under them. The seam is the last word on `mod.theme` in a
    /// run that has no sheet in it, and the first of several in a run that
    /// has: `desktop_style::apply_theme` evaluates the sheet's own script
    /// after it, and that script's first line points `mod.theme` back at the
    /// base theme it was written against -- over the pins, as it would go
    /// over a mix. The lab has no seam of its own after the sheet, and the
    /// place that has one is the panel: `Event::LiveEdit` reaches a widget
    /// after the rebuild the reload asked for, which is where a saved
    /// theme's tokens go on when it is PICKED, and is the only place they can
    /// go on when it is handed back. A panel that lands them there on an
    /// [`Applied::Entry`] and on the fold closes this; the lab cannot.
    fn install_entry(&mut self, vm: &mut ScriptVm, entry: &Entry) {
        self.rebuilds = self.rebuilds.saturating_add(1);
        crate::set_base_theme(vm.cx_mut(), entry.base);
        match entry.sheet {
            Some((style, dark)) => {
                desktop_style::install(vm, StyleSheet::load_with_appearance(style, dark))
            }
            None => desktop_style::uninstall(vm),
        }
        let pinned = entry.pinned.as_ref().map(|pinned| pinned.script.clone());
        crate::set_theme_mix(vm.cx_mut(), pinned);
        vm.cx_mut().request_style_reload();
    }
}

#[cfg(test)]
mod theme_lab_tests {
    use super::*;
    use crate::makepad_platform::{Cx, LiveId, NoTrap, ScriptMod};
    use crate::theme_tokens::{
        mix_rgb, theme_module_script, BlendValue, ThemeValues, TokenValue, READABLE,
    };
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

    /// The tick a `cx.request_style_reload()` lands on, driven by hand.
    ///
    /// An install writes a choice onto the Cx and asks for a reload; the
    /// reload is what re-runs `script_mod` and rebuilds every template the
    /// widgets are made from, so until it has run nothing an install did is
    /// anywhere. In an app that run is `app_main!`'s `Event::LiveEdit` arm,
    /// over the APP's own `script_mod` -- which is the whole reason the choice
    /// lives on the Cx: an app's templates bake `theme.color_x` into a literal
    /// when the app's module block runs, and re-running that is the only thing
    /// that moves them. Here the app is the library.
    fn the_reload_lands(vm: &mut ScriptVm) {
        assert!(
            std::mem::take(&mut vm.cx_mut().pending_style_reload),
            "nothing asked for the style reload that carries an install to the app"
        );
        vm.cx_mut().pending_live_edit_request = false;
        vm.with_reload(crate::script_mod);
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
    /// deliberate order: the first pair the table finds anything for here is a
    /// near miss at 4.37 against a bar of 4.5, and the worst of the four is
    /// the second voice on the highest container, at 1.40 against a bar of 3.
    /// It carries four of the seventeen pairs and nothing else, so the other
    /// thirteen are skipped and `measured` is 4. Neither ground is derived again
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
            the_reload_lands(vm);
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
            assert_eq!(lab.entry.as_ref().unwrap().theme, OMARCHY);
            assert_eq!(lab.appearance(), Appearance::Dark, "a dark sheet opens the dark group");
            // A lab that was only looked at restores nothing.
            let mut untouched = lab.clone();
            untouched.leave(vm);
            assert!(!untouched.installed);
            assert_eq!(desktop_style::current_name(vm).as_deref(), Some("omarchy"));

            let dark = lab.index_of(DARK).unwrap();
            lab.set_weight(dark, RELATIVE_TOTAL);
            assert_eq!(lab.apply(vm).unwrap(), Applied::Mix);
            the_reload_lands(vm);
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

    /// A saved theme is a base, a sheet, and a set of tokens pinned over the
    /// pair of them, and the pins are the part only the panel knows. Four
    /// clicks used to lose them: pick the theme, open the mix, move a weight,
    /// press `- mix`. Leaving put the base back and dropped the pins on the
    /// floor, so the picker went on naming a theme the screen was not.
    ///
    /// Driven the way the panel drives it and read off the app: the pinned
    /// ground goes on for real, a real mix goes over it, and BOTH ways back
    /// out -- the rows returning to the theme the lab opened on, and the fold
    /// -- have to hand back the pinned value rather than the base theme's.
    /// Each way back out is read after the reload it asked for, because that
    /// reload IS the install: the pins are held on the Cx and it is the run
    /// they are emitted in.
    ///
    /// The first assertion is also the guard for the swallowed last
    /// statement: without the `true` [`PinnedTheme::new`] appends, the line
    /// that makes the theme current never runs and the theme never goes on.
    ///
    /// Over a bare base theme, because that is the theme whose pins are the
    /// lab's to hand back. A saved theme with a style SHEET under it is not:
    /// the sheet's script runs after the seam the pins are emitted at and
    /// points `mod.theme` at its own base, so what comes back over a sheet is
    /// the base and the sheet -- see `ThemeLab::install_entry`, which says
    /// where the rest of that theme has to come from. That the sheet itself
    /// comes back is `leaving_puts_back_what_entering_found`.
    #[test]
    fn leaving_puts_back_a_saved_themes_own_tokens_and_not_only_its_base() {
        const PINNED: u32 = 0x3B1F5CFF;
        let mut lab = ThemeLab::new();
        lab.cache = bench();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            desktop_style::uninstall(vm);
            crate::set_base_theme(vm.cx_mut(), BaseTheme::Dark);
            vm.with_reload(crate::script_mod);
            // The theme somebody saved, put on the way the panel puts one on:
            // the base theme, and then its own tokens over the top.
            let pinned = PinnedTheme::new(
                "sunset",
                &theme_module_script(
                    "sunset",
                    "dark",
                    &[("color_bg_app".to_string(), TokenValue::Color(PINNED))],
                ),
            );
            vm.eval(ScriptMod {
                cargo_manifest_path: env!("CARGO_MANIFEST_DIR").into(),
                module_path: "theme_lab_test_sunset".to_string(),
                file: "sunset.splash".to_string(),
                line: 0,
                column: 0,
                code: pinned.script().to_string(),
                values: vec![],
            });
            assert_eq!(
                read_theme(vm, "color_bg_app"),
                Some(PINNED),
                "the saved theme never went on, so nothing below is about one"
            );

            lab.enter_pinned(vm, &pinned);
            assert_eq!(
                lab.entry.as_ref().unwrap().theme,
                DARK,
                "the base theme under the pins"
            );
            let omarchy = lab.index_of(OMARCHY).unwrap();
            lab.set_mode(WeightMode::Absolute);
            lab.set_weight(omarchy, RELATIVE_TOTAL);
            assert_eq!(lab.apply(vm).unwrap(), Applied::Mix);
            the_reload_lands(vm);
            assert_ne!(read_theme(vm, "color_bg_app"), Some(PINNED), "the mix never reached the app");

            // The rows back where they started, which is one of the two ways
            // the entry theme goes back on.
            lab.reset();
            assert_eq!(lab.apply(vm).unwrap(), Applied::Entry);
            the_reload_lands(vm);
            assert_eq!(
                read_theme(vm, "color_bg_app"),
                Some(PINNED),
                "the entry theme came back as its bare base and none of its own tokens"
            );

            // ...and the fold, which is the press it was found on.
            lab.set_weight(omarchy, RELATIVE_TOTAL);
            assert_eq!(lab.apply(vm).unwrap(), Applied::Mix);
            the_reload_lands(vm);
            lab.leave(vm);
            the_reload_lands(vm);
            assert_eq!(
                read_theme(vm, "color_bg_app"),
                Some(PINNED),
                "leaving left the picker naming a theme the screen is not"
            );
            assert_eq!(crate::base_theme(vm.cx_mut()), BaseTheme::Dark);
        });
    }

    /// The app can be rebuilt out from under the lab -- a style reload, a live
    /// edit, a base theme switched elsewhere -- and the mix stands through it.
    /// It has to: the reload that makes an app wear the mix at all IS a
    /// rebuild, so a mix a rebuild could lose could never be worn.
    ///
    /// The lab still cannot SEE a rebuild, and being told about one is still
    /// free to do -- what it must not cost is a second install. A lab that
    /// reinstalled on every word would ask for the reload that told it.
    #[test]
    fn a_rebuild_under_the_lab_leaves_the_mix_standing() {
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
            the_reload_lands(vm);
            let mix = [(DARK, RELATIVE_TOTAL), (OMARCHY, RELATIVE_TOTAL)];
            let want = lab.cache.blend(&mix).unwrap().color("color_bg_app");
            assert_eq!(read_theme(vm, "color_bg_app"), want, "the mix did not go on");

            // Somebody else's rebuild, with the mix standing: a live edit off
            // the file watcher, the panel's own wear switch, a theme reload.
            // `theme_mod` re-emits the mix, so it comes back up with the
            // module rather than being buried by it.
            vm.with_reload(crate::script_mod);
            assert_eq!(read_theme(vm, "color_bg_app"), want, "the rebuild took the mix off");
            assert_eq!(
                read_widget_theme(vm, "color_bg_app"),
                want,
                "the widgets of the rebuilt module were not built off the mix"
            );
            assert!(!lab.is_dirty(), "a mix that never came off is owed an apply");

            // And being told about it is free. This is the loop: the panel is
            // told about EVERY rebuild, including the one its own install
            // asked for, so an apply that reinstalled here would ask for the
            // reload that told it, with a module rebuild inside the turn.
            lab.invalidate();
            assert!(lab.is_dirty(), "a lab that has been told is owed an apply");
            let rebuilds = lab.rebuilds();
            assert_eq!(
                lab.apply(vm).unwrap(),
                Applied::Nothing,
                "a mix already in force went in a second time"
            );
            assert_eq!(lab.rebuilds(), rebuilds, "and cost a rebuild doing it");
            assert!(
                !vm.cx_mut().pending_style_reload,
                "the apply asked for the reload that would tell it again"
            );
            assert!(!lab.is_dirty(), "and is still owed one");
            assert_eq!(read_theme(vm, "color_bg_app"), want);
        });
    }

    /// A theme chosen from OUTSIDE the lab wins, and the lab opens again on
    /// it.
    ///
    /// This is the other half of holding the mix on the Cx. A blend that
    /// `theme_mod` re-emits on every run survives every rebuild -- which is
    /// the point -- and would therefore survive the rebuild an app's own
    /// theme picker asks for too, going straight back over the top of the
    /// theme just chosen. A picker that had worked for years would sit there
    /// doing nothing for as long as the section was open.
    ///
    /// So `set_base_theme` takes a standing mix off, and the lab reads a mix
    /// that has gone as having been stood down: it does not put the blend
    /// back, it re-enters on what is now in force. What `leave` then hands
    /// back is the theme the picker names, and the weights start from it.
    ///
    /// Driven here through the bare calls an app's picker makes --
    /// `set_base_theme` and a style reload -- because that is all the library
    /// can see of it. `apps/storybook/src/theme.rs::select` is the one that
    /// was measured doing nothing.
    #[test]
    fn a_theme_chosen_from_outside_the_lab_is_the_theme_that_stays() {
        let mut lab = ThemeLab::new();
        lab.cache = bench();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            desktop_style::uninstall(vm);
            crate::set_base_theme(vm.cx_mut(), BaseTheme::Dark);
            vm.with_reload(crate::script_mod);
            lab.enter(vm);
            let omarchy = lab.index_of(OMARCHY).unwrap();
            lab.set_mode(WeightMode::Absolute);
            lab.set_weight(omarchy, RELATIVE_TOTAL);
            assert_eq!(lab.apply(vm).unwrap(), Applied::Mix);
            the_reload_lands(vm);
            let mix = [(DARK, RELATIVE_TOTAL), (OMARCHY, RELATIVE_TOTAL)];
            let blend = lab.cache.blend(&mix).unwrap().color("color_bg_app");
            assert_eq!(read_theme(vm, "color_bg_app"), blend, "the mix never went on");

            // The app's own picker: a base theme set, and the reload that
            // carries it. Nothing here knows the lab exists.
            crate::set_base_theme(vm.cx_mut(), BaseTheme::Light);
            vm.cx_mut().request_style_reload();
            the_reload_lands(vm);
            let light = read_theme(vm, "color_bg_app");
            assert_ne!(light, blend, "the blend went back over the theme just picked");

            // ...and the word the panel owes the lab when a rebuild lands,
            // which is where the lab finds out. It must not reinstall.
            lab.invalidate();
            let rebuilds = lab.rebuilds();
            assert_eq!(
                lab.apply(vm).unwrap(),
                Applied::Nothing,
                "the lab put its mix back over somebody else's pick"
            );
            assert_eq!(lab.rebuilds(), rebuilds, "and spent a rebuild doing it");
            assert!(
                !vm.cx_mut().pending_style_reload,
                "the stand-down asked for a reload of its own"
            );
            assert_eq!(read_theme(vm, "color_bg_app"), light, "the pick did not stay");
            assert!(!lab.installed, "the lab still believes its mix is on the app");

            // The section is open on the theme that was picked, so that is
            // what the weights start from and what leaving hands back.
            assert!(lab.is_open(), "the stand-down folded the section away");
            assert_eq!(lab.entry.as_ref().unwrap().theme, BlendTheme::Base(Scheme::Light));
            lab.leave(vm);
            assert!(!vm.cx_mut().pending_style_reload, "leaving an uninstalled lab rebuilt anyway");
            assert_eq!(read_theme(vm, "color_bg_app"), light, "leaving took the pick away");
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
            the_reload_lands(vm);
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
    /// and not the first pair. The near miss the table reaches first is the
    /// one nobody would notice; the second voice on the highest container, at
    /// 1.40 against a bar of 3, is the one that has gone.
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

    /// The panel measures whatever `held_pairs` hands it, so its table and the
    /// library's cannot drift: there is one table. What can still go missing
    /// is a pair the blend carries no value for -- `readability` skips one of
    /// those rather than failing, and a skipped pair is a pair nobody has a
    /// number for, which is how the inverse page went unmeasured the first
    /// time. So this enters on the real fifteen and counts what the reading
    /// measured against what the library holds.
    ///
    /// It pays for the full resolve, which no other test here does. That is
    /// the point of it: made-up themes carry the tokens the test wrote into
    /// them, and would agree with a table naming any token at all.
    #[test]
    fn a_reading_measures_every_pair_the_library_holds() {
        let held = held_pairs();
        // A table that came back short would leave everything below agreeing
        // with nothing.
        assert!(held.len() >= 17, "only {} pairs came out of the library's table", held.len());
        assert!(
            held.contains(&("color_inverse_surface", "color_inverse_on_surface", READABLE)),
            "the inverse page is the pair this check exists for"
        );
        // A group the library only MEASURES must not reach the bar a mix is
        // failed against: the loading block is one, and every shipped theme
        // fails it.
        assert!(
            !held.iter().any(|(_, ink, _)| *ink == "color_placeholder"),
            "a group the library does not hold itself to is being held against a mix"
        );

        let mut lab = ThemeLab::new();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            desktop_style::uninstall(vm);
            lab.enter(vm);
            let reading = lab.readability();
            assert_eq!(
                reading.measured,
                held.len(),
                "a pair the library holds carried no value into the blend and was skipped: {reading:?}"
            );
        });
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
        lab.set_appearance(Appearance::Light);
        assert!(lab.rows().is_empty(), "a closed lab grew a row");
        assert_eq!(lab.appearance(), Appearance::Dark, "and moved the group it says is on show");
        assert_eq!(lab.readability().measured, 0, "and measured a mix of nothing");
        let mut cx = Cx::new(Box::new(|_, _| {}));
        assert_eq!(cx.with_vm(|vm| lab.apply(vm)), Ok(Applied::Nothing));
        assert_eq!(lab.rebuilds(), 0, "a closed lab rebuilt the module");

        // An empty cache is not what makes any of that true, which matters
        // because the cache a panel has is the warm one: the resolved themes
        // are kept across a leave, so a closed lab that HAS been entered can
        // blend, and a group drawn over it would measure a mix nothing is
        // wearing and read as if it were on the screen.
        let mut warm = entered();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| warm.leave(vm));
        assert!(!warm.is_open());
        assert_eq!(warm.cache().len(), BlendTheme::all().len(), "the themes were not kept");
        warm.set_appearance(Appearance::Light);
        warm.randomize(0x5EED);
        assert!(warm.rows().is_empty(), "a closed lab with the themes in hand drew a group");
        assert_eq!(warm.readability().measured, 0, "and measured a mix that is not in force");
        // What it does still do is remember: the mode is the one it will open
        // on next, and a panel may set it while the section is folded.
        warm.set_mode(WeightMode::Relative);
        assert_eq!(warm.mode(), WeightMode::Relative, "a closed lab forgot the mode");

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
