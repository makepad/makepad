use crate::makepad_regex;
use makepad_regex::ParseOptions;
use makepad_regex::Regex as InnerRegex;

/// Tag for GC marking of regex objects (same pattern as StringTag)
#[derive(Default)]
pub struct RegexTag(u64);

impl RegexTag {
    const MARK: u64 = 0x1;
    const STATIC: u64 = 0x2;

    pub fn is_marked(&self) -> bool {
        self.0 & Self::MARK != 0
    }

    pub fn set_mark(&mut self) {
        self.0 |= Self::MARK
    }

    pub fn clear_mark(&mut self) {
        self.0 &= !Self::MARK
    }

    pub fn set_static(&mut self) {
        self.0 |= Self::STATIC
    }

    pub fn is_static(&self) -> bool {
        self.0 & Self::STATIC != 0
    }
}

/// Flags parsed from the flags string "gims"
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RegexFlags {
    pub global: bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
}

impl RegexFlags {
    pub fn parse(flags: &str) -> Result<Self, String> {
        let mut f = RegexFlags::default();
        for ch in flags.chars() {
            match ch {
                'g' => {
                    if f.global {
                        return Err("duplicate flag 'g'".into());
                    }
                    f.global = true;
                }
                'i' => {
                    if f.ignore_case {
                        return Err("duplicate flag 'i'".into());
                    }
                    f.ignore_case = true;
                }
                'm' => {
                    if f.multiline {
                        return Err("duplicate flag 'm'".into());
                    }
                    f.multiline = true;
                }
                's' => {
                    if f.dot_all {
                        return Err("duplicate flag 's'".into());
                    }
                    f.dot_all = true;
                }
                _ => return Err(format!("unknown regex flag '{}'", ch)),
            }
        }
        Ok(f)
    }

    pub fn to_parse_options(&self) -> ParseOptions {
        ParseOptions {
            dot_all: self.dot_all,
            ignore_case: self.ignore_case,
            multiline: self.multiline,
        }
    }
}

/// The key used for interning: (pattern, flags)
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RegexInternKey {
    pub pattern: String,
    pub flags: RegexFlags,
}

/// Data stored per regex on the heap
pub struct ScriptRegexData {
    pub tag: RegexTag,
    pub inner: InnerRegex,
    pub pattern: String,
    pub flags: RegexFlags,
    /// Number of capture groups (not counting group 0)
    pub num_captures: usize,
}

impl ScriptRegexData {
    pub fn new(pattern: &str, flags: RegexFlags) -> Result<Self, String> {
        let options = flags.to_parse_options();
        let inner = InnerRegex::new_with_options(pattern, options).map_err(|e| e.to_string())?;
        let num_captures = count_captures(pattern);
        Ok(ScriptRegexData {
            tag: RegexTag::default(),
            inner,
            pattern: pattern.to_string(),
            flags,
            num_captures,
        })
    }
}

/// Count the number of capturing groups in a pattern.
/// Counts unescaped '(' that are NOT followed by '?'
fn count_captures(pattern: &str) -> usize {
    let mut count = 0;
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2; // skip escaped char
            continue;
        }
        if bytes[i] == b'[' {
            // skip character class contents
            i += 1;
            while i < bytes.len() && bytes[i] != b']' {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
        }
        if bytes[i] == b'(' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'?' {
                // non-capturing or lookahead - don't count
            } else {
                count += 1;
            }
        }
        i += 1;
    }
    count
}
