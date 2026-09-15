//! Application font-set selection and the package/runtime font contract.
//!
//! The data lives below widgets so `app_main!` can install the selection before
//! an application's script module runs. `makepad-widgets` is the public theme
//! boundary for these types through its normal platform re-export.

/// Name of the wasm custom section (and native metadata section) emitted by
/// `app_main!`.
pub const FONT_ASSET_MANIFEST_SECTION: &str = "makepad.font-assets.v1";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FontSet {
    Latin,
    International,
}

impl FontSet {
    /// The compatibility-preserving target default. Web is deliberately small.
    pub const fn target_default() -> Self {
        Self::target_default_for_web(cfg!(target_arch = "wasm32"))
    }

    const fn target_default_for_web(is_web: bool) -> Self {
        if is_web { Self::Latin } else { Self::International }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Latin => "Latin",
            Self::International => "International",
        }
    }

    pub const fn policy(self) -> &'static FontPolicy {
        match self {
            Self::Latin => &LATIN_POLICY,
            Self::International => &INTERNATIONAL_POLICY,
        }
    }

    pub const fn manifest_bytes(self) -> &'static [u8] {
        match self {
            Self::Latin => LATIN_FONT_ASSET_MANIFEST,
            Self::International => INTERNATIONAL_FONT_ASSET_MANIFEST,
        }
    }
}

/// Font resources registered by the standard widgets module outside the
/// semantic theme roles. `app_main!` includes these in every emitted manifest.
pub const MATH_VIEW_FONT_ASSET: &str = "makepad_widgets/resources/NewCMMath-Regular.otf";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FontRole {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Monospace,
    Icons,
}

/// A large fallback family that is available to the Latin/UI set but loaded
/// only after shaping proves that the currently loaded faces miss a glyph.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LazyFontFamily {
    Cjk,
    Emoji,
}

impl LazyFontFamily {
    pub fn contains(self, ch: char) -> bool {
        let code = ch as u32;
        match self {
            Self::Cjk => {
                !Self::Emoji.contains(ch)
                    && matches!(
                        code,
                        0x2E80..=0x2FFF
                            | 0x3000..=0x303F
                            | 0x3040..=0x30FF
                            | 0x3100..=0x31BF
                            | 0x31C0..=0x31EF
                            | 0x31F0..=0x31FF
                            | 0x3200..=0x4DBF
                            | 0x4E00..=0x9FFF
                            | 0xAC00..=0xD7AF
                            | 0xF900..=0xFAFF
                            | 0x20000..=0x2FA1F
                    )
            }
            Self::Emoji => matches!(
                code,
                0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x20E3 | 0x2122 | 0x2139
                    | 0x231A..=0x231B
                    | 0x23E9..=0x23F3
                    | 0x23F8..=0x23FA
                    | 0x25AA..=0x25AB
                    | 0x25B6
                    | 0x25C0
                    | 0x25FB..=0x25FE
                    | 0x2600..=0x27BF
                    | 0x2934..=0x2935
                    | 0x2B05..=0x2B07
                    | 0x2B1B..=0x2B1C
                    | 0x2B50
                    | 0x2B55
                    | 0x3030
                    | 0x303D
                    | 0x3297
                    | 0x3299
                    | 0xFE0F
                    | 0x1F000..=0x1FAFF
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LazyFontAsset {
    pub family: LazyFontFamily,
    pub asset: FontAsset,
}

impl FontRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::Bold => "bold",
            Self::Italic => "italic",
            Self::BoldItalic => "bold_italic",
            Self::Monospace => "monospace",
            Self::Icons => "icons",
        }
    }
}

/// One stable logical resource in an ordered font chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontAsset {
    pub id: &'static str,
    pub resource_path: &'static str,
    pub ascender: f32,
    pub descender: f32,
    pub weight: f32,
}

/// Ordered fallback data for one semantic text role.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontChain {
    pub role: FontRole,
    pub members: &'static [FontAsset],
}

/// The single source of truth used to build theme families and package assets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontPolicy {
    pub set: FontSet,
    pub regular: FontChain,
    pub bold: FontChain,
    pub italic: FontChain,
    pub bold_italic: FontChain,
    pub monospace: FontChain,
    pub icons: FontChain,
    /// Exact de-duplicated union of every role chain, in package order.
    pub assets: &'static [FontAsset],
}

