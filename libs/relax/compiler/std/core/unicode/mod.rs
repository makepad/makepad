//! Unicode properties and case mappings for char/str (tables generated from real std).

pub(crate) mod tables;

pub use tables::UNICODE_VERSION;

/// Binary search in sorted, non-overlapping inclusive ranges.
pub(crate) fn in_ranges(t: &[(u32, u32)], c: u32) -> bool {
    let mut lo = 0usize;
    let mut hi = t.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let (a, b) = t[mid];
        if c < a {
            hi = mid;
        } else if c > b {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

pub(crate) fn is_alphabetic(c: char) -> bool {
    in_ranges(&tables::ALPHABETIC, c as u32)
}
pub(crate) fn is_lowercase(c: char) -> bool {
    in_ranges(&tables::LOWERCASE, c as u32)
}
pub(crate) fn is_uppercase(c: char) -> bool {
    in_ranges(&tables::UPPERCASE, c as u32)
}
pub(crate) fn is_white_space(c: char) -> bool {
    in_ranges(&tables::WHITE_SPACE, c as u32)
}
pub(crate) fn is_numeric(c: char) -> bool {
    in_ranges(&tables::NUMERIC, c as u32)
}
pub(crate) fn is_printable(c: char) -> bool {
    in_ranges(&tables::PRINTABLE, c as u32)
}
/// A printable grapheme extender (escaped by escape_debug where extenders are escaped).
pub(crate) fn is_grapheme_extend_printable(c: char) -> bool {
    in_ranges(&tables::GRAPHEME_EXTEND_PRINTABLE, c as u32)
}
pub(crate) fn is_case_ignorable(c: char) -> bool {
    in_ranges(&tables::CASE_IGNORABLE, c as u32)
}
/// Cased (for chars that are not case-ignorable).
pub(crate) fn is_cased(c: char) -> bool {
    in_ranges(&tables::CASED, c as u32)
}

fn lookup_single(t: &[(u32, u32)], c: u32) -> Option<u32> {
    let mut lo = 0usize;
    let mut hi = t.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let (k, v) = t[mid];
        if c < k {
            hi = mid;
        } else if c > k {
            lo = mid + 1;
        } else {
            return Some(v);
        }
    }
    None
}

fn lookup_multi(t: &[(u32, [u32; 3])], c: u32) -> Option<[u32; 3]> {
    let mut lo = 0usize;
    let mut hi = t.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let (k, v) = t[mid];
        if c < k {
            hi = mid;
        } else if c > k {
            lo = mid + 1;
        } else {
            return Some(v);
        }
    }
    None
}

fn to_chars(v: [u32; 3]) -> [char; 3] {
    let mut out = ['\0'; 3];
    let mut i = 0;
    while i < 3 {
        out[i] = unsafe { crate::char::from_u32_unchecked(v[i]) };
        i += 1;
    }
    out
}

/// Lowercase mapping: [a, '\0', '\0'] for one char, else up to three chars.
pub(crate) fn to_lower(c: char) -> [char; 3] {
    let cp = c as u32;
    if cp < 0x80 {
        let l = if cp >= 0x41 && cp <= 0x5a { cp + 32 } else { cp };
        return [unsafe { crate::char::from_u32_unchecked(l) }, '\0', '\0'];
    }
    if let Some(v) = lookup_single(&tables::TO_LOWER_SINGLE, cp) {
        return [unsafe { crate::char::from_u32_unchecked(v) }, '\0', '\0'];
    }
    if let Some(v) = lookup_multi(&tables::TO_LOWER_MULTI, cp) {
        return to_chars(v);
    }
    [c, '\0', '\0']
}

/// Uppercase mapping: [a, '\0', '\0'] for one char, else up to three chars.
pub(crate) fn to_upper(c: char) -> [char; 3] {
    let cp = c as u32;
    if cp < 0x80 {
        let u = if cp >= 0x61 && cp <= 0x7a { cp - 32 } else { cp };
        return [unsafe { crate::char::from_u32_unchecked(u) }, '\0', '\0'];
    }
    if let Some(v) = lookup_single(&tables::TO_UPPER_SINGLE, cp) {
        return [unsafe { crate::char::from_u32_unchecked(v) }, '\0', '\0'];
    }
    if let Some(v) = lookup_multi(&tables::TO_UPPER_MULTI, cp) {
        return to_chars(v);
    }
    [c, '\0', '\0']
}
