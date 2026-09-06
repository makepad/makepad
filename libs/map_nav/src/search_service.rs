//! Shared policy for merging a regional in-memory search index with the
//! continent-scale positioned-read database (or its in-memory places
//! fallback).

use crate::{
    geo::{haversine_m, LonLat},
    search::{SearchIndex, SearchResult},
    searchdb::SearchDb,
};
use std::sync::Arc;

pub struct SearchService {
    pub regional: Arc<SearchIndex>,
    pub searchdb: Option<Arc<SearchDb>>,
    pub places: Option<Arc<SearchIndex>>,
}

impl SearchService {
    pub fn new(
        regional: Arc<SearchIndex>,
        searchdb: Option<Arc<SearchDb>>,
        places: Option<Arc<SearchIndex>>,
    ) -> Self {
        Self {
            regional,
            searchdb,
            places,
        }
    }

    pub fn query(&self, text: &str, near: Option<LonLat>, limit: usize) -> Vec<SearchResult> {
        let regional = self.regional.query(text, near, limit);
        let broader = if let Some(db) = &self.searchdb {
            db.query(text, near, limit).unwrap_or_default()
        } else if let Some(places) = &self.places {
            places.query(text, near, limit)
        } else {
            Vec::new()
        };
        merge_search_results(regional, broader, limit)
    }
}

/// Score-sort and deduplicate same-name results within two kilometres.
/// Kept public so existing app-owned data can use the exact server policy.
pub fn merge_search_results(
    mut regional: Vec<SearchResult>,
    broader: Vec<SearchResult>,
    limit: usize,
) -> Vec<SearchResult> {
    regional.extend(broader);
    regional.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept: Vec<SearchResult> = Vec::new();
    for result in regional {
        if kept.iter().any(|existing| {
            existing.name.eq_ignore_ascii_case(&result.name)
                && haversine_m(existing.pos, result.pos) < 2_000.0
        }) {
            continue;
        }
        kept.push(result);
        if kept.len() >= limit {
            break;
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{Category, SearchIndexBuilder};

    #[test]
    fn merge_prefers_score_and_dedupes_nearby_same_name() {
        let mut regional = SearchIndexBuilder::new();
        regional.add("Museum", "regional", LonLat::new(4.9, 52.37), Category::Museum, 100);
        let mut places = SearchIndexBuilder::new();
        places.add("museum", "duplicate", LonLat::new(4.901, 52.371), Category::Museum, 200);
        places.add("Museum", "far", LonLat::new(5.2, 52.37), Category::Museum, 150);
        let service = SearchService::new(
            Arc::new(regional.build()),
            None,
            Some(Arc::new(places.build())),
        );
        let results = service.query("museum", None, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].secondary, "duplicate");
        assert_eq!(results[1].secondary, "far");
    }
}
