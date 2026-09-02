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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FontRole {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Monospace,
    Icons,
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
    /// De-duplicated package order, frozen by the v1 manifest fixture.
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

/// No checked-in small UI-symbol font has a matching license file. Bravura is
/// explicitly licensed but is a large music-symbol face, so it is not used as
/// a general UI fallback. Until a licensed subset is added, IBM Plex remains
/// the sole proportional member of the Latin policy.
pub const UI_SYMBOL_FALLBACK: Option<FontAsset> = None;

const LATIN_REGULAR: &[FontAsset] = &[IBM_PLEX_TEXT];
const LATIN_BOLD: &[FontAsset] = &[IBM_PLEX_SEMIBOLD];
const LATIN_ITALIC: &[FontAsset] = &[IBM_PLEX_ITALIC];
const LATIN_BOLD_ITALIC: &[FontAsset] = &[IBM_PLEX_BOLD_ITALIC];
const MONOSPACE: &[FontAsset] = &[LIBERATION_MONO];
const ICONS: &[FontAsset] = &[FONT_AWESOME];

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

pub const LATIN_FONT_ASSET_MANIFEST: &[u8; 372] = b"format=makepad.font-assets.v1\nset=Latin\nasset=makepad_widgets/resources/IBMPlexSans-Text.ttf\nasset=makepad_widgets/resources/IBMPlexSans-SemiBold.ttf\nasset=makepad_widgets/resources/IBMPlexSans-Italic.ttf\nasset=makepad_widgets/resources/IBMPlexSans-BoldItalic.ttf\nasset=makepad_widgets/resources/LiberationMono-Regular.ttf\nasset=makepad_widgets/resources/fa-solid-900.ttf\n";

pub const INTERNATIONAL_FONT_ASSET_MANIFEST: &[u8; 536] = b"format=makepad.font-assets.v1\nset=International\nasset=makepad_widgets/resources/IBMPlexSans-Text.ttf\nasset=makepad_widgets/resources/IBMPlexSans-SemiBold.ttf\nasset=makepad_widgets/resources/IBMPlexSans-Italic.ttf\nasset=makepad_widgets/resources/IBMPlexSans-BoldItalic.ttf\nasset=makepad_widgets/resources/LiberationMono-Regular.ttf\nasset=makepad_widgets/resources/fa-solid-900.ttf\nasset=makepad_widgets/resources/LXGWWenKaiRegular.ttf\nasset=makepad_widgets/resources/LXGWWenKaiBold.ttf\nasset=makepad_widgets/resources/NotoColorEmoji.ttf\n";

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(latin.regular.members, &[IBM_PLEX_TEXT]);
        assert_eq!(latin.bold.members, &[IBM_PLEX_SEMIBOLD]);
        assert_eq!(latin.monospace.members, &[LIBERATION_MONO]);
        assert_eq!(latin.icons.members, &[FONT_AWESOME]);
        assert_eq!(UI_SYMBOL_FALLBACK, None);

        let international = FontSet::International.policy();
        assert_eq!(international.regular.members, INTERNATIONAL_REGULAR);
        assert_eq!(international.bold.members, INTERNATIONAL_BOLD);
        assert_eq!(international.italic.members, INTERNATIONAL_ITALIC);
        assert_eq!(international.bold_italic.members, INTERNATIONAL_BOLD_ITALIC);
        assert_eq!(international.monospace, latin.monospace);
        assert_eq!(international.icons, latin.icons);
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
    fn selection_is_immutable_after_registration_starts() {
        let mut cx = crate::Cx::new(Box::new(|_, _| {}));
        assert!(cx.set_font_set(FontSet::Latin));
        cx.freeze_font_set();
        assert!(!cx.set_font_set(FontSet::International));
        assert_eq!(cx.font_set(), FontSet::Latin);
    }
}
