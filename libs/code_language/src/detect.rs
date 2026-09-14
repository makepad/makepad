//! Path and bounded-content language detection. Overrides and trusted context
//! belong to the caller; this module implements extension then content.

use crate::id::{Detection, DetectionSource, Dialect, LanguageId};

/// Extensions that belong in a mixed-language source inventory.
pub fn inventory_extensions() -> &'static [&'static str] {
    &[
        "rs", "toml", "cc", "cpp", "cxx", "c++", "h", "hh", "hpp", "hxx", "inl", "ipp", "tpp", "c",
        "m", "mm", "py", "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "cs", "csx", "html",
        "htm", "xhtml", "css", "pyi", "pyw", "java", "splash", "xml", "xsd", "xsl", "xslt", "plist",
        "svg", "md", "markdown",
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
    // `*.d.ts` is TypeScript dialect "dts"; checked before the generic `.ts` match.
    if filename_ends_ignore(path, ".d.ts") {
        return Detection {
            language: LanguageId::TypeScript,
            dialect: dialect(LanguageId::TypeScript, "dts"),
            source: DetectionSource::Extension,
            note: "typescript declaration file",
        };
    }

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
    if eq_ignore(ext, "pyi") {
        return Detection {
            language: LanguageId::Python,
            dialect: dialect(LanguageId::Python, "stub"),
            source: DetectionSource::Extension,
            note: "python stub",
        };
    }
    if eq_ignore(ext, "py") || eq_ignore(ext, "pyw") {
        return Detection {
            language: LanguageId::Python,
            dialect: Dialect::default_for(LanguageId::Python),
            source: DetectionSource::Extension,
            note: "python source",
        };
    }
    if eq_ignore(ext, "js") || eq_ignore(ext, "mjs") || eq_ignore(ext, "cjs") {
        return Detection {
            language: LanguageId::JavaScript,
            dialect: Dialect::default_for(LanguageId::JavaScript),
            source: DetectionSource::Extension,
            note: "javascript source",
        };
    }
    if eq_ignore(ext, "jsx") {
        return Detection {
            language: LanguageId::JavaScript,
            dialect: dialect(LanguageId::JavaScript, "jsx"),
            source: DetectionSource::Extension,
            note: "javascript jsx",
        };
    }
    if eq_ignore(ext, "ts") || eq_ignore(ext, "mts") || eq_ignore(ext, "cts") {
        return Detection {
            language: LanguageId::TypeScript,
            dialect: Dialect::default_for(LanguageId::TypeScript),
            source: DetectionSource::Extension,
            note: "typescript source",
        };
    }
    if eq_ignore(ext, "tsx") {
        return Detection {
            language: LanguageId::TypeScript,
            dialect: dialect(LanguageId::TypeScript, "tsx"),
            source: DetectionSource::Extension,
            note: "typescript tsx",
        };
    }
    if eq_ignore(ext, "cs") {
        return Detection {
            language: LanguageId::CSharp,
            dialect: Dialect::default_for(LanguageId::CSharp),
            source: DetectionSource::Extension,
            note: "csharp source",
        };
    }
    if eq_ignore(ext, "csx") {
        return Detection {
            language: LanguageId::CSharp,
            dialect: dialect(LanguageId::CSharp, "script"),
            source: DetectionSource::Extension,
            note: "csharp script",
        };
    }
    if eq_ignore(ext, "html") || eq_ignore(ext, "htm") {
        return Detection {
            language: LanguageId::Html,
            dialect: Dialect::default_for(LanguageId::Html),
            source: DetectionSource::Extension,
            note: "html document",
        };
    }
    if eq_ignore(ext, "xhtml") {
        return Detection {
            language: LanguageId::Html,
            dialect: dialect(LanguageId::Html, "xhtml"),
            source: DetectionSource::Extension,
            note: "xhtml document",
        };
    }
    if eq_ignore(ext, "css") {
        return Detection {
            language: LanguageId::Css,
            dialect: Dialect::default_for(LanguageId::Css),
            source: DetectionSource::Extension,
            note: "css stylesheet",
        };
    }
    if filename_ends_ignore(path, "module-info.java") {
        let name = path.rsplit('/').next().unwrap_or(path);
        if name.len() == "module-info.java".len() {
            return Detection {
                language: LanguageId::Java,
                dialect: dialect(LanguageId::Java, "module"),
                source: DetectionSource::Extension,
                note: "java module descriptor",
            };
        }
    }
    if eq_ignore(ext, "java") {
        return Detection {
            language: LanguageId::Java,
            dialect: Dialect::default_for(LanguageId::Java),
            source: DetectionSource::Extension,
            note: "java source",
        };
    }
    if eq_ignore(ext, "splash") {
        return Detection {
            language: LanguageId::Splash,
            dialect: Dialect::default_for(LanguageId::Splash),
            source: DetectionSource::Extension,
            note: "splash script",
        };
    }
    if eq_ignore(ext, "xml")
        || eq_ignore(ext, "xsd")
        || eq_ignore(ext, "xsl")
        || eq_ignore(ext, "xslt")
        || eq_ignore(ext, "plist")
    {
        return Detection {
            language: LanguageId::Xml,
            dialect: Dialect::default_for(LanguageId::Xml),
            source: DetectionSource::Extension,
            note: "xml document",
        };
    }
    if eq_ignore(ext, "svg") {
        return Detection {
            language: LanguageId::Svg,
            dialect: Dialect::default_for(LanguageId::Svg),
            source: DetectionSource::Extension,
            note: "svg document (source)",
        };
    }
    if eq_ignore(ext, "md") || eq_ignore(ext, "markdown") {
        return Detection {
            language: LanguageId::Markdown,
            dialect: Dialect::default_for(LanguageId::Markdown),
            source: DetectionSource::Extension,
            note: "markdown document",
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

fn filename_ends_ignore(path: &str, suffix: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let nb = name.as_bytes();
    let sb = suffix.as_bytes();
    nb.len() >= sb.len()
        && nb[nb.len() - sb.len()..]
            .iter()
            .zip(sb)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
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

pub fn is_script_family(language: LanguageId) -> bool {
    matches!(language, LanguageId::JavaScript | LanguageId::TypeScript)
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
            | LanguageId::Python
            | LanguageId::JavaScript
            | LanguageId::TypeScript
            | LanguageId::CSharp
            | LanguageId::Html
            | LanguageId::Css
            | LanguageId::Java
    )
}

/// Text formats that are inventory-eligible and have a tokens-only frontend.
/// Distinct from [`has_compiled_frontend`]: these languages produce tokens and
/// line summaries but no declarations, imports, calls or joins.
pub fn is_text_format(language: LanguageId) -> bool {
    matches!(
        language,
        LanguageId::Splash | LanguageId::Xml | LanguageId::Svg | LanguageId::Markdown
    )
}
