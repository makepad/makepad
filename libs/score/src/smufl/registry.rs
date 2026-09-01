use super::json::{
    child_path, codepoint, field, object, parse, root_object, string, string_array,
};
use super::{SmuflError, SmuflResult};
use std::collections::{HashMap, HashSet};

/// One canonical entry from the SMuFL glyph-name registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphInfo {
    pub name: String,
    pub codepoint: char,
    pub description: String,
}

/// Bidirectional canonical glyph-name and Unicode-codepoint registry.
#[derive(Clone, Debug, Default)]
pub struct GlyphRegistry {
    by_name: HashMap<String, GlyphInfo>,
    by_codepoint: HashMap<char, String>,
}

impl GlyphRegistry {
    /// Loads the canonical `glyphnames.json` shape from caller-owned bytes.
    pub fn from_bytes(bytes: &[u8]) -> SmuflResult<Self> {
        const ROOT: &str = "glyphNames";
        let value = parse(bytes)?;
        let root = root_object(&value, ROOT)?;
        let mut registry = Self::default();

        for (name, value) in root {
            let glyph_path = child_path(ROOT, name);
            let glyph = object(value, &glyph_path)?;
            let codepoint_path = child_path(&glyph_path, "codepoint");
            let codepoint = codepoint(field(glyph, "codepoint", &glyph_path)?, &codepoint_path)?;
            let description = string(
                field(glyph, "description", &glyph_path)?,
                &child_path(&glyph_path, "description"),
            )?
            .to_string();
            if let Some(first_name) = registry.by_codepoint.get(&codepoint) {
                return Err(SmuflError::DuplicateCodepoint {
                    codepoint,
                    first_name: first_name.clone(),
                    second_name: name.to_string(),
                });
            }

            registry.by_codepoint.insert(codepoint, name.to_string());
            registry.by_name.insert(
                name.to_string(),
                GlyphInfo {
                    name: name.to_string(),
                    codepoint,
                    description,
                },
            );
        }
        Ok(registry)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn glyph(&self, canonical_name: &str) -> Option<&GlyphInfo> {
        self.by_name.get(canonical_name)
    }

    pub fn glyph_for_codepoint(&self, codepoint: char) -> Option<&GlyphInfo> {
        self.name_for_codepoint(codepoint)
            .and_then(|name| self.glyph(name))
    }

    pub fn codepoint_for_name(&self, canonical_name: &str) -> Option<char> {
        self.glyph(canonical_name).map(|glyph| glyph.codepoint)
    }

    pub fn name_for_codepoint(&self, codepoint: char) -> Option<&str> {
        self.by_codepoint.get(&codepoint).map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = &GlyphInfo> {
        self.by_name.values()
    }
}

/// A named range from `ranges.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphRange {
    pub name: String,
    pub description: String,
    pub glyphs: Vec<String>,
    pub start: char,
    pub end: char,
}

impl GlyphRange {
    pub fn contains_codepoint(&self, codepoint: char) -> bool {
        (self.start..=self.end).contains(&codepoint)
    }

    pub fn contains_glyph(&self, canonical_name: &str) -> bool {
        self.glyphs.iter().any(|glyph| glyph == canonical_name)
    }
}

/// Named SMuFL ranges with reverse lookup from glyph name to ranges.
#[derive(Clone, Debug, Default)]
pub struct GlyphRanges {
    by_name: HashMap<String, GlyphRange>,
    by_glyph: HashMap<String, Vec<String>>,
}

impl GlyphRanges {
    /// Loads the canonical `ranges.json` shape from caller-owned bytes.
    pub fn from_bytes(bytes: &[u8]) -> SmuflResult<Self> {
        const ROOT: &str = "ranges";
        let value = parse(bytes)?;
        let root = root_object(&value, ROOT)?;
        let mut ranges = Self::default();

        for (name, value) in root {
            let range_path = child_path(ROOT, name);
            let range = object(value, &range_path)?;
            let glyphs = string_array(
                field(range, "glyphs", &range_path)?,
                &child_path(&range_path, "glyphs"),
            )?;
            for glyph in &glyphs {
                ranges
                    .by_glyph
                    .entry(glyph.clone())
                    .or_default()
                    .push(name.to_string());
            }
            ranges.by_name.insert(
                name.to_string(),
                GlyphRange {
                    name: name.to_string(),
                    description: string(
                        field(range, "description", &range_path)?,
                        &child_path(&range_path, "description"),
                    )?
                    .to_string(),
                    glyphs,
                    start: codepoint(
                        field(range, "range_start", &range_path)?,
                        &child_path(&range_path, "range_start"),
                    )?,
                    end: codepoint(
                        field(range, "range_end", &range_path)?,
                        &child_path(&range_path, "range_end"),
                    )?,
                },
            );
        }
        for range_names in ranges.by_glyph.values_mut() {
            range_names.sort();
        }
        Ok(ranges)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn get(&self, range_name: &str) -> Option<&GlyphRange> {
        self.by_name.get(range_name)
    }

    pub fn ranges_for_glyph(&self, canonical_name: &str) -> &[String] {
        self.by_glyph
            .get(canonical_name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn contains(&self, range_name: &str, canonical_name: &str) -> bool {
        self.get(range_name)
            .is_some_and(|range| range.contains_glyph(canonical_name))
    }

    pub fn iter(&self) -> impl Iterator<Item = &GlyphRange> {
        self.by_name.values()
    }
}

/// Named semantic glyph classes with reverse lookup by canonical glyph name.
#[derive(Clone, Debug, Default)]
pub struct GlyphClasses {
    by_name: HashMap<String, HashSet<String>>,
    by_glyph: HashMap<String, Vec<String>>,
}

impl GlyphClasses {
    /// Loads the canonical `classes.json` shape from caller-owned bytes.
    pub fn from_bytes(bytes: &[u8]) -> SmuflResult<Self> {
        const ROOT: &str = "classes";
        let value = parse(bytes)?;
        let root = root_object(&value, ROOT)?;
        let mut classes = Self::default();

        for (name, value) in root {
            let class_path = child_path(ROOT, name);
            let glyphs: HashSet<_> = string_array(value, &class_path)?.into_iter().collect();
            for glyph in &glyphs {
                classes
                    .by_glyph
                    .entry(glyph.clone())
                    .or_default()
                    .push(name.to_string());
            }
            classes.by_name.insert(name.to_string(), glyphs);
        }
        for class_names in classes.by_glyph.values_mut() {
            class_names.sort();
        }
        Ok(classes)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn glyphs(&self, class_name: &str) -> Option<&HashSet<String>> {
        self.by_name.get(class_name)
    }

    pub fn classes_for_glyph(&self, canonical_name: &str) -> &[String] {
        self.by_glyph
            .get(canonical_name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn contains(&self, class_name: &str, canonical_name: &str) -> bool {
        self.glyphs(class_name)
            .is_some_and(|glyphs| glyphs.contains(canonical_name))
    }

    /// Convenience lookup used by notation code that needs notehead behavior.
    pub fn is_notehead(&self, canonical_name: &str) -> bool {
        self.contains("noteheads", canonical_name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &HashSet<String>)> {
        self.by_name
            .iter()
            .map(|(name, glyphs)| (name.as_str(), glyphs))
    }
}