impl FontPolicy {
    pub const fn for_set(set: FontSet) -> &'static Self {
        set.policy()
    }

    pub const fn chain(&self, role: FontRole) -> FontChain {
        match role {
            FontRole::Regular => self.regular,
            FontRole::Bold => self.bold,
            FontRole::Italic => self.italic,
            FontRole::BoldItalic => self.bold_italic,
            FontRole::Monospace => self.monospace,
            FontRole::Icons => self.icons,
        }
    }

    pub const fn manifest_bytes(&self) -> &'static [u8] {
        self.set.manifest_bytes()
    }

    /// Large fallback members registered with a Latin/UI family but excluded
    /// from its eager asset set and manifest.
    pub const fn lazy_chain(&self, role: FontRole) -> &'static [LazyFontAsset] {
        if !matches!(self.set, FontSet::Latin) {
            return NO_LAZY_FONTS;
        }
        match role {
            FontRole::Regular | FontRole::Italic => LATIN_LAZY_REGULAR,
            FontRole::Bold | FontRole::BoldItalic => LATIN_LAZY_BOLD,
            FontRole::Monospace | FontRole::Icons => NO_LAZY_FONTS,
        }
    }

    pub fn declares_asset_path(&self, path: &str) -> bool {
        let mut index = 0;
        while index < self.assets.len() {
            if bytes_equal(self.assets[index].resource_path.as_bytes(), path.as_bytes()) {
                return true;
            }
            index += 1;
        }
        for role in [
            FontRole::Regular,
            FontRole::Bold,
            FontRole::Italic,
            FontRole::BoldItalic,
        ] {
            let lazy = self.lazy_chain(role);
            index = 0;
            while index < lazy.len() {
                if bytes_equal(lazy[index].asset.resource_path.as_bytes(), path.as_bytes()) {
                    return true;
                }
                index += 1;
            }
        }
        false
    }
}

const IBM_PLEX_TEXT: FontAsset = FontAsset {
    id: "ibm_plex_text",
    resource_path: "makepad_widgets/resources/IBMPlexSans-Text.ttf",
    ascender: -0.1,
    descender: 0.0,
    weight: 0.0,
};
const IBM_PLEX_SEMIBOLD: FontAsset = FontAsset {
    id: "ibm_plex_semibold",
    resource_path: "makepad_widgets/resources/IBMPlexSans-SemiBold.ttf",
    ascender: -0.1,
    descender: 0.0,
    weight: 0.0,
};
const IBM_PLEX_ITALIC: FontAsset = FontAsset {
    id: "ibm_plex_italic",
    resource_path: "makepad_widgets/resources/IBMPlexSans-Italic.ttf",
    ascender: -0.1,
    descender: 0.0,
    weight: 0.0,
};
const IBM_PLEX_BOLD_ITALIC: FontAsset = FontAsset {
    id: "ibm_plex_bold_italic",
    resource_path: "makepad_widgets/resources/IBMPlexSans-BoldItalic.ttf",
    ascender: -0.1,
    descender: 0.0,
    weight: 0.0,
};
const LIBERATION_MONO: FontAsset = FontAsset {
    id: "liberation_mono",
    resource_path: "makepad_widgets/resources/LiberationMono-Regular.ttf",
    ascender: 0.0,
    descender: 0.0,
    weight: 0.0,
};
const FONT_AWESOME: FontAsset = FontAsset {
    id: "font_awesome_solid",
    resource_path: "makepad_widgets/resources/fa-solid-900.ttf",
    ascender: 0.0,
    descender: 0.0,
    weight: 0.0,
};
const JETBRAINS_UI_SYMBOLS: FontAsset = FontAsset {
    id: "jetbrains_ui_symbols",
    resource_path: "makepad_widgets/resources/jetbrains_mono_variable.ttf",
    ascender: 0.0,
    descender: 0.0,
    weight: 0.0,
};
const NOTO_SANS_REGULAR: FontAsset = FontAsset {
    id: "noto_sans_regular",
    resource_path: "makepad_widgets/resources/NotoSans-Regular.ttf",
    ascender: 0.0,
    descender: 0.0,
    weight: 0.0,
};
const LXGW_REGULAR: FontAsset = FontAsset {
    id: "lxgw_wenkai_regular",
    resource_path: "makepad_widgets/resources/LXGWWenKaiRegular.ttf",
    ascender: 0.0,
    descender: 0.0,
    weight: 0.0,
};
const LXGW_BOLD: FontAsset = FontAsset {
    id: "lxgw_wenkai_bold",
    resource_path: "makepad_widgets/resources/LXGWWenKaiBold.ttf",
    ascender: 0.0,
    descender: 0.0,
    weight: 0.0,
};
const NOTO_COLOR_EMOJI: FontAsset = FontAsset {
    id: "noto_color_emoji",
    resource_path: "makepad_widgets/resources/NotoColorEmoji.ttf",
    ascender: 0.0,
    descender: 0.0,
    weight: 0.0,
};

