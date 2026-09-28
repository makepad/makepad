//! Themes the person saved, and the rule that a shipped one is permanent.
//!
//! The library ships three base themes ([`Scheme`]) and a style sheet per
//! desktop ([`DesktopStyle`]). Those are BUILT-IN: they live in the binary,
//! the picker always offers them, and nothing here can remove or overwrite
//! one. Everything else is a SAVED theme -- a snapshot of whatever was in
//! force when the person pressed "save as", written to a file under
//! [`themes_dir`] and offered in the same picker below the built-ins. Saved
//! themes are the only ones [`delete`] will touch, and the check that says so
//! lives in this module rather than in the panel, so a caller cannot delete a
//! shipped theme by getting the UI wrong.
//!
//! # What a saved theme is
//!
//! A [`SavedTheme`] is a name, the base theme it derives from, the style
//! sheet that was installed when it was taken (so the fonts and the widget
//! re-skins come back with it), and every colour and number that was in
//! `mod.theme` at that moment. Applying one is
//! [`theme_module_script`]`(name, base, overrides)` -- the only sanctioned
//! override path in [`crate::theme_tokens`] -- so a saved theme is exactly as
//! legitimate as the equalizer's output, which builds the same script through
//! [`crate::theme_tokens::ThemeBlend::script`].
//!
//! What a snapshot does NOT carry is everything in a theme that is neither a
//! colour nor a number: a `TextStyle`, an `Ease`, the `Inset` of `mspace_1`.
//! Those have no literal to write, so they come from the base theme the
//! script derives from, the same way [`crate::theme_tokens::BlendCache`]
//! leaves them to the argmax theme. This is why a snapshot records its sheet:
//! re-install the sheet, then run the script, and the parts a script cannot
//! carry are back before the tokens are pinned over them.
//!
//! # Names
//!
//! A name is used twice: as a file name under [`themes_dir`], and as a script
//! identifier in `mod.themes.<name>`. So it is the intersection of both --
//! see [`is_valid_name`]. [`normalize_name`] turns what a person types into
//! one, and the panel is expected to call it before showing the name back.
//!
//! # Where the files are
//!
//! [`themes_dir`] is `<makepad home>/themes`, where the makepad home is the
//! one shared per-user state directory the whole workspace already agrees on
//! (`makepad_home`, which `MAKEPAD_HOME` redirects). A run that must leave
//! the person's own themes alone -- a test, a demo, a CI job -- points
//! `MAKEPAD_THEME_DIR` somewhere else instead, the same idiom the storybook's
//! settings file uses. Nothing here ever writes outside that directory: every
//! path is `dir.join(name + ".theme")` for a name that already passed
//! [`is_valid_name`], which admits no separator and no `..`.
//!
//! # Collisions
//!
//! Saving over a BUILT-IN name is refused outright, with no flag that allows
//! it ([`StoreError::Builtin`]). Saving over an existing SAVED theme is
//! refused by [`save`] ([`StoreError::Exists`]) so the panel can ask, and
//! goes through once the person has said yes with [`save_replacing`].
//!
//! # The `_in` twins
//!
//! Every filesystem call comes in two: the plain one, which works in
//! [`themes_dir`], and a `_in` one that takes the directory. The panel wants
//! the plain one. The `_in` ones are what the tests drive, so the tests need
//! no environment variable and cannot tread on each other or on the person's
//! own theme folder.

use std::path::{Path, PathBuf};

use crate::desktop_style::DesktopStyle;
use crate::makepad_platform::home::makepad_home;
use crate::theme_tokens::{theme_module_script, Scheme, TokenValue};
use crate::Cx;

/// The extension every saved theme file carries.
pub const FILE_EXTENSION: &str = "theme";

/// The first line of a saved theme file. The trailing number is the format
/// version: a reader that does not know a version refuses the file rather
/// than guessing at it.
pub const FORMAT_HEADER: &str = "makepad-theme 1";

/// The environment variable that moves the theme folder somewhere else.
pub const DIR_ENV: &str = "MAKEPAD_THEME_DIR";

/// Longest a theme name may be. Long enough for a sentence fragment, short
/// enough that the name plus the extension is a comfortable file name on
/// every platform the library builds for.
pub const MAX_NAME_LEN: usize = 48;

/// Names that are not a theme but would collide with one if they were saved.
/// `theme` and `themes` are the script module's own keys, `me` is what a
/// theme file calls itself, and `equalized` is the key the equalizer's blend
/// script writes into; `default` and `current` are reserved because a picker
/// that showed either next to a real theme would be lying about which it is.
pub const RESERVED_NAMES: [&str; 6] = ["theme", "themes", "me", "equalized", "default", "current"];

/// Every word the script language reserves, copied from the parser's own
/// `is_reserved_binding`.
///
/// A theme name is written into a script as `mod.themes.<name>`, so a name
/// that is a keyword is a name the reader will not accept: the file would be
/// written happily and then refuse to evaluate, and the theme would be
/// unusable and un-overwritable in one go. [`RESERVED_NAMES`] covers the
/// names the theme MODULE owns; this covers the names the LANGUAGE owns.
/// `me` is in both lists, from both directions, which costs nothing.
pub const RESERVED_KEYWORDS: [&str; 28] = [
    "me", "scope", "self", "nil", "true", "false", "ok", "let", "var", "mut", "fn", "if", "elif",
    "else", "for", "in", "while", "loop", "match", "return", "break", "continue", "and", "or",
    "is", "do", "try", "use",
];

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a store call did not do what was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The name is not one [`is_valid_name`] accepts. Carries the name as it
    /// was offered, so the panel can say which one it means.
    InvalidName(String),
    /// The name belongs to a theme the library ships. Saving over it and
    /// deleting it are both refused, permanently and with no override.
    Builtin(String),
    /// A saved theme of that name is already there. [`save_replacing`] is the
    /// call that goes ahead anyway.
    Exists(String),
    /// No saved theme of that name.
    NotFound(String),
    /// The file is there but is not a theme this version can read: a missing
    /// or unknown header, a line without a tab, a token name that is not an
    /// identifier, a value with a newline in it.
    Malformed(String),
    /// The filesystem said no. Carries the message, since the panel shows it
    /// and `std::io::Error` is not `PartialEq`.
    Io(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::InvalidName(name) => write!(
                f,
                "{name:?} is not a usable theme name: use letters, digits and underscores, starting with a letter"
            ),
            StoreError::Builtin(name) => {
                write!(f, "{name:?} is a theme the library ships and cannot be saved over or deleted")
            }
            StoreError::Exists(name) => write!(f, "a saved theme named {name:?} is already there"),
            StoreError::NotFound(name) => write!(f, "no saved theme named {name:?}"),
            StoreError::Malformed(why) => write!(f, "that theme file cannot be read: {why}"),
            StoreError::Io(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for StoreError {}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// Is this a name the store will accept?
///
/// It has to survive being a file name AND being the `<name>` of
/// `mod.themes.<name>`, so the rule is the tighter of the two: one to
/// [`MAX_NAME_LEN`] characters of ASCII lowercase letters, digits and
/// underscores, beginning with a letter. Upper case is out because two names
/// that differ only in case are one file on Windows and macOS; `-`, `.` and
/// every separator are out because they are not identifier characters, which
/// also means no name can reach out of [`themes_dir`].
///
/// This says nothing about whether the name is free -- a built-in name is
/// perfectly valid and still cannot be saved over. See [`is_builtin`].
pub fn is_valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// [`is_valid_name`] as a result, so a caller can `?` it.
pub fn validate_name(name: &str) -> Result<(), StoreError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(StoreError::InvalidName(name.to_string()))
    }
}

