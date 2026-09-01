//! `advancedsearch.php`: building the query, reading the page.
//!
//! The archive's search speaks Lucene. The operator's words go in as they
//! are — `title:foo` and `"a phrase"` keep working for anyone who knows
//! them — wrapped in parentheses and ANDed with the media filter. Results
//! come back sorted by downloads, because on a VJ console the thing that
//! ten thousand people fetched is far more likely to be the clip you meant
//! than the newsroom recording that happened to match a word.

use crate::http::Error;
use crate::url::encode_query_component;
use makepad_micro_serde::{DeJson, JsonValue};

/// Rows per page the archive will honour without complaint.
pub const MAX_ROWS: u32 = 100;

/// Which of the archive's media types to search.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MediaFilter {
    /// Video and stills — the two things a picture wall can use.
    #[default]
    ImagesAndVideo,
    Video,
    Images,
    Audio,
    /// No media clause at all.
    Any,
}

impl MediaFilter {
    fn clause(self) -> Option<&'static str> {
        match self {
            MediaFilter::ImagesAndVideo => Some("mediatype:(movies OR image)"),
            MediaFilter::Video => Some("mediatype:(movies)"),
            MediaFilter::Images => Some("mediatype:(image)"),
            MediaFilter::Audio => Some("mediatype:(audio)"),
            MediaFilter::Any => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MediaFilter::ImagesAndVideo => "all",
            MediaFilter::Video => "video",
            MediaFilter::Images => "images",
            MediaFilter::Audio => "audio",
            MediaFilter::Any => "any",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Downloads,
    Relevance,
    Newest,
}

impl SortOrder {
    fn param(self) -> Option<&'static str> {
        match self {
            SortOrder::Downloads => Some("downloads desc"),
            SortOrder::Relevance => None,
            SortOrder::Newest => Some("publicdate desc"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub media: MediaFilter,
    pub sort: SortOrder,
    /// 1-based, like the archive's own `page`.
    pub page: u32,
    pub rows: u32,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            media: MediaFilter::default(),
            sort: SortOrder::default(),
            page: 1,
            rows: 48,
        }
    }

    /// The Lucene query string sent as `q`.
    pub fn lucene(&self) -> String {
        // Control bytes cannot mean anything in a query and can break the
        // request line; everything else is the operator's own syntax.
        let text: String = self.text.chars().filter(|c| !c.is_control()).collect();
        let text = text.trim();
        match (text.is_empty(), self.media.clause()) {
            (true, Some(clause)) => clause.to_string(),
            (true, None) => "*:*".to_string(),
            (false, Some(clause)) => format!("({text}) AND {clause}"),
            (false, None) => format!("({text})"),
        }
    }

    pub fn url(&self) -> String {
        let mut url = format!(
            "https://archive.org/advancedsearch.php?q={}",
            encode_query_component(&self.lucene())
        );
        for field in [
            "identifier",
            "title",
            "mediatype",
            "description",
            "date",
            "downloads",
            "creator",
            "licenseurl",
        ] {
            url.push_str("&fl%5B%5D=");
            url.push_str(field);
        }
        if let Some(sort) = self.sort.param() {
            url.push_str("&sort%5B%5D=");
            url.push_str(&encode_query_component(sort));
        }
        url.push_str(&format!(
            "&rows={}&page={}&output=json",
            self.rows.clamp(1, MAX_ROWS),
            self.page.max(1)
        ));
        url
    }
}

/// The archive's coarse media type of an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemMediaType {
    Movies,
    Image,
    Audio,
    Texts,
    Other(String),
}

impl ItemMediaType {
    pub fn parse(s: &str) -> Self {
        match s {
            "movies" => ItemMediaType::Movies,
            "image" => ItemMediaType::Image,
            "audio" => ItemMediaType::Audio,
            "texts" => ItemMediaType::Texts,
            other => ItemMediaType::Other(other.to_string()),
        }
    }