/// The checked-in JetBrains Mono variable face supplies the compact UI-symbol
/// cmap missing from IBM Plex. Its name table carries the SIL OFL 1.1 license;
/// the draw crate freezes both the license and required cmap in a test.
pub const UI_SYMBOL_FALLBACK: Option<FontAsset> = Some(JETBRAINS_UI_SYMBOLS);

const LATIN_REGULAR: &[FontAsset] = &[IBM_PLEX_TEXT, NOTO_SANS_REGULAR, JETBRAINS_UI_SYMBOLS];
const LATIN_BOLD: &[FontAsset] = &[IBM_PLEX_SEMIBOLD, NOTO_SANS_REGULAR, JETBRAINS_UI_SYMBOLS];
const LATIN_ITALIC: &[FontAsset] = &[IBM_PLEX_ITALIC, NOTO_SANS_REGULAR, JETBRAINS_UI_SYMBOLS];
const LATIN_BOLD_ITALIC: &[FontAsset] = &[
    IBM_PLEX_BOLD_ITALIC,
    NOTO_SANS_REGULAR,
    JETBRAINS_UI_SYMBOLS,
];
const MONOSPACE: &[FontAsset] = &[LIBERATION_MONO];
const ICONS: &[FontAsset] = &[FONT_AWESOME];

const NO_LAZY_FONTS: &[LazyFontAsset] = &[];
const LATIN_LAZY_REGULAR: &[LazyFontAsset] = &[
    LazyFontAsset { family: LazyFontFamily::Cjk, asset: LXGW_REGULAR },
    LazyFontAsset { family: LazyFontFamily::Emoji, asset: NOTO_COLOR_EMOJI },
];
const LATIN_LAZY_BOLD: &[LazyFontAsset] = &[
    LazyFontAsset { family: LazyFontFamily::Cjk, asset: LXGW_BOLD },
    LazyFontAsset { family: LazyFontFamily::Emoji, asset: NOTO_COLOR_EMOJI },
];

const INTERNATIONAL_REGULAR: &[FontAsset] = &[IBM_PLEX_TEXT, LXGW_REGULAR, NOTO_COLOR_EMOJI];
const INTERNATIONAL_BOLD: &[FontAsset] = &[IBM_PLEX_SEMIBOLD, LXGW_BOLD, NOTO_COLOR_EMOJI];
const INTERNATIONAL_ITALIC: &[FontAsset] = &[IBM_PLEX_ITALIC, LXGW_REGULAR, NOTO_COLOR_EMOJI];
const INTERNATIONAL_BOLD_ITALIC: &[FontAsset] =
    &[IBM_PLEX_BOLD_ITALIC, LXGW_BOLD, NOTO_COLOR_EMOJI];

const LATIN_ASSETS: &[FontAsset] = &[
    IBM_PLEX_TEXT,
    IBM_PLEX_SEMIBOLD,
    IBM_PLEX_ITALIC,
    IBM_PLEX_BOLD_ITALIC,
    LIBERATION_MONO,
    FONT_AWESOME,
    JETBRAINS_UI_SYMBOLS,
    NOTO_SANS_REGULAR,
];
const INTERNATIONAL_ASSETS: &[FontAsset] = &[
    IBM_PLEX_TEXT,
    IBM_PLEX_SEMIBOLD,
    IBM_PLEX_ITALIC,
    IBM_PLEX_BOLD_ITALIC,
    LIBERATION_MONO,
    FONT_AWESOME,
    LXGW_REGULAR,
    LXGW_BOLD,
    NOTO_COLOR_EMOJI,
];