/// What a person typed, as a name the store accepts -- or `None` when there
/// is nothing left of it to use.
///
/// Lower cases, turns every run of characters a name may not hold into a
/// single underscore, drops the leading underscores and digits that would
/// stop it being an identifier, trims the trailing underscores, and cuts it
/// to [`MAX_NAME_LEN`]. "My Sunset " becomes `my_sunset`, "2000" becomes
/// `None`. The panel is expected to run this over the text field and show the
/// answer back, so the person sees the name they are actually saving.
pub fn normalize_name(input: &str) -> Option<String> {
    let mut out = String::with_capacity(input.len());
    let mut pending_gap = false;
    for c in input.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            // A leading digit cannot open an identifier, so nothing counts
            // until the first letter has landed.
            if out.is_empty() && !c.is_ascii_lowercase() {
                continue;
            }
            if pending_gap && !out.is_empty() {
                out.push('_');
            }
            pending_gap = false;
            out.push(c);
        } else if !out.is_empty() {
            pending_gap = true;
        }
    }
    while out.len() > MAX_NAME_LEN {
        out.pop();
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// The comparison key [`is_builtin`] works in: lower case, with `-` read as
/// `_`. The shipped sheets are named with hyphens (`windows-2000`) and a
/// saved name may not hold one, so without this fold a person could save
/// `windows_2000`, which is a different file but the SAME key in
/// `mod.themes` and would quietly shadow the sheet's theme.
fn fold(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('-', "_")
}

/// Every name the library itself owns: the three base themes, every style
/// sheet, each sheet's dark appearance, [`RESERVED_NAMES`] and
/// [`RESERVED_KEYWORDS`].
///
/// Read off the library's own lists rather than written out here, so a theme
/// or a sheet added there becomes undeletable without anybody remembering to
/// come back. The `-dark` form is listed for every sheet, including the ones
/// that do not offer a dark appearance today: reserving a name that is not in
/// use yet costs nothing, and a sheet that grows one later must not turn a
/// theme the person already saved into a collision.
pub fn builtin_names() -> Vec<String> {
    let mut out: Vec<String> = Scheme::ALL.iter().map(|s| s.theme_name().to_string()).collect();
    for style in DesktopStyle::ALL {
        out.push(style.id().to_string());
        out.push(format!("{}-dark", style.id()));
    }
    out.extend(RESERVED_NAMES.iter().map(|s| s.to_string()));
    out.extend(RESERVED_KEYWORDS.iter().map(|s| s.to_string()));
    out
}

/// Does this name belong to the library rather than to the person?
///
/// A built-in can never be saved over and can never be deleted. The check is
/// the folded one described at [`fold`], so neither case nor a hyphen swapped
/// for an underscore gets past it.
pub fn is_builtin(name: &str) -> bool {
    let name = fold(name);
    builtin_names().iter().any(|b| fold(b) == name)
}

/// May the person delete this theme? True only for a name that is valid, not
/// built-in, and actually saved. This is what greys the delete button out;
/// [`delete`] enforces the same rule itself and does not trust the answer.
pub fn can_delete(name: &str) -> bool {
    is_valid_name(name) && !is_builtin(name) && exists(name)
}

// ---------------------------------------------------------------------------
// A saved theme
// ---------------------------------------------------------------------------

/// A theme the person saved: what to derive from, what sheet to wear, and
/// every colour and number to pin over the result.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedTheme {
    /// The name, which is both the file's stem and the key in `mod.themes`.
    /// Always one [`is_valid_name`] accepts by the time the store has taken
    /// it; [`SavedTheme::new`] does not check, [`save`] does.
    pub name: String,
    /// The base theme the script derives from. Everything a token cannot
    /// carry -- text styles, easings, insets -- comes from here.
    pub base: Scheme,
    /// The style sheet that was installed when the snapshot was taken, with
    /// its appearance, or `None` for a bare base theme. Install this before
    /// running [`SavedTheme::script`]: the sheet brings the fonts and the
    /// widget re-skins, and the script then pins the tokens over it.
    pub sheet: Option<(DesktopStyle, bool)>,
    /// Every token the theme pins, sorted by name.
    pub overrides: Vec<(String, TokenValue)>,
}

impl SavedTheme {
    /// An empty theme over a base: no sheet, no tokens.
    pub fn new(name: &str, base: Scheme) -> Self {
        Self { name: name.to_string(), base, sheet: None, overrides: Vec::new() }
    }

    /// The script that makes this theme current, through the one sanctioned
    /// override path: `mod.themes.<name> = mod.themes.<base>{ ... }` followed
    /// by `mod.theme = mod.themes.<name>`. Evaluate it the way the equalizer
    /// evaluates [`crate::theme_tokens::ThemeBlend::script`] -- with the
    /// sheet from [`SavedTheme::sheet`] installed first -- and follow it with
    /// `cx.request_script_reapply()`.
    ///
    /// A theme whose DIMENSIONS moved -- its spacing, its roundness, its type
    /// size -- and that has no sheet under it derives from the base built
    /// again with those numbers in place, rather than from the base object:
    /// the insets and the text styles are objects the base file derives, a
    /// pin cannot carry one, and without this a theme saved roomy came back
    /// with the library's margins. See
    /// [`crate::theme_builder::rederived_pin_script`].
    pub fn script(&self) -> String {
        if self.sheet.is_none() {
            if let Some(script) =
                crate::theme_builder::rederived_pin_script(&self.name, self.base, &self.overrides)
            {
                return script;
            }
        }
        theme_module_script(&self.name, self.base.theme_name(), &self.overrides)
    }

    /// The sheet's name as [`crate::desktop_style::current_name`] reports it
    /// and [`crate::desktop_style::DesktopStyle::parse`] reads it back:
    /// `macos`, `macos-dark`.
    pub fn sheet_name(&self) -> Option<String> {
        self.sheet.map(|(style, dark)| {
            if dark {
                format!("{}-dark", style.id())
            } else {
                style.id().to_string()
            }
        })
    }

    /// The file's whole text, as [`SavedTheme::parse`] reads it: the header,
    /// then one `key<TAB>value` line per header field, then one
    /// `token<TAB>name<TAB>value` line per token, values rendered exactly as
    /// the script would read them ([`TokenValue::render`]).
    pub fn to_text(&self) -> String {
        let mut out = String::with_capacity(64 + self.overrides.len() * 32);
        out.push_str(FORMAT_HEADER);
        out.push('\n');
        out.push_str(&format!("name\t{}\n", self.name));
        out.push_str(&format!("base\t{}\n", self.base.theme_name()));
        if let Some(sheet) = self.sheet_name() {
            out.push_str(&format!("sheet\t{sheet}\n"));
        }
        // Sorted on the way out rather than trusted to be sorted already.
        // `parse` sorts what it reads, so writing in the caller's order would
        // make a save and a load disagree for a theme somebody assembled by
        // hand; and a settled order means the same theme is the same bytes,
        // which is what makes two theme files worth comparing.
        let mut tokens: Vec<&(String, TokenValue)> = self.overrides.iter().collect();
        tokens.sort_by(|a, b| a.0.cmp(&b.0));
        for (key, value) in tokens {
            out.push_str(&format!("token\t{key}\t{}\n", value.render()));
        }
        out
    }

    /// Read back what [`SavedTheme::to_text`] wrote.
    ///
    /// Strict about the things that would mislead -- an unknown header, a
    /// base that is not one of the three, a token name that is not an
    /// identifier -- and quiet about the things that cannot: a blank line, an
    /// unknown header key from a later version, a sheet name this build no
    /// longer ships (the theme then loads as its bare base, which is the
    /// honest answer rather than a refusal).
    pub fn parse(text: &str) -> Result<SavedTheme, StoreError> {
        let mut lines = text.lines();
        match lines.next().map(str::trim_end) {
            Some(first) if first == FORMAT_HEADER => {}
            Some(first) => {
                return Err(StoreError::Malformed(format!("expected {FORMAT_HEADER:?}, found {first:?}")))
            }
            None => return Err(StoreError::Malformed("the file is empty".to_string())),
        }
        let mut name: Option<String> = None;
        let mut base: Option<Scheme> = None;
        let mut sheet: Option<(DesktopStyle, bool)> = None;
        let mut overrides: Vec<(String, TokenValue)> = Vec::new();
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let Some((key, rest)) = line.split_once('\t') else {
                return Err(StoreError::Malformed(format!("line without a tab: {line:?}")));
            };
            match key {
                "name" => name = Some(rest.trim().to_string()),
                "base" => {
                    base = Scheme::ALL.into_iter().find(|s| s.theme_name() == rest.trim());
                    if base.is_none() {
                        return Err(StoreError::Malformed(format!("unknown base theme {:?}", rest.trim())));
                    }
                }
                "sheet" => {
                    let id = rest.trim();
                    // A sheet this build does not know is not an error: the
                    // theme's own tokens are all still there, and dropping
                    // the sheet loses only the re-skin.
                    sheet = DesktopStyle::parse(id).map(|style| (style, id.ends_with("-dark")));
                }
                "token" => {
                    let Some((token, value)) = rest.split_once('\t') else {
                        return Err(StoreError::Malformed(format!("token line without a value: {line:?}")));
                    };
                    if !is_token_key(token) {
                        return Err(StoreError::Malformed(format!("{token:?} is not a token name")));
                    }
                    let Some(parsed) = parse_value(value) else {
                        return Err(StoreError::Malformed(format!(
                            "{token}: {value:?} is neither a colour nor a number"
                        )));
                    };
                    overrides.push((token.to_string(), parsed));
                }
                // A key from a later version of the format. Skipping it is
                // what lets this version read that file at all.
                _ => {}
            }
        }
        let Some(name) = name else {
            return Err(StoreError::Malformed("no name line".to_string()));
        };
        let Some(base) = base else {
            return Err(StoreError::Malformed("no base line".to_string()));
        };
        validate_name(&name)?;
        overrides.sort_by(|a, b| a.0.cmp(&b.0));
        overrides.dedup_by(|a, b| a.0 == b.0);
        Ok(SavedTheme { name, base, sheet, overrides })
    }
}