    /// Three-letter badge for a tile corner.
    pub fn badge(&self) -> &'static str {
        match self {
            ItemMediaType::Movies => "MOV",
            ItemMediaType::Image => "IMG",
            ItemMediaType::Audio => "AUD",
            ItemMediaType::Texts => "TXT",
            ItemMediaType::Other(_) => "",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub identifier: String,
    pub title: String,
    pub mediatype: ItemMediaType,
    pub description: String,
    pub creator: String,
    /// ISO date as the archive gives it (`2010-05-30T00:00:00Z`), or empty.
    pub date: String,
    pub downloads: u64,
    pub license_url: String,
}

impl SearchHit {
    /// `2010` from `2010-05-30T00:00:00Z`; empty when there is no date.
    pub fn year(&self) -> &str {
        self.date.get(..4).filter(|y| y.bytes().all(|b| b.is_ascii_digit())).unwrap_or("")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchPage {
    /// Matches in the whole archive, not on this page.
    pub total: u64,
    pub page: u32,
    pub rows: u32,
    pub hits: Vec<SearchHit>,
}

impl SearchPage {
    pub fn pages(&self) -> u32 {
        if self.rows == 0 {
            return 1;
        }
        ((self.total + self.rows as u64 - 1) / self.rows as u64).clamp(1, u32::MAX as u64) as u32
    }
}

/// Text from a value the archive may give as a string OR an array of
/// strings (description, creator, subject all do this).
pub(crate) fn text_of(value: Option<&JsonValue>) -> String {
    match value {
        Some(JsonValue::String(s)) => s.clone(),
        Some(JsonValue::Array(items)) => items
            .iter()
            .filter_map(|v| match v {
                JsonValue::String(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(", "),
        Some(JsonValue::U64(n)) => n.to_string(),
        Some(JsonValue::I64(n)) => n.to_string(),
        Some(JsonValue::F64(n)) => n.to_string(),
        Some(JsonValue::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// A number the archive may give as a number or a decimal string.
pub(crate) fn u64_of(value: Option<&JsonValue>) -> u64 {
    match value {
        Some(JsonValue::U64(n)) => *n,
        Some(JsonValue::I64(n)) => (*n).max(0) as u64,
        Some(JsonValue::F64(n)) => n.max(0.0) as u64,
        Some(JsonValue::String(s)) => s.trim().parse::<f64>().map(|f| f.max(0.0) as u64).unwrap_or(0),
        _ => 0,
    }
}

pub(crate) fn f64_of(value: Option<&JsonValue>) -> f64 {
    match value {
        Some(JsonValue::U64(n)) => *n as f64,
        Some(JsonValue::I64(n)) => *n as f64,
        Some(JsonValue::F64(n)) => *n,
        Some(JsonValue::String(s)) => s.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub(crate) fn parse_json(json: &str) -> Result<JsonValue, Error> {
    JsonValue::deserialize_json(json).map_err(|e| Error::Json(format!("{e:?}")))
}

/// Read one search page. Rows without an identifier are dropped — they
/// cannot be fetched, so they cannot be shown.
pub fn parse_search(json: &str, query: &SearchQuery) -> Result<SearchPage, Error> {
    let root = parse_json(json)?;
    let response = root.key("response").ok_or(Error::Json("no response object".into()))?;
    let total = u64_of(response.key("numFound"));
    let mut hits = Vec::new();
    if let Some(JsonValue::Array(docs)) = response.key("docs") {
        for doc in docs {
            let identifier = text_of(doc.key("identifier"));
            if !crate::url::is_valid_identifier(&identifier) {
                continue;
            }
            let title = {
                let t = text_of(doc.key("title"));
                if t.trim().is_empty() {
                    identifier.clone()
                } else {
                    t
                }
            };
            hits.push(SearchHit {
                identifier,
                title,
                mediatype: ItemMediaType::parse(&text_of(doc.key("mediatype"))),
                description: text_of(doc.key("description")),
                creator: text_of(doc.key("creator")),
                date: text_of(doc.key("date")),
                downloads: u64_of(doc.key("downloads")),
                license_url: text_of(doc.key("licenseurl")),
            });
        }
    }
    Ok(SearchPage { total, page: query.page.max(1), rows: query.rows.clamp(1, MAX_ROWS), hits })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"responseHeader":{"status":0},"response":{"numFound":112797,"start":0,"docs":[
        {"date":"2025-12-10T00:00:00Z","downloads":1,"identifier":"SUDAN_20251210_033000","mediatype":"movies","title":"SUDAN : December 10"},
        {"creator":["Nifty-Senpai","Chiibi"],"date":"2010-05-30T00:00:00Z","description":["A meme","yey"],"downloads":6,"identifier":"apple-fukkireta","mediatype":"movies","title":"Apple Fukkireta"},
        {"identifier":"bad id","title":"dropped"},
        {"identifier":"untitled_1","mediatype":"image"}
    ]}}"#;

    #[test]
    fn query_shapes() {
        let q = SearchQuery::new("cat AND dog");
        assert_eq!(q.lucene(), "(cat AND dog) AND mediatype:(movies OR image)");
        let mut q = SearchQuery::new("  ");
        q.media = MediaFilter::Video;
        assert_eq!(q.lucene(), "mediatype:(movies)");
        q.media = MediaFilter::Any;
        assert_eq!(q.lucene(), "*:*");
        q.text = "x\u{0}y".into();
        assert_eq!(q.lucene(), "(xy)");
    }

    #[test]
    fn url_shape() {
        let mut q = SearchQuery::new("cat");
        q.rows = 500;
        q.page = 0;
        let url = q.url();
        assert!(url.starts_with("https://archive.org/advancedsearch.php?q=%28cat%29%20AND%20mediatype"));
        assert!(url.contains("&fl%5B%5D=identifier"));
        assert!(url.contains("&fl%5B%5D=licenseurl"));
        assert!(url.contains("&sort%5B%5D=downloads%20desc"));
        assert!(url.ends_with("&rows=100&page=1&output=json"));
        assert!(!url.contains(' '));
    }

    #[test]
    fn parse_page() {
        let q = SearchQuery::new("cat");
        let page = parse_search(SAMPLE, &q).unwrap();
        assert_eq!(page.total, 112797);
        assert_eq!(page.hits.len(), 3);
        assert_eq!(page.hits[1].creator, "Nifty-Senpai, Chiibi");
        assert_eq!(page.hits[1].description, "A meme, yey");
        assert_eq!(page.hits[1].year(), "2010");
        assert_eq!(page.hits[1].mediatype, ItemMediaType::Movies);
        assert_eq!(page.hits[2].title, "untitled_1");
        assert_eq!(page.hits[2].year(), "");
        assert_eq!(page.pages(), 2350);
    }

    #[test]
    fn bad_json() {
        assert!(matches!(parse_search("{", &SearchQuery::new("x")), Err(Error::Json(_))));
        assert!(matches!(parse_search("{}", &SearchQuery::new("x")), Err(Error::Json(_))));
    }
}