const LATIN_POLICY: FontPolicy = FontPolicy {
    set: FontSet::Latin,
    regular: FontChain { role: FontRole::Regular, members: LATIN_REGULAR },
    bold: FontChain { role: FontRole::Bold, members: LATIN_BOLD },
    italic: FontChain { role: FontRole::Italic, members: LATIN_ITALIC },
    bold_italic: FontChain { role: FontRole::BoldItalic, members: LATIN_BOLD_ITALIC },
    monospace: FontChain { role: FontRole::Monospace, members: MONOSPACE },
    icons: FontChain { role: FontRole::Icons, members: ICONS },
    assets: LATIN_ASSETS,
};

const INTERNATIONAL_POLICY: FontPolicy = FontPolicy {
    set: FontSet::International,
    regular: FontChain { role: FontRole::Regular, members: INTERNATIONAL_REGULAR },
    bold: FontChain { role: FontRole::Bold, members: INTERNATIONAL_BOLD },
    italic: FontChain { role: FontRole::Italic, members: INTERNATIONAL_ITALIC },
    bold_italic: FontChain { role: FontRole::BoldItalic, members: INTERNATIONAL_BOLD_ITALIC },
    monospace: FontChain { role: FontRole::Monospace, members: MONOSPACE },
    icons: FontChain { role: FontRole::Icons, members: ICONS },
    assets: INTERNATIONAL_ASSETS,
};

pub const LATIN_FONT_ASSET_MANIFEST: &[u8; 485] = b"format=makepad.font-assets.v1\nset=Latin\nasset=makepad_widgets/resources/IBMPlexSans-Text.ttf\nasset=makepad_widgets/resources/IBMPlexSans-SemiBold.ttf\nasset=makepad_widgets/resources/IBMPlexSans-Italic.ttf\nasset=makepad_widgets/resources/IBMPlexSans-BoldItalic.ttf\nasset=makepad_widgets/resources/LiberationMono-Regular.ttf\nasset=makepad_widgets/resources/fa-solid-900.ttf\nasset=makepad_widgets/resources/jetbrains_mono_variable.ttf\nasset=makepad_widgets/resources/NotoSans-Regular.ttf\n";

/// The wasm/package payload for the Latin/UI runtime set. The final three
/// files remain distributable assets, but are not members of the eager Latin
/// manifest above and are fetched/read only after a glyph miss.
const LATIN_LAZY_PACKAGE_ASSETS: &[&str] = &[
    LXGW_REGULAR.resource_path,
    LXGW_BOLD.resource_path,
    NOTO_COLOR_EMOJI.resource_path,
];
const LATIN_PACKAGE_MANIFEST_LEN: usize =
    font_asset_manifest_len(LATIN_FONT_ASSET_MANIFEST, LATIN_LAZY_PACKAGE_ASSETS);
pub const LATIN_FONT_ASSET_PACKAGE_MANIFEST: &[u8; LATIN_PACKAGE_MANIFEST_LEN] =
    &extend_font_asset_manifest(LATIN_FONT_ASSET_MANIFEST, LATIN_LAZY_PACKAGE_ASSETS);

pub const INTERNATIONAL_FONT_ASSET_MANIFEST: &[u8; 536] = b"format=makepad.font-assets.v1\nset=International\nasset=makepad_widgets/resources/IBMPlexSans-Text.ttf\nasset=makepad_widgets/resources/IBMPlexSans-SemiBold.ttf\nasset=makepad_widgets/resources/IBMPlexSans-Italic.ttf\nasset=makepad_widgets/resources/IBMPlexSans-BoldItalic.ttf\nasset=makepad_widgets/resources/LiberationMono-Regular.ttf\nasset=makepad_widgets/resources/fa-solid-900.ttf\nasset=makepad_widgets/resources/LXGWWenKaiRegular.ttf\nasset=makepad_widgets/resources/LXGWWenKaiBold.ttf\nasset=makepad_widgets/resources/NotoColorEmoji.ttf\n";

const fn bytes_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

const fn bytes_at_equal(haystack: &[u8], start: usize, needle: &[u8]) -> bool {
    if start + needle.len() > haystack.len() {
        return false;
    }
    let mut index = 0;
    while index < needle.len() {
        if haystack[start + index] != needle[index] {
            return false;
        }
        index += 1;
    }
    true
}