/// Is this a token name a script would accept on the left of a colon? Token
/// names come out of `live_id_token`, so they already are; this is the check
/// that keeps a hand-edited file from writing something else into a script.
fn is_token_key(key: &str) -> bool {
    !key.is_empty()
        && key.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// One token value as the file holds it: `#x` and eight hex digits is a
/// colour, and anything that reads as a finite number is a number.
///
/// Nothing else is accepted, and that is the point. A value read from a file
/// is pasted into a generated script, so a value carried through verbatim is
/// a value that can END the object it sits in and write whatever it likes
/// after it -- including `mod.themes.dark = ...`, which replaces a built-in
/// theme outright. The store guards its NAMES carefully and would have been
/// walked round entirely through a value. A file that holds anything but a
/// colour or a number is malformed, and a malformed file is refused rather
/// than run.
fn parse_value(text: &str) -> Option<TokenValue> {
    if let Some(hex) = text.strip_prefix("#x") {
        if hex.len() == 8 {
            if let Ok(rgba) = u32::from_str_radix(hex, 16) {
                return Some(TokenValue::Color(rgba));
            }
        }
        return None;
    }
    match text.parse::<f64>() {
        Ok(number) if number.is_finite() => Some(TokenValue::Num(number)),
        _ => None,
    }
}

/// A theme whose text would not read back the way it was written, checked
/// before anything is opened for writing so a bad value is a refusal rather
/// than a file that fails on the next load.
///
/// Three ways a value can be unwritable. A line break is one: every line of
/// the format is one record, so a value holding one would read back as a
/// record of its own. A number that is not finite is the second -- `NaN` and
/// `inf` are what `f64` prints for those, and neither is a literal the script
/// reader accepts, so a theme carrying one would save happily and then refuse
/// to evaluate.
///
/// The third is anything [`parse_value`] would refuse on the way back in,
/// which is everything that is neither a colour nor a number: a
/// [`TokenValue::Raw`] holding an expression renders as that expression, and
/// a caller outside this module CAN hand one to [`save`] -- nothing inside it
/// produces one, since [`snapshot`] emits only colours and numbers and
/// [`SavedTheme::parse`] reads only colours and numbers. Refusing it here as
/// well as there means the poisoned value never reaches a file, rather than
/// reaching one and being refused every time it is read. The rule is
/// `parse_value` itself rather than a second copy of it, so the two ends
/// cannot drift: a `Raw` that renders as a plain number still writes, because
/// that is a value the reader accepts and the script it produces is the same
/// either way.
fn check_writable(theme: &SavedTheme) -> Result<(), StoreError> {
    for (key, value) in &theme.overrides {
        if !is_token_key(key) {
            return Err(StoreError::Malformed(format!("{key:?} is not a token name")));
        }
        if let TokenValue::Num(number) = value {
            if !number.is_finite() {
                return Err(StoreError::Malformed(format!(
                    "the value of {key:?} is {number}, which is not a number a script can read"
                )));
            }
        }
        let rendered = value.render();
        if rendered.contains('\n') || rendered.contains('\r') {
            return Err(StoreError::Malformed(format!("the value of {key:?} spans several lines")));
        }
        if parse_value(&rendered).is_none() {
            return Err(StoreError::Malformed(format!(
                "the value of {key:?} is {rendered:?}, which is neither a colour nor a number"
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The folder
// ---------------------------------------------------------------------------

/// Where saved themes live: `MAKEPAD_THEME_DIR` when it is set, otherwise
/// `themes` under the workspace's one per-user state directory, which
/// `MAKEPAD_HOME` already redirects. Nothing is created until the first save.
pub fn themes_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(DIR_ENV) {
        return PathBuf::from(dir);
    }
    makepad_home().join("themes")
}

/// The file a saved theme of this name would have. Errors on a name the
/// store does not accept, which is also what keeps the join inside `dir`.
pub fn path_in(dir: &Path, name: &str) -> Result<PathBuf, StoreError> {
    validate_name(name)?;
    Ok(dir.join(format!("{name}.{FILE_EXTENSION}")))
}

/// [`path_in`] in [`themes_dir`].
pub fn path_of(name: &str) -> Result<PathBuf, StoreError> {
    path_in(&themes_dir(), name)
}

// ---------------------------------------------------------------------------
// Listing, loading, saving, deleting
// ---------------------------------------------------------------------------

/// Every saved theme in `dir`, by name, in order.
///
/// `Ok` of nothing means there are no saved themes, and a folder that is not
/// there is one of those: it is where every workspace starts, and the first
/// save creates it. Every other failure is an `Err`, because it does not mean
/// the themes are gone, it means the store could not TELL -- a sync client
/// renaming the folder out from under it, a scanner holding a handle, a share
/// that blipped. On Windows those are a handle away and this runs on every
/// entry into the Theme tab.
///
/// The two used to be one answer, an empty `Vec` for both, and the caller
/// that drops the theme it is wearing when the wearer's name is not in the
/// list took "I could not tell" for "it is gone" and stripped a saved theme,
/// name and pins, off the screen with no undo. A `Result` is the smallest
/// thing that cannot be read the wrong way round: a caller that truly wants
/// an empty list out of a failure has to write `unwrap_or_default` and say so
/// where the next reader can see it.
pub fn list_in(dir: &Path) -> Result<Vec<String>, StoreError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(StoreError::Io(error.to_string())),
    };
    let mut out: Vec<String> = Vec::new();
    for entry in entries {
        // An entry that cannot be read is the same "could not tell" one file
        // further in: skipping it would hand back a list that is short by one
        // and looks exactly like a list somebody deleted from.
        let entry = entry.map_err(|error| StoreError::Io(error.to_string()))?;
        let path = entry.path();
        // Case-insensitively, because `exists_in` asks the FILESYSTEM and
        // Windows and macOS answer that `sunset.THEME` is `sunset.theme`.
        // Matched exactly, a file saved or dropped in under the upper-case
        // spelling would block every save of that name while never once
        // appearing in the list it blocks.
        let matches_ext = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(FILE_EXTENSION));
        if !matches_ext {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // A file somebody dropped in by hand under a name the store would not
        // have written is skipped: offering it would offer a name that cannot
        // be loaded, deleted or written back.
        if is_valid_name(stem) && !is_builtin(stem) {
            out.push(stem.to_string());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Every saved theme, by name, in order. What the picker lists under the
/// built-ins. [`list_in`] has what an `Err` means and why it is not a list.
pub fn list() -> Result<Vec<String>, StoreError> {
    list_in(&themes_dir())
}

/// Is there a saved theme of this name in `dir`? False for every built-in,
/// whatever is on disk: a built-in is not a saved theme.
pub fn exists_in(dir: &Path, name: &str) -> bool {
    if is_builtin(name) {
        return false;
    }
    match path_in(dir, name) {
        Ok(path) => path.is_file(),
        Err(_) => false,
    }
}

/// [`exists_in`] in [`themes_dir`].
pub fn exists(name: &str) -> bool {
    exists_in(&themes_dir(), name)
}

/// Read one saved theme out of `dir`.
pub fn load_in(dir: &Path, name: &str) -> Result<SavedTheme, StoreError> {
    validate_name(name)?;
    if is_builtin(name) {
        return Err(StoreError::Builtin(name.to_string()));
    }
    let path = path_in(dir, name)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(StoreError::NotFound(name.to_string()))
        }
        Err(error) => return Err(StoreError::Io(format!("{}: {error}", path.display()))),
    };
    let mut theme = SavedTheme::parse(&text)?;
    // The file name is the name the picker showed and the caller asked for;
    // a `name` line that disagrees is a file somebody renamed, and the file
    // name is the one that has to win or the picker would apply a theme the
    // person did not pick.
    theme.name = name.to_string();
    Ok(theme)
}

/// [`load_in`] from [`themes_dir`].
pub fn load(name: &str) -> Result<SavedTheme, StoreError> {
    load_in(&themes_dir(), name)
}

/// Write a theme into `dir`, refusing a name that is already taken.
///
/// A built-in name is [`StoreError::Builtin`] and stays refused whatever the
/// caller does next; an existing saved name is [`StoreError::Exists`], which
/// is the panel's cue to ask, and [`save_replacing_in`] is the answer to yes.
pub fn save_in(dir: &Path, theme: &SavedTheme) -> Result<PathBuf, StoreError> {
    if exists_in(dir, &theme.name) {
        return Err(StoreError::Exists(theme.name.clone()));
    }
    write_in(dir, theme)
}

/// [`save_in`] in [`themes_dir`].
pub fn save(theme: &SavedTheme) -> Result<PathBuf, StoreError> {
    save_in(&themes_dir(), theme)
}

/// Write a theme into `dir` over one already saved under that name. Still
/// refuses a built-in: no caller and no answer to any prompt makes a shipped
/// theme writable.
pub fn save_replacing_in(dir: &Path, theme: &SavedTheme) -> Result<PathBuf, StoreError> {
    write_in(dir, theme)
}

/// [`save_replacing_in`] in [`themes_dir`].
pub fn save_replacing(theme: &SavedTheme) -> Result<PathBuf, StoreError> {
    save_replacing_in(&themes_dir(), theme)
}

/// The write both save paths share, and the one place the built-in rule is
/// enforced on the way in.
fn write_in(dir: &Path, theme: &SavedTheme) -> Result<PathBuf, StoreError> {
    validate_name(&theme.name)?;
    if is_builtin(&theme.name) {
        return Err(StoreError::Builtin(theme.name.clone()));
    }
    check_writable(theme)?;
    let path = path_in(dir, &theme.name)?;
    if let Err(error) = std::fs::create_dir_all(dir) {
        return Err(StoreError::Io(format!("{}: {error}", dir.display())));
    }
    write_atomically(&path, &theme.to_text())?;
    Ok(path)
}

/// The extension the not-yet-finished file wears while it is being written.
/// Deliberately not [`FILE_EXTENSION`], so one left behind by a machine that
/// lost power mid-save is invisible to [`list_in`] rather than being offered
/// as a theme that will not load.
const TEMP_EXTENSION: &str = "tmp";

/// Put `text` at `path` without ever truncating what is already there.
///
/// The obvious `fs::write` opens the destination and truncates it, so a write
/// that dies halfway -- a full disk, a network share that went away, a killed
/// process -- leaves half a theme where a whole one used to be, and every
/// load after that refuses it. Instead the text goes to a file beside the
/// destination and is renamed over it once it is complete: until the rename
/// the old theme is untouched, and after it the new one is whole. There is no
/// moment at which the file on disk is a partial theme.
///
/// The rename is one step on unix. Windows refuses a rename onto a file that
/// exists, so there the old file is removed first -- but only once the new
/// text is safely on disk, so the worst an interruption can leave is the
/// theme missing rather than corrupt. That removal happens only when the
/// destination really is in the way, so a rename that failed for some other
/// reason cannot talk this into deleting a theme it was unable to replace.
fn write_atomically(path: &Path, text: &str) -> Result<(), StoreError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_name().and_then(|s| s.to_str()).unwrap_or(FILE_EXTENSION);
    // Unique per process and per call, so two saves at once -- two windows of
    // one app, tests under a parallel runner -- cannot share a temporary file
    // and write over each other's text.
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = dir.join(format!("{stem}.{}-{serial}.{TEMP_EXTENSION}", std::process::id()));

    if let Err(error) = std::fs::write(&temp, text) {
        let _ = std::fs::remove_file(&temp);
        return Err(StoreError::Io(format!("{}: {error}", temp.display())));
    }
    match std::fs::rename(&temp, path) {
        Ok(()) => return Ok(()),
        // The destination is in the way: the windows case, and the only one
        // worth clearing the way for.
        Err(_) if path.is_file() => {}
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            return Err(StoreError::Io(format!("{}: {error}", path.display())));
        }
    }
    if let Err(error) = std::fs::remove_file(path) {
        let _ = std::fs::remove_file(&temp);
        return Err(StoreError::Io(format!("{}: {error}", path.display())));
    }
    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(StoreError::Io(format!("{}: {error}", path.display())))
        }
    }
}

/// Remove a saved theme from `dir`.
///
/// A built-in is [`StoreError::Builtin`] and is refused here, in the store,
/// so no caller can delete a shipped theme by getting its own guard wrong.
/// A name that is not saved is [`StoreError::NotFound`].
pub fn delete_in(dir: &Path, name: &str) -> Result<(), StoreError> {
    validate_name(name)?;
    if is_builtin(name) {
        return Err(StoreError::Builtin(name.to_string()));
    }
    let path = path_in(dir, name)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(StoreError::NotFound(name.to_string()))
        }
        Err(error) => Err(StoreError::Io(format!("{}: {error}", path.display()))),
    }
}

