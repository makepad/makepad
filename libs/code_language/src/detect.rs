//! Path and bounded-content language detection. Overrides and trusted context
//! belong to the caller; this module implements extension then content.

use crate::id::{Detection, DetectionSource, Dialect, LanguageId};

/// Extensions that belong in a mixed-language source inventory. Includes
/// languages without a compiled-in frontend (Python) so they remain visible.
pub fn inventory_extensions() -> &'static [&'static str] {
    &[
        "rs", "toml", "cc", "cpp", "cxx", "c++", "h", "hh", "hpp", "hxx", "inl", "ipp", "tpp", "c",
        "m", "mm", "py",
    ]
}

/// Extensions a C/C++ frontend recognises for analysis.
pub fn cpp_family_extensions() -> &'static [&'static str] {
    &[
        "cc", "cpp", "cxx", "c++", "h", "hh", "hpp", "hxx", "inl", "ipp", "tpp", "c", "m", "mm",
    ]
}

pub fn extension_of(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() && !ext.contains('.') => ext,
        _ => "",
    }
}

fn dialect(language: LanguageId, name: &'static str) -> Dialect {
    Dialect { language, name }
}

/// Detect from an unambiguous or documented-fallback filename. `bytes` is
/// consulted only when the extension is missing or still ambiguous and the
/// caller already has a bounded prefix.
pub fn detect_path(path: &str, bytes: Option<&[u8]>) -> Detection {
    let ext = extension_of(path);
    let ext_l = if ext.bytes().all(|b| b.is_ascii()) {
        let mut buf = [0u8; 8];
        let n = ext.len().min(8);
        for (i, b) in ext.bytes().take(n).enumerate() {
            buf[i] = b.to_ascii_lowercase();
        }
        // compared via str from the original when ascii
        Some(ext)
    } else {
        None
    };
    let lower_eq = |want: &str| {
        ext.len() == want.len()
            && ext
                .bytes()
                .zip(want.bytes())
                .all(|(a, b)| a.eq_ignore_ascii_case(&b))
    };
    let _ = (ext_l, lower_eq);

    if eq_ignore(ext, "rs") {
        return Detection {
            language: LanguageId::Rust,
            dialect: Dialect::default_for(LanguageId::Rust),
            source: DetectionSource::Extension,
            note: "rust source",
        };
    }
    if eq_ignore(ext, "toml") {
        return Detection {
            language: LanguageId::Toml,
            dialect: Dialect::default_for(LanguageId::Toml),
            source: DetectionSource::Extension,
            note: "toml document",
        };
    }
    if eq_ignore(ext, "cc")
        || eq_ignore(ext, "cpp")
        || eq_ignore(ext, "cxx")
        || eq_ignore(ext, "c++")
    {
        return Detection {
            language: LanguageId::Cpp,
            dialect: dialect(LanguageId::Cpp, "cxx"),
            source: DetectionSource::Extension,
            note: "c++ translation unit",
        };
    }
    if eq_ignore(ext, "hh")
        || eq_ignore(ext, "hpp")
        || eq_ignore(ext, "hxx")
        || eq_ignore(ext, "inl")
        || eq_ignore(ext, "ipp")
        || eq_ignore(ext, "tpp")
    {
        return Detection {
            language: LanguageId::Cpp,
            dialect: dialect(LanguageId::Cpp, "header"),
            source: DetectionSource::Extension,
            note: "c++ header",
        };
    }
    if eq_ignore(ext, "h") {
        return Detection {
            language: LanguageId::Cpp,
            dialect: dialect(LanguageId::Cpp, "ambiguous-header"),
            source: DetectionSource::Fallback,
            note: "header treated as c++ (ambiguous with c)",
        };
    }
    if eq_ignore(ext, "c") {
        return Detection {
            language: LanguageId::C,
            dialect: Dialect::default_for(LanguageId::C),
            source: DetectionSource::Extension,
            note: "c source; not claimed as c++",
        };
    }
    if eq_ignore(ext, "m") {
        return Detection {
            language: LanguageId::ObjectiveC,
            dialect: Dialect::default_for(LanguageId::ObjectiveC),
            source: DetectionSource::Extension,
            note: "objective-c source; not claimed as c++",
        };
    }
    if eq_ignore(ext, "mm") {
        return Detection {
            language: LanguageId::ObjectiveCpp,
            dialect: Dialect::default_for(LanguageId::ObjectiveCpp),
            source: DetectionSource::Extension,
            note: "objective-c++ source",
        };
    }
    if eq_ignore(ext, "py") {
        return Detection {
            language: LanguageId::Python,
            dialect: Dialect::default_for(LanguageId::Python),
            source: DetectionSource::Extension,
            note: "python source; no frontend is compiled in",
        };
    }
    if let Some(bytes) = bytes {
        if let Some(detected) = detect_content(bytes) {
            return detected;
        }
    }
    Detection::unknown()
}

fn eq_ignore(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).all(|(x, y)| x.eq_ignore_ascii_case(&y))
}

/// Bounded content detection used only when the filename is not decisive.
/// Looks at a short prefix; never treats a whole repository as evidence.
fn detect_content(bytes: &[u8]) -> Option<Detection> {
    let prefix = &bytes[..bytes.len().min(256)];
    let text = std::str::from_utf8(prefix).ok()?;
    let t = text.trim_start();
    if t.starts_with("#!") && t.contains("python") {
        return Some(Detection {
            language: LanguageId::Python,
            dialect: Dialect::default_for(LanguageId::Python),
            source: DetectionSource::Content,
            note: "python shebang",
        });
    }
    None
}

pub fn is_cpp_family(language: LanguageId) -> bool {
    matches!(
        language,
        LanguageId::Cpp | LanguageId::C | LanguageId::ObjectiveC | LanguageId::ObjectiveCpp
    )
}

pub fn has_compiled_frontend(language: LanguageId) -> bool {
    matches!(
        language,
        LanguageId::Rust
            | LanguageId::Toml
            | LanguageId::Cpp
            | LanguageId::C
            | LanguageId::ObjectiveC
            | LanguageId::ObjectiveCpp
    )
}