const fn manifest_contains_asset(manifest: &[u8], asset: &str) -> bool {
    let prefix = b"asset=";
    let asset = asset.as_bytes();
    let mut start = 0;
    while start + prefix.len() + asset.len() < manifest.len() {
        if bytes_at_equal(manifest, start, prefix)
            && bytes_at_equal(manifest, start + prefix.len(), asset)
            && manifest[start + prefix.len() + asset.len()] == b'\n'
        {
            return true;
        }
        while start < manifest.len() && manifest[start] != b'\n' {
            start += 1;
        }
        start += 1;
    }
    false
}

const fn extra_asset_is_duplicate(manifest: &[u8], assets: &[&str], index: usize) -> bool {
    if manifest_contains_asset(manifest, assets[index]) {
        return true;
    }
    let mut previous = 0;
    while previous < index {
        if bytes_equal(assets[previous].as_bytes(), assets[index].as_bytes()) {
            return true;
        }
        previous += 1;
    }
    false
}

#[doc(hidden)]
pub const fn font_asset_manifest_len(manifest: &[u8], extra_assets: &[&str]) -> usize {
    let mut len = manifest.len();
    let mut index = 0;
    while index < extra_assets.len() {
        if !extra_asset_is_duplicate(manifest, extra_assets, index) {
            len += b"asset=".len() + extra_assets[index].len() + 1;
        }
        index += 1;
    }
    len
}