/// [`delete_in`] in [`themes_dir`].
pub fn delete(name: &str) -> Result<(), StoreError> {
    delete_in(&themes_dir(), name)
}

// ---------------------------------------------------------------------------
// A person's own colour schemes
// ---------------------------------------------------------------------------

/// The file a person's own colour schemes go in, beside their saved themes.
///
/// A plain text file and not a theme file, because it holds none of a theme:
/// a line is two, three or four colours that somebody liked together, and
/// what the builder does with them is grow a palette from whichever of them
/// is nearest the colour that was picked. Plain text so that a list copied
/// out of a notebook, a screenshot or somebody else's page can be pasted
/// straight in.
pub const PALETTES_FILE: &str = "palettes.txt";

/// The schemes written in `dir`'s palettes file, and how many lines were
/// meant to be schemes and were not.
///
/// One scheme to a line: two to four colours as hex, separated by spaces or
/// commas, with or without a leading `#`. Three, six and eight hex digits are
/// all read; an alpha is dropped, because a palette colour is a hue and a
/// strength and a theme draws its accents solid. A blank line is nothing, and
/// a line opening `# ` or `//` is a note to the reader -- `#` before a hex
/// digit is a colour, which is why the comment form needs its space.
///
/// Nothing here is an error. A file that is not there is a person who has not
/// written one, and hands back no schemes; a line that does not parse is
/// skipped and counted, so a panel can say "three lines were not colours"
/// without the other forty being lost behind the first typo. That is the
/// whole reason this returns a count rather than a `Result`: the schemes are
/// what the caller wants and one bad line must never cost them.
pub fn read_palettes_in(dir: &Path) -> (Vec<Vec<u32>>, usize) {
    let Ok(text) = std::fs::read_to_string(dir.join(PALETTES_FILE)) else {
        return (Vec::new(), 0);
    };
    let mut schemes = Vec::new();
    let mut skipped = 0;
    // The byte order mark an editor may have put at the head of the file,
    // off before anything looks at a line. It is not whitespace by Rust's
    // reckoning, so a trim leaves it welded to the first word, and the first
    // scheme in the file would be read as a line that is not colours -- the
    // one line in forty that fails, which looks like nothing so much as a
    // typo the person cannot find.
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line == "#" || line.starts_with("# ") || line.starts_with("//") {
            continue;
        }
        match parse_palette(line) {
            Some(scheme) => schemes.push(scheme),
            None => skipped += 1,
        }
    }
    (schemes, skipped)
}

/// [`read_palettes_in`] in [`themes_dir`], which `MAKEPAD_THEME_DIR`
/// redirects like everything else in the folder.
pub fn read_palettes() -> (Vec<Vec<u32>>, usize) {
    read_palettes_in(&themes_dir())
}

/// One line of the palettes file. `None` for a line that is not two to four
/// colours, whether because one word was not a colour or because there were
/// too few or too many of them.
fn parse_palette(line: &str) -> Option<Vec<u32>> {
    let mut out: Vec<u32> = Vec::new();
    for word in line.split([' ', '\t', ',', ';']).filter(|word| !word.is_empty()) {
        out.push(parse_hex_color(word)?);
    }
    (2..=4).contains(&out.len()).then_some(out)
}

