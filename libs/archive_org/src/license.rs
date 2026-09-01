//! The item's `licenseurl` as something a rights record can carry.
//!
//! Archive uploaders pick a Creative Commons URL, or nothing. This maps
//! the URL to an SPDX-style id and to the two grants a content store
//! records: may it be redistributed, may it be derived from. An item with
//! no license says nothing, and the mapping says nothing back — `Unknown`,
//! never a guess. The host decides what "unknown" means for its store.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grant {
    Allowed,
    AttributionRequired,
    Forbidden,
    /// The license did not say, or there was no license.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LicenseInfo {
    /// SPDX id when the URL maps to one (`CC-BY-SA-4.0`, `CC0-1.0`,
    /// `PDM-1.0`), else a `LicenseRef-…` marker.
    pub id: String,
    pub url: String,
    pub redistribution: Grant,
    pub derivatives: Grant,
    /// The license carries a NonCommercial clause — a flag the id also
    /// spells out, surfaced so a UI can badge it without parsing.
    pub non_commercial: bool,
}

impl LicenseInfo {
    pub fn unspecified() -> Self {
        LicenseInfo {
            id: "LicenseRef-Archive-Org-Unspecified".to_string(),
            url: String::new(),
            redistribution: Grant::Unknown,
            derivatives: Grant::Unknown,
            non_commercial: false,
        }
    }
}

/// Map a Creative Commons URL. Anything else (empty, a custom terms page)
/// is `LicenseRef-Archive-Org-Unspecified` with unknown grants and the
/// URL kept, so nothing about the item is thrown away.
pub fn license_from_url(url: &str) -> LicenseInfo {
    let url = url.trim();
    if url.is_empty() {
        return LicenseInfo::unspecified();
    }
    let lower = url.to_ascii_lowercase();
    let path = lower
        .split_once("creativecommons.org/")
        .map(|(_, rest)| rest.trim_end_matches('/'))
        .unwrap_or("");
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    let info = match (segs.next(), segs.next(), segs.next()) {
        (Some("publicdomain"), Some("zero"), _) => LicenseInfo {
            id: "CC0-1.0".into(),
            url: url.into(),
            redistribution: Grant::Allowed,
            derivatives: Grant::Allowed,
            non_commercial: false,
        },
        (Some("publicdomain"), Some("mark"), _) => LicenseInfo {
            id: "PDM-1.0".into(),
            url: url.into(),
            redistribution: Grant::Allowed,
            derivatives: Grant::Allowed,
            non_commercial: false,
        },
        (Some("licenses"), Some(kind), version) => {
            let version = version
                .filter(|v| v.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
                .unwrap_or("4.0");
            let parts: Vec<&str> = kind.split('-').collect();
            if parts.first() != Some(&"by") {
                return unknown_with_url(url);
            }
            let nd = parts.contains(&"nd");
            let nc = parts.contains(&"nc");
            LicenseInfo {
                id: format!("CC-{}-{}", kind.to_ascii_uppercase(), version),
                url: url.into(),
                redistribution: Grant::AttributionRequired,
                derivatives: if nd { Grant::Forbidden } else { Grant::AttributionRequired },
                non_commercial: nc,
            }
        }
        _ => unknown_with_url(url),
    };
    info
}

fn unknown_with_url(url: &str) -> LicenseInfo {
    LicenseInfo { url: url.to_string(), ..LicenseInfo::unspecified() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creative_commons() {
        let l = license_from_url("https://creativecommons.org/licenses/by-sa/3.0/");
        assert_eq!(l.id, "CC-BY-SA-3.0");
        assert_eq!(l.redistribution, Grant::AttributionRequired);
        assert_eq!(l.derivatives, Grant::AttributionRequired);
        assert!(!l.non_commercial);
        let l = license_from_url("http://creativecommons.org/licenses/by-nc-nd/4.0/");
        assert_eq!(l.id, "CC-BY-NC-ND-4.0");
        assert_eq!(l.derivatives, Grant::Forbidden);
        assert!(l.non_commercial);
        let l = license_from_url("http://creativecommons.org/licenses/by/");
        assert_eq!(l.id, "CC-BY-4.0");
        let l = license_from_url("https://creativecommons.org/publicdomain/zero/1.0/");
        assert_eq!(l.id, "CC0-1.0");
        assert_eq!(l.redistribution, Grant::Allowed);
        let l = license_from_url("https://creativecommons.org/publicdomain/mark/1.0/");
        assert_eq!(l.id, "PDM-1.0");
    }

    #[test]
    fn unknowns() {
        let l = license_from_url("");
        assert_eq!(l.id, "LicenseRef-Archive-Org-Unspecified");
        assert_eq!(l.redistribution, Grant::Unknown);
        let l = license_from_url("https://example.com/terms");
        assert_eq!(l.id, "LicenseRef-Archive-Org-Unspecified");
        assert_eq!(l.url, "https://example.com/terms");
        let l = license_from_url("https://creativecommons.org/licenses/sampling+/1.0/");
        assert_eq!(l.redistribution, Grant::Unknown);
    }
}