#[doc(hidden)]
pub const fn extend_font_asset_manifest<const N: usize>(
    manifest: &[u8],
    extra_assets: &[&str],
) -> [u8; N] {
    assert!(N == font_asset_manifest_len(manifest, extra_assets));
    let mut out = [0; N];
    let mut write = 0;
    while write < manifest.len() {
        out[write] = manifest[write];
        write += 1;
    }
    let mut index = 0;
    while index < extra_assets.len() {
        let asset = extra_assets[index].as_bytes();
        let mut byte = 0;
        while byte < asset.len() {
            assert!(asset[byte] != b'\n' && asset[byte] != b'\r');
            byte += 1;
        }
        if !extra_asset_is_duplicate(manifest, extra_assets, index) {
            let prefix = b"asset=";
            byte = 0;
            while byte < prefix.len() {
                out[write] = prefix[byte];
                write += 1;
                byte += 1;
            }
            byte = 0;
            while byte < asset.len() {
                out[write] = asset[byte];
                write += 1;
                byte += 1;
            }
            out[write] = b'\n';
            write += 1;
        }
        index += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn fixture(set: FontSet) -> String {
        let policy = set.policy();
        let mut out = format!("format={}\nset={}\n", FONT_ASSET_MANIFEST_SECTION, set.as_str());
        for asset in policy.assets {
            out.push_str("asset=");
            out.push_str(asset.resource_path);
            out.push('\n');
        }
        out
    }

    #[test]
    fn policy_construction_has_ordered_latin_and_international_chains() {
        let latin = FontSet::Latin.policy();
        assert_eq!(
            latin.regular.members,
            &[IBM_PLEX_TEXT, NOTO_SANS_REGULAR, JETBRAINS_UI_SYMBOLS]
        );
        assert_eq!(
            latin.bold.members,
            &[IBM_PLEX_SEMIBOLD, NOTO_SANS_REGULAR, JETBRAINS_UI_SYMBOLS]
        );
        assert_eq!(latin.monospace.members, &[LIBERATION_MONO]);
        assert_eq!(latin.icons.members, &[FONT_AWESOME]);
        assert_eq!(UI_SYMBOL_FALLBACK, Some(JETBRAINS_UI_SYMBOLS));
        assert_eq!(latin.lazy_chain(FontRole::Regular), LATIN_LAZY_REGULAR);
        assert_eq!(latin.lazy_chain(FontRole::Bold), LATIN_LAZY_BOLD);

        let international = FontSet::International.policy();
        assert_eq!(international.regular.members, INTERNATIONAL_REGULAR);
        assert_eq!(international.bold.members, INTERNATIONAL_BOLD);
        assert_eq!(international.italic.members, INTERNATIONAL_ITALIC);
        assert_eq!(international.bold_italic.members, INTERNATIONAL_BOLD_ITALIC);
        assert!(international.lazy_chain(FontRole::Regular).is_empty());
        assert_eq!(international.monospace, latin.monospace);
        assert_eq!(international.icons, latin.icons);
    }

    #[test]
    fn assets_are_the_exact_unique_union_of_role_chains() {
        for set in [FontSet::Latin, FontSet::International] {
            let policy = set.policy();
            let mut union = HashMap::new();
            for role in [
                FontRole::Regular,
                FontRole::Bold,
                FontRole::Italic,
                FontRole::BoldItalic,
                FontRole::Monospace,
                FontRole::Icons,
            ] {
                let mut chain_paths = HashSet::new();
                for asset in policy.chain(role).members {
                    assert!(chain_paths.insert(asset.resource_path), "duplicate in {role:?}");
                    if let Some(previous) = union.insert(asset.resource_path, *asset) {
                        assert_eq!(previous, *asset, "conflicting declarations for {}", asset.resource_path);
                    }
                }
            }
            let asset_paths = policy
                .assets
                .iter()
                .map(|asset| asset.resource_path)
                .collect::<HashSet<_>>();
            assert_eq!(asset_paths.len(), policy.assets.len(), "duplicate policy asset");
            assert_eq!(asset_paths, union.into_keys().collect());
        }
    }

    #[test]
    fn manifest_extension_is_exact_and_suppresses_duplicates() {
        const EXTRAS: &[&str] = &[
            "makepad_widgets/resources/NewCMMath-Regular.otf",
            "makepad_widgets/resources/IBMPlexSans-Text.ttf",
            "makepad_widgets/resources/NewCMMath-Regular.otf",
        ];
        const N: usize = font_asset_manifest_len(LATIN_FONT_ASSET_MANIFEST, EXTRAS);
        const MANIFEST: [u8; N] = extend_font_asset_manifest(LATIN_FONT_ASSET_MANIFEST, EXTRAS);
        let text = std::str::from_utf8(&MANIFEST).unwrap();
        assert_eq!(text.matches("NewCMMath-Regular.otf").count(), 1);
        assert_eq!(text.matches("IBMPlexSans-Text.ttf").count(), 1);
        assert!(text.ends_with("asset=makepad_widgets/resources/NewCMMath-Regular.otf\n"));
    }

    #[test]
    fn target_default_matrix_is_stable() {
        assert_eq!(FontSet::target_default_for_web(true), FontSet::Latin);
        assert_eq!(FontSet::target_default_for_web(false), FontSet::International);
    }

    #[test]
    fn frozen_manifest_fixture_matches_policy_assets() {
        assert_eq!(
            FontSet::Latin.manifest_bytes(),
            include_bytes!("../tests/fixtures/font-assets-latin-v1.txt")
        );
        assert_eq!(
            FontSet::International.manifest_bytes(),
            include_bytes!("../tests/fixtures/font-assets-international-v1.txt")
        );
        assert_eq!(FontSet::Latin.manifest_bytes(), fixture(FontSet::Latin).as_bytes());
        assert_eq!(
            FontSet::International.manifest_bytes(),
            fixture(FontSet::International).as_bytes()
        );
    }

    #[test]
    fn latin_manifest_never_names_international_fonts() {
        let manifest = std::str::from_utf8(FontSet::Latin.manifest_bytes()).unwrap();
        assert!(!manifest.contains("LXGW"));
        assert!(!manifest.contains("NotoColorEmoji"));
        assert!(!manifest.contains("GoNoto"));
    }

    #[test]
    fn latin_package_keeps_lazy_international_assets() {
        let manifest = std::str::from_utf8(LATIN_FONT_ASSET_PACKAGE_MANIFEST).unwrap();
        assert!(manifest.contains("LXGWWenKaiRegular.ttf"));
        assert!(manifest.contains("LXGWWenKaiBold.ttf"));
        assert!(manifest.contains("NotoColorEmoji.ttf"));
    }

    #[test]
    fn selection_is_immutable_after_registration_starts() {
        let mut cx = crate::Cx::new(Box::new(|_, _| {}));
        assert!(cx.set_font_set(FontSet::Latin));
        cx.freeze_font_set();
        assert!(!cx.set_font_set(FontSet::International));
        assert_eq!(cx.font_set(), FontSet::Latin);
    }
}