/// One colour as somebody writes one: `#1e90ff`, `1e90ff`, `#1EF`, or eight
/// digits with an alpha that is dropped.
fn parse_hex_color(word: &str) -> Option<u32> {
    let digits = word.strip_prefix('#').unwrap_or(word);
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    match digits.len() {
        // Each digit doubled, the way every short hex colour is read.
        3 => {
            let wide = |shift: u32| ((value >> shift) & 0xF) * 0x11;
            Some((wide(8) << 24) | (wide(4) << 16) | (wide(0) << 8) | 0xFF)
        }
        6 => Some((value << 8) | 0xFF),
        8 => Some(value | 0xFF),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Taking a snapshot of what is in force
// ---------------------------------------------------------------------------

/// Which base theme a snapshot derives from: the theme `mod.theme` is
/// ACTUALLY bound to, which is not always the app's own base theme.
///
/// A style sheet's first line rebinds `mod.theme` itself --
/// `mod.theme = mod.themes.light` -- and six of the twelve shipped sheets
/// are written against the light theme, while the panel lays every sheet
/// over the DARK base the way one arriving from the window manager is laid.
/// So while a sheet is installed the app's base theme says nothing about
/// what is on screen, and reading it here would record `dark` for a macOS,
/// Windows, Windows-2000, NeXTSTEP, iOS or Android theme.
///
/// That matters because a saved theme is applied as
/// `mod.themes.<name> = mod.themes.<base>{ ... }`: everything a token cannot
/// carry -- text styles, easings, insets -- comes from `<base>`, so the wrong
/// one restores the theme onto the wrong ground.
///
/// The answer is the blend engine's [`crate::theme_tokens::BlendTheme::base`],
/// which is the same table the equalizer mixes from and is held to the
/// sheets' own first lines by `the_sheet_table_is_what_the_sheets_say`. With
/// no sheet installed, `mod.theme` is whatever `theme_mod` last emitted, so
/// the app's base theme is the honest answer and is the one taken.
pub fn snapshot_base(sheet: Option<(DesktopStyle, bool)>, app_base: Scheme) -> Scheme {
    match sheet {
        Some((style, dark)) => crate::theme_tokens::BlendTheme::Sheet(style, dark).base(),
        None => app_base,
    }
}

/// The theme in force right now, as a [`SavedTheme`] under `name`. Nothing is
/// written: hand the answer to [`save`] or [`save_replacing`].
///
/// What is taken:
/// - the base theme [`snapshot_base`] settles on -- the installed sheet's
///   own, or the app's when there is no sheet;
/// - the installed sheet from [`crate::desktop_style::current_name`], read
///   the way the panel's own preset lookup reads it;
/// - every colour and number in `mod.theme` from
///   [`crate::tweaker::theme_values`], which reads the live script cascade --
///   so a value the person changed in the Theme tab a moment ago is in the
///   snapshot, and so is everything the installed sheet assigned over the
///   base.
///
/// What is not: every token that is neither a colour nor a number. See the
/// module note -- that is what the recorded sheet and base are for.
pub fn snapshot(cx: &mut Cx, name: &str) -> Result<SavedTheme, StoreError> {
    validate_name(name)?;
    if is_builtin(name) {
        return Err(StoreError::Builtin(name.to_string()));
    }
    let sheet = cx
        .with_vm(|vm| crate::desktop_style::current_name(vm))
        .and_then(|id| DesktopStyle::parse(&id).map(|style| (style, id.ends_with("-dark"))));
    let app_base = match crate::base_theme(cx) {
        crate::BaseTheme::Dark => Scheme::Dark,
        crate::BaseTheme::Light => Scheme::Light,
        crate::BaseTheme::Skeleton => Scheme::Skeleton,
    };
    let base = snapshot_base(sheet, app_base);
    let mut overrides: Vec<(String, TokenValue)> = crate::tweaker::theme_values(cx)
        .into_iter()
        .filter(|(key, _, _, _)| is_token_key(key))
        .map(|(key, _, value, _)| {
            let value = match value {
                crate::tweaker::ThemeVal::Color(c) => TokenValue::Color(c),
                crate::tweaker::ThemeVal::Num(n) => TokenValue::Num(n),
            };
            (key, value)
        })
        .collect();
    overrides.sort_by(|a, b| a.0.cmp(&b.0));
    overrides.dedup_by(|a, b| a.0 == b.0);
    Ok(SavedTheme { name: name.to_string(), base, sheet, overrides })
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A directory of this test's own, so the tests need no environment
    /// variable, cannot tread on each other under a parallel runner, and
    /// never go near the person's own theme folder.
    pub(super) fn scratch(tag: &str) -> PathBuf {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("makepad-theme-store-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn sample(name: &str) -> SavedTheme {
        SavedTheme {
            name: name.to_string(),
            base: Scheme::Dark,
            sheet: Some((DesktopStyle::Macos, true)),
            overrides: vec![
                ("color_text".to_string(), TokenValue::Color(0xFF_EE_DD_CC)),
                ("space_factor".to_string(), TokenValue::Num(1.25)),
            ],
        }
    }

    #[test]
    fn a_name_is_a_filename_and_an_identifier() {
        for good in ["sunset", "a", "my_sunset", "theme2", "x_9_z"] {
            assert!(is_valid_name(good), "{good:?} should be a usable name");
        }
        for bad in [
            "",            // nothing to file it under
            "Sunset",      // one file with `sunset` on two of three platforms
            "my sunset",   // not an identifier
            "my-sunset",   // not an identifier either
            "2000",        // cannot open an identifier
            "_sunset",     // ditto, and invisible in a listing
            "sun.set",     // an extension, not a name
            "a/b",         // a path
            "../evil",     // a path out of the folder
            "sünset",      // not ASCII
        ] {
            assert!(!is_valid_name(bad), "{bad:?} should be refused");
        }
        let long = "a".repeat(MAX_NAME_LEN);
        assert!(is_valid_name(&long));
        assert!(!is_valid_name(&format!("{long}a")));
    }

    #[test]
    fn normalizing_what_a_person_types() {
        assert_eq!(normalize_name("My Sunset "), Some("my_sunset".to_string()));
        assert_eq!(normalize_name("windows/2000"), Some("windows_2000".to_string()));
        assert_eq!(normalize_name("  Deep   Blue  "), Some("deep_blue".to_string()));
        assert_eq!(normalize_name("2000 AD"), Some("ad".to_string()));
        assert_eq!(normalize_name("2000"), None);
        assert_eq!(normalize_name("   "), None);
        assert_eq!(normalize_name("!!!"), None);
        let long = normalize_name(&"ab".repeat(MAX_NAME_LEN)).unwrap();
        assert_eq!(long.len(), MAX_NAME_LEN);
        assert!(is_valid_name(&long));
        // Whatever comes out is a name the store will take.
        for typed in ["My Sunset", "windows 2000", "a-b-c", "Ünder Ground"] {
            let name = normalize_name(typed).unwrap();
            assert!(is_valid_name(&name), "{typed:?} normalized to {name:?}");
        }
    }

    #[test]
    fn every_shipped_theme_is_builtin() {
        for scheme in Scheme::ALL {
            assert!(is_builtin(scheme.theme_name()), "{:?}", scheme);
        }
        for style in DesktopStyle::ALL {
            assert!(is_builtin(style.id()), "{:?}", style);
            assert!(is_builtin(&format!("{}-dark", style.id())), "{:?} dark", style);
        }
        for reserved in RESERVED_NAMES {
            assert!(is_builtin(reserved), "{reserved:?}");
        }
        // The fold: neither case nor an underscore for a hyphen gets past it,
        // because both would be the same key in `mod.themes`.
        assert!(is_builtin("Dark"));
        assert!(is_builtin("windows_2000"));
        assert!(is_builtin("black_orange"));
        assert!(!is_builtin("sunset"));
        assert!(!is_builtin("my_windows_2000"));
    }

    #[test]
    fn a_builtin_can_never_be_saved_over_or_deleted() {
        let dir = scratch("builtin");
        for name in ["dark", "light", "skeleton", "macos", "windows_2000", "nextstep", "theme"] {
            let theme = sample(name);
            assert_eq!(save_in(&dir, &theme), Err(StoreError::Builtin(name.to_string())));
            // The yes-I-mean-it path does not make one writable either.
            assert_eq!(save_replacing_in(&dir, &theme), Err(StoreError::Builtin(name.to_string())));
            assert_eq!(delete_in(&dir, name), Err(StoreError::Builtin(name.to_string())));
            assert_eq!(load_in(&dir, name), Err(StoreError::Builtin(name.to_string())));
            assert!(!exists_in(&dir, name));
        }
        assert!(list_in(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_over_a_saved_theme_needs_the_replacing_call() {
        let dir = scratch("collide");
        let mut theme = sample("sunset");
        assert!(save_in(&dir, &theme).is_ok());
        assert_eq!(save_in(&dir, &theme), Err(StoreError::Exists("sunset".to_string())));
        // ... and the first write is untouched by the refusal.
        assert_eq!(load_in(&dir, "sunset").unwrap().overrides, theme.overrides);
        theme.overrides = vec![("color_text".to_string(), TokenValue::Color(0x11_22_33_44))];
        assert!(save_replacing_in(&dir, &theme).is_ok());
        assert_eq!(load_in(&dir, "sunset").unwrap().overrides, theme.overrides);
        assert_eq!(list_in(&dir).unwrap(), vec!["sunset".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_invalid_name_never_reaches_the_disk() {
        let dir = scratch("invalid");
        for bad in ["", "My Sunset", "../evil", "a/b", "sun.set"] {
            let theme = sample(bad);
            assert_eq!(save_in(&dir, &theme), Err(StoreError::InvalidName(bad.to_string())));
            assert_eq!(delete_in(&dir, bad), Err(StoreError::InvalidName(bad.to_string())));
            assert_eq!(load_in(&dir, bad), Err(StoreError::InvalidName(bad.to_string())));
        }
        assert!(!dir.exists(), "a refused save must not create the folder");
    }

    #[test]
    fn saving_loading_and_deleting_one() {
        let dir = scratch("round");
        assert!(list_in(&dir).unwrap().is_empty());
        assert_eq!(load_in(&dir, "sunset"), Err(StoreError::NotFound("sunset".to_string())));
        assert_eq!(delete_in(&dir, "sunset"), Err(StoreError::NotFound("sunset".to_string())));
        let theme = sample("sunset");
        let path = save_in(&dir, &theme).unwrap();
        assert!(path.is_file());
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some(FILE_EXTENSION));
        assert!(exists_in(&dir, "sunset"));
        assert_eq!(load_in(&dir, "sunset").unwrap(), theme);
        assert_eq!(delete_in(&dir, "sunset"), Ok(()));
        assert!(!exists_in(&dir, "sunset"));
        assert!(list_in(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder that is not there is no saved themes. A folder the store
    /// cannot read is not an answer at all, and must not arrive looking like
    /// the first one: the panel drops the theme it is wearing, and its pins,
    /// when the name it wears is missing from the list.
    ///
    /// The unreadable case is a path that is a FILE, which is the one
    /// `read_dir` failure a test can make happen on every platform without a
    /// sync client or a scanner. It is the same `read_dir` call and the same
    /// arm as the handle that really does this.
    #[test]
    fn a_folder_that_cannot_be_read_is_not_a_folder_with_nothing_in_it() {
        let dir = scratch("unreadable");
        assert_eq!(list_in(&dir), Ok(Vec::new()), "a folder nobody has saved into yet");

        std::fs::create_dir_all(&dir).unwrap();
        let not_a_dir = dir.join("sunset.theme");
        std::fs::write(&not_a_dir, sample("sunset").to_text()).unwrap();
        assert_eq!(list_in(&dir), Ok(vec!["sunset".to_string()]), "and one that has");
        match list_in(&not_a_dir) {
            Err(StoreError::Io(why)) => assert!(!why.is_empty(), "the reason is what the panel shows"),
            other => panic!("a file read as a folder answered {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_listing_is_sorted_and_skips_what_it_cannot_use() {
        let dir = scratch("list");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["zulu", "alpha", "mike"] {
            save_in(&dir, &sample(name)).unwrap();
        }
        // Things a hand cannot be stopped from dropping in the folder.
        std::fs::write(dir.join("notes.txt"), "not a theme").unwrap();
        std::fs::write(dir.join("Sunset.theme"), sample("sunset").to_text()).unwrap();
        std::fs::write(dir.join("dark.theme"), sample("mike").to_text()).unwrap();
        assert_eq!(list_in(&dir).unwrap(), vec!["alpha".to_string(), "mike".to_string(), "zulu".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_text_round_trips() {
        let theme = sample("sunset");
        let text = theme.to_text();
        assert!(text.starts_with(FORMAT_HEADER));
        assert!(text.contains("base\tdark\n"));
        assert!(text.contains("sheet\tmacos-dark\n"));
        assert!(text.contains("token\tcolor_text\t#xFFEEDDCC\n"));
        assert_eq!(SavedTheme::parse(&text).unwrap(), theme);
        // A theme with no sheet writes no sheet line and reads back as one.
        let bare = SavedTheme {
            name: "bare".to_string(),
            base: Scheme::Light,
            sheet: None,
            overrides: vec![("space_factor".to_string(), TokenValue::Num(1.5))],
        };
        assert!(!bare.to_text().contains("sheet\t"));
        assert_eq!(SavedTheme::parse(&bare.to_text()).unwrap(), bare);
        // A `Raw` deliberately does NOT round trip: `parse_value` admits a
        // colour or a number and nothing else, so an expression that reached
        // a file by hand is refused rather than pasted into a script. This
        // half of the test asserted the opposite and had been failing since.
        let raw = SavedTheme {
            name: "bare".to_string(),
            base: Scheme::Light,
            sheet: None,
            overrides: vec![("space_factor".to_string(), TokenValue::Raw("theme.space_1".to_string()))],
        };
        assert!(matches!(SavedTheme::parse(&raw.to_text()), Err(StoreError::Malformed(_))));
    }

    #[test]
    fn a_file_that_is_not_a_theme_is_refused_not_guessed_at() {
        assert!(matches!(SavedTheme::parse(""), Err(StoreError::Malformed(_))));
        assert!(matches!(SavedTheme::parse("makepad-theme 2\n"), Err(StoreError::Malformed(_))));
        assert!(matches!(
            SavedTheme::parse(&format!("{FORMAT_HEADER}\nname\tsunset\n")),
            Err(StoreError::Malformed(_))
        ));
        assert!(matches!(
            SavedTheme::parse(&format!("{FORMAT_HEADER}\nbase\tdark\n")),
            Err(StoreError::Malformed(_))
        ));
        assert!(matches!(
            SavedTheme::parse(&format!("{FORMAT_HEADER}\nname\tsunset\nbase\tpurple\n")),
            Err(StoreError::Malformed(_))
        ));
        assert!(matches!(
            SavedTheme::parse(&format!("{FORMAT_HEADER}\nname\tsunset\nbase\tdark\nnotabhere\n")),
            Err(StoreError::Malformed(_))
        ));
        assert!(matches!(
            SavedTheme::parse(&format!("{FORMAT_HEADER}\nname\tsunset\nbase\tdark\ntoken\tone two\t1.0\n")),
            Err(StoreError::Malformed(_))
        ));
        // A name in the file that the store would not have written.
        assert!(matches!(
            SavedTheme::parse(&format!("{FORMAT_HEADER}\nname\tMy Sunset\nbase\tdark\n")),
            Err(StoreError::InvalidName(_))
        ));
        // A key from a later version, and a sheet this build does not ship:
        // both are read past rather than refused.
        let text = format!("{FORMAT_HEADER}\nname\tsunset\nbase\tdark\nfuture\twhatever\nsheet\tbeos\n");
        let theme = SavedTheme::parse(&text).unwrap();
        assert_eq!(theme.name, "sunset");
        assert_eq!(theme.sheet, None);
    }

    /// The trailing `true` is part of the script and not decoration. The
    /// panel evaluates this text as it stands, and the VM drops the last
    /// statement of a body it parsed from text -- see `theme_module_script`,
    /// whose own test drives a VM over both shapes.
    #[test]
    fn the_script_is_the_sanctioned_override_path() {
        let theme = sample("sunset");
        let script = theme.script();
        assert_eq!(
            script,
            "mod.themes.sunset = mod.themes.dark{ color_text: #xFFEEDDCC space_factor: 1.25 }\nmod.theme = mod.themes.sunset\ntrue\n"
        );
        assert_eq!(theme.sheet_name(), Some("macos-dark".to_string()));
        assert_eq!(SavedTheme::new("bare", Scheme::Light).sheet_name(), None);
    }

    #[test]
    fn a_value_that_would_not_read_back_is_refused_at_the_save() {
        let dir = scratch("multiline");
        let theme = SavedTheme {
            name: "sunset".to_string(),
            base: Scheme::Dark,
            sheet: None,
            overrides: vec![("color_text".to_string(), TokenValue::Raw("mix(\n  a, b)".to_string()))],
        };
        assert!(matches!(save_in(&dir, &theme), Err(StoreError::Malformed(_))));
        assert!(!dir.exists());
    }

    /// The hole is shut at BOTH ends. `parse_value` refuses an expression on
    /// the way in; this is the same refusal on the way out, so a caller
    /// outside this module cannot put one in a file for a later reader to
    /// refuse. A `Raw` that renders as a plain number still writes: that is a
    /// value the reader accepts.
    #[test]
    fn a_value_the_reader_would_refuse_is_refused_at_the_save_too() {
        let dir = scratch("raw-save");
        for poison in [
            "Ease.Linear",
            "theme.color_text",
            "1.0 } mod.themes.dark = mod.themes.skeleton{ x: 1.0",
        ] {
            let theme = SavedTheme {
                name: "sunset".to_string(),
                base: Scheme::Dark,
                sheet: None,
                overrides: vec![("color_text".to_string(), TokenValue::Raw(poison.to_string()))],
            };
            assert!(matches!(save_in(&dir, &theme), Err(StoreError::Malformed(_))), "{poison:?}");
            assert!(matches!(save_replacing_in(&dir, &theme), Err(StoreError::Malformed(_))), "{poison:?}");
        }
        assert!(!dir.exists(), "nothing was written");
        // A colour, a number, and a `Raw` that is one, all still save.
        for value in [
            TokenValue::Color(0xFFEEDDCC),
            TokenValue::Num(1.25),
            TokenValue::Raw("1.5".to_string()),
        ] {
            let theme = SavedTheme {
                name: "sunset".to_string(),
                base: Scheme::Dark,
                sheet: None,
                overrides: vec![("space_factor".to_string(), value.clone())],
            };
            save_replacing_in(&dir, &theme).unwrap_or_else(|e| panic!("{value:?}: {e}"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_number_a_script_could_not_read_is_refused_at_the_save() {
        let dir = scratch("nonfinite");
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let theme = SavedTheme {
                name: "sunset".to_string(),
                base: Scheme::Dark,
                sheet: None,
                overrides: vec![("space_factor".to_string(), TokenValue::Num(bad))],
            };
            assert!(matches!(save_in(&dir, &theme), Err(StoreError::Malformed(_))), "{bad}");
        }
        assert!(!dir.exists());
    }

    #[test]
    fn a_refused_write_leaves_the_saved_theme_whole() {
        let dir = scratch("keep");
        let good = sample("sunset");
        save_in(&dir, &good).unwrap();
        // Every way a write can be refused, tried over a theme already there.
        let broken = [
            TokenValue::Raw("mix(\n  a, b)".to_string()),
            TokenValue::Num(f64::NAN),
        ];
        for value in broken {
            let theme = SavedTheme {
                name: "sunset".to_string(),
                base: Scheme::Light,
                sheet: None,
                overrides: vec![("color_text".to_string(), value)],
            };
            assert!(matches!(save_replacing_in(&dir, &theme), Err(StoreError::Malformed(_))));
            // Not truncated, not half replaced, not the new base: untouched.
            assert_eq!(load_in(&dir, "sunset").unwrap(), good);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_leaves_nothing_behind_but_the_theme() {
        let dir = scratch("tidy");
        save_in(&dir, &sample("sunset")).unwrap();
        let mut replaced = sample("sunset");
        replaced.base = Scheme::Light;
        replaced.sheet = None;
        save_replacing_in(&dir, &replaced).unwrap();
        // The write goes through a file beside the destination; when it is
        // done that file is gone, so the folder holds one theme and no
        // leftovers for the picker to trip over.
        let files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(files, vec![format!("sunset.{FILE_EXTENSION}")]);
        assert_eq!(load_in(&dir, "sunset").unwrap(), replaced);
        assert_eq!(list_in(&dir).unwrap(), vec!["sunset".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_theme_file_a_person_edited_by_hand_still_reads() {
        // Opened in an editor on windows and saved back, which turns every
        // line ending into a carriage return and a newline, and tends to
        // leave a blank line at the end.
        let text = format!(
            "{FORMAT_HEADER}\r\nname\tsunset\r\nbase\tdark\r\nsheet\tmacos-dark\r\n\r\ntoken\tcolor_text\t#xFFEEDDCC\r\n\r\n"
        );
        let theme = SavedTheme::parse(&text).unwrap();
        assert_eq!(theme.name, "sunset");
        assert_eq!(theme.base, Scheme::Dark);
        assert_eq!(theme.sheet, Some((DesktopStyle::Macos, true)));
        assert_eq!(theme.overrides, vec![("color_text".to_string(), TokenValue::Color(0xFF_EE_DD_CC))]);
    }

    #[test]
    fn the_file_name_is_the_name_that_wins() {
        let dir = scratch("renamed");
        std::fs::create_dir_all(&dir).unwrap();
        // Somebody renamed the file in a file manager, so the `name` line
        // inside it is stale. The picker offered the file name, so that is
        // the theme that has to come back -- under that name, or the script
        // would define a theme the person did not ask for.
        std::fs::write(dir.join(format!("evening.{FILE_EXTENSION}")), sample("sunset").to_text()).unwrap();
        let theme = load_in(&dir, "evening").unwrap();
        assert_eq!(theme.name, "evening");
        assert!(theme.script().starts_with("mod.themes.evening ="));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_delete_guard_answers_without_needing_the_folder() {
        // `can_delete` is what greys the button out, and for a built-in or a
        // name that is not a name it must answer no on the name alone --
        // before it ever looks at a folder, which in a test is the person's
        // own.
        for never in ["dark", "light", "skeleton", "macos", "windows-2000", "theme", "current"] {
            assert!(!can_delete(never), "{never:?} must never be deletable");
        }
        for never in ["", "My Sunset", "../evil", "a/b"] {
            assert!(!can_delete(never), "{never:?} must never be deletable");
        }
    }

    #[test]
    fn awkward_numbers_survive_the_file() {
        let dir = scratch("numbers");
        let overrides: Vec<(String, TokenValue)> = [
            ("a_tenth", 0.1),
            ("a_whole", 1.0),
            ("huge", 1e20),
            ("negative", -12.5),
            ("tiny", 1e-7),
            ("zero", 0.0),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), TokenValue::Num(v)))
        .collect();
        let theme = SavedTheme { name: "numbers".to_string(), base: Scheme::Dark, sheet: None, overrides };
        save_in(&dir, &theme).unwrap();
        assert_eq!(load_in(&dir, "numbers").unwrap(), theme);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_theme_is_written_in_a_settled_order() {
        let dir = scratch("order");
        let theme = SavedTheme {
            name: "sunset".to_string(),
            base: Scheme::Dark,
            sheet: None,
            overrides: vec![
                ("zebra".to_string(), TokenValue::Num(1.0)),
                ("alpha".to_string(), TokenValue::Num(2.0)),
                ("middle".to_string(), TokenValue::Num(3.0)),
            ],
        };
        save_in(&dir, &theme).unwrap();
        let read_back = load_in(&dir, "sunset").unwrap();
        // Same tokens, in the order the struct promises -- so a save and a
        // load agree even about a theme that was assembled by hand.
        assert_eq!(
            read_back.overrides,
            vec![
                ("alpha".to_string(), TokenValue::Num(2.0)),
                ("middle".to_string(), TokenValue::Num(3.0)),
                ("zebra".to_string(), TokenValue::Num(1.0)),
            ]
        );
        // Written twice, the same theme is the same bytes.
        assert_eq!(theme.to_text(), read_back.to_text());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_raw_value_that_reads_as_a_number_comes_back_as_one() {
        // The file holds the script literal, not which rust variant produced
        // it, so a `Raw` holding a bare number is a `Num` on the way back.
        // That is only worth knowing because the thing that matters -- the
        // script the theme applies -- is identical either way.
        let theme = SavedTheme {
            name: "sunset".to_string(),
            base: Scheme::Dark,
            sheet: None,
            overrides: vec![("space_factor".to_string(), TokenValue::Raw("1.5".to_string()))],
        };
        let read_back = SavedTheme::parse(&theme.to_text()).unwrap();
        assert_eq!(read_back.overrides, vec![("space_factor".to_string(), TokenValue::Num(1.5))]);
        assert_eq!(read_back.script(), theme.script());
    }

    #[test]
    fn a_folder_that_is_not_there_is_no_themes_rather_than_an_error() {
        let dir = scratch("absent");
        assert!(!dir.exists());
        assert!(list_in(&dir).unwrap().is_empty());
        assert!(!exists_in(&dir, "sunset"));
        assert_eq!(load_in(&dir, "sunset"), Err(StoreError::NotFound("sunset".to_string())));
        assert_eq!(delete_in(&dir, "sunset"), Err(StoreError::NotFound("sunset".to_string())));
        // Asking never creates it; the first save does.
        assert!(!dir.exists());
        save_in(&dir, &sample("sunset")).unwrap();
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A theme saved while a LIGHT-based sheet is on comes back onto the
    /// light base, not the dark one the panel installed the sheet over.
    ///
    /// The whole round trip, because the defect was only visible across it:
    /// the base is chosen, written, read back and rendered into the script
    /// that applies the theme.
    #[test]
    fn a_theme_saved_under_a_light_sheet_restores_onto_the_light_base() {
        let dir = scratch("light-sheet");
        // macOS in its light appearance, while the panel has the app on the
        // dark base -- which is exactly what `apply_theme_preset` leaves.
        let sheet = Some((DesktopStyle::Macos, false));
        let base = snapshot_base(sheet, Scheme::Dark);
        assert_eq!(base, Scheme::Light, "a light sheet derives from light");
        let theme = SavedTheme {
            name: "sunset".to_string(),
            base,
            sheet,
            overrides: vec![("color_text".to_string(), TokenValue::Color(0x11223344))],
        };
        save_in(&dir, &theme).unwrap();
        let back = load_in(&dir, "sunset").unwrap();
        assert_eq!(back.base, Scheme::Light);
        assert_eq!(back.sheet, sheet);
        assert!(
            back.script().contains("mod.themes.sunset = mod.themes.light{"),
            "{}",
            back.script()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every sheet, both appearances: the snapshot's base is the sheet's own
    /// and never the app's. Driven off the same table the blend engine mixes
    /// from, so a sheet that changes its first line moves both together.
    #[test]
    fn a_snapshot_takes_the_sheets_base_and_not_the_apps() {
        for style in DesktopStyle::ALL {
            for dark in [false, true] {
                if dark && !style.supports_dark() {
                    continue;
                }
                let sheet = Some((style, dark));
                let wanted = crate::theme_tokens::BlendTheme::Sheet(style, dark).base();
                // Whatever the app's own base is, the sheet's wins.
                for app in Scheme::ALL {
                    assert_eq!(
                        snapshot_base(sheet, app),
                        wanted,
                        "{}{}",
                        style.id(),
                        if dark { "-dark" } else { "" }
                    );
                }
            }
        }
        // ...and with no sheet on, the app's base is the only answer there
        // is, so it is the one taken.
        for app in Scheme::ALL {
            assert_eq!(snapshot_base(None, app), app);
        }
    }

    /// A name the script language owns is refused, or it would be written
    /// and then fail to evaluate.
    #[test]
    fn a_script_keyword_is_not_a_theme_name() {
        let dir = scratch("keywords");
        for word in RESERVED_KEYWORDS {
            assert!(is_builtin(word), "{word} is a script keyword");
            assert!(!can_delete(word), "{word}");
            let theme = SavedTheme::new(word, Scheme::Dark);
            assert_eq!(save_in(&dir, &theme), Err(StoreError::Builtin(word.to_string())));
        }
        // The spot checks the review named, so a shrunken list still fails.
        for word in ["let", "fn", "use", "if", "match", "true", "nil"] {
            assert!(is_builtin(word), "{word}");
        }
        assert!(!dir.exists(), "nothing was written");
    }

    /// `list_in` and `exists_in` have to agree about what is on disk. They
    /// did not: the extension was matched exactly while the filesystem
    /// matches it case-insensitively, so an upper-case file blocked a save
    /// and never showed up in the list.
    #[test]
    fn the_extension_is_matched_the_way_the_filesystem_matches_it() {
        let dir = scratch("ext-case");
        std::fs::create_dir_all(&dir).unwrap();
        let theme = sample("sunset");
        std::fs::write(dir.join("sunset.THEME"), theme.to_text()).unwrap();
        assert_eq!(list_in(&dir).unwrap(), vec!["sunset".to_string()]);
        // A different extension is still not a theme.
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();
        assert_eq!(list_in(&dir).unwrap(), vec!["sunset".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod injection_tests {
    use super::*;

    /// A value read from a file is pasted into a generated script, so a value
    /// carried through verbatim could close the object it sits in and write
    /// whatever it liked after it. The store guards its names with care; this
    /// is the door round the back, and it stays shut.
    #[test]
    fn a_saved_theme_cannot_smuggle_a_script_past_the_value_parser() {
        let poison = "makepad-theme 1
name	sunset
base	dark
token	color_text	1.0 } mod.themes.dark = mod.themes.skeleton{ x: 1.0
";
        let err = SavedTheme::parse(poison).unwrap_err();
        assert!(matches!(err, StoreError::Malformed(_)), "{err:?}");
        // and the ordinary shapes still read
        let good = "makepad-theme 1
name	sunset
base	dark
token	color_text	#xFFEEDDCC
token	space_factor	1.25
";
        let t = SavedTheme::parse(good).expect("a colour and a number are a theme");
        assert_eq!(t.overrides.len(), 2);
    }

    #[test]
    fn a_value_that_is_not_a_colour_or_a_number_is_refused() {
        assert!(parse_value("#xFFEEDDCC").is_some());
        assert!(parse_value("1.25").is_some());
        assert!(parse_value("-3").is_some());
        for bad in ["theme.color_text", "#xZZZZZZZZ", "#xFFF", "inf", "NaN", "", "1.0 } x"] {
            assert!(parse_value(bad).is_none(), "{bad:?} should not parse");
        }
    }
}

#[cfg(test)]
mod palette_tests {
    use super::tests::scratch;
    use super::*;

    /// A person's own schemes, written the several ways a person writes
    /// them: hashes or not, commas or spaces, three digits or six or eight.
    /// The lines that are not schemes are counted and stepped over, and the
    /// forty that are do not go down with them.
    #[test]
    fn palettes_are_read_a_line_at_a_time() {
        let dir = scratch("palettes");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(PALETTES_FILE),
            "# my own, from the notebook\r\n\
             \r\n\
             #1e90ff #ffa500 #2f4f4f\r\n\
             1E90FF, FFA500\r\n\
             #f0a, 0b3, 39c, ccc\r\n\
             // the next one has an alpha on it\r\n\
             #1e90ff80 #ffa50040\r\n\
             #\r\n\
             not a colour at all\r\n\
             #1e90ff\r\n\
             112233 445566 778899 aabbcc ddeeff\r\n\
             #1e90ff #gggggg\r\n",
        )
        .unwrap();
        let (schemes, skipped) = read_palettes_in(&dir);
        assert_eq!(
            schemes,
            vec![
                vec![0x1E90FFFF, 0xFFA500FF, 0x2F4F4FFF],
                vec![0x1E90FFFF, 0xFFA500FF],
                vec![0xFF00AAFF, 0x00BB33FF, 0x3399CCFF, 0xCCCCCCFF],
                // An alpha is dropped: a palette colour is a hue and a
                // strength, and a theme draws its accents solid.
                vec![0x1E90FFFF, 0xFFA500FF],
            ]
        );
        // One colour, five colours, a word, a bad digit -- four lines that
        // were meant to be schemes; the comments and the blank were not.
        assert_eq!(skipped, 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// No file is no schemes and no complaint: it is where everybody starts,
    /// and a builder that offered nothing of its own would be right.
    #[test]
    fn no_palettes_file_is_no_schemes() {
        let dir = scratch("no-palettes");
        assert_eq!(read_palettes_in(&dir), (Vec::new(), 0));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_palettes_in(&dir), (Vec::new(), 0));
        std::fs::write(dir.join(PALETTES_FILE), "# nothing but a note\r\n\r\n").unwrap();
        assert_eq!(read_palettes_in(&dir), (Vec::new(), 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file an editor put a byte order mark on is still a palettes file,
    /// and the first line of it is still a scheme.
    ///
    /// The mark is not whitespace, so a trim leaves it welded to the first
    /// word; the word is then not a comment and not hex, and the scheme goes
    /// down as a line that is not colours. It is the FIRST line every time,
    /// which makes the loss look arbitrary to somebody whose other forty
    /// lines read perfectly.
    #[test]
    fn a_byte_order_mark_does_not_cost_the_first_scheme() {
        let dir = scratch("bom-palettes");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(PALETTES_FILE),
            "\u{FEFF}#1e90ff #ffa500 #2f4f4f\r\n#f0a #0b3\r\n",
        )
        .unwrap();
        assert_eq!(
            read_palettes_in(&dir),
            (
                vec![vec![0x1E90FFFF, 0xFFA500FF, 0x2F4F4FFF], vec![0xFF00AAFF, 0x00BB33FF]],
                0
            )
        );
        // And a mark over a file whose first line is a note leaves the note
        // a note rather than making it the one line that is not colours.
        std::fs::write(dir.join(PALETTES_FILE), "\u{FEFF}// from the notebook\r\n#f0a #0b3\r\n").unwrap();
        assert_eq!(read_palettes_in(&dir), (vec![vec![0xFF00AAFF, 0x00BB33FF]], 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
