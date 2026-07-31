//! geo.* tools — search over the nav indexes (SearchIndex + SearchDb).

use makepad_ai::makepad_micro_serde::JsonValue;
use makepad_ai::ToolDefinition;
use makepad_map_nav::geo::LonLat;

use crate::broker::{arg_f64, arg_str, arg_usize, def, ToolCtx};

pub fn defs() -> Vec<ToolDefinition> {
    vec![def(
        "geo_search",
        "Search places, streets, addresses and categories (e.g. 'supermarkt', 'Zaanse Schans', 'Groningen'). Netherlands has full detail; the rest of Europe has settlements and major places. Returns candidates with coordinates to use in other tools.",
        r#"{"type":"object","properties":{
            "query":{"type":"string"},
            "near_lon":{"type":"number","description":"bias results towards this point; defaults to current map center"},
            "near_lat":{"type":"number"},
            "limit":{"type":"integer","description":"max results, default 8"}},
          "required":["query"]}"#,
    )]
}

pub fn search(ctx: &mut ToolCtx, args: &JsonValue) -> Result<String, String> {
    let query = arg_str(args, "query").ok_or("missing query")?.to_string();
    // A degenerate/repetitious query (greedy-decoding loops produce them)
    // makes the fuzzy index scan crawl. Refuse with guidance instead.
    if query.len() > 64 || query.split_whitespace().count() > 8 {
        return Err(format!(
            "query too long ({} chars) — search for ONE short place name or category, \
             e.g. 'Oudegracht 399 Utrecht' or 'hotel museumkwartier'",
            query.len()
        ));
    }
    let limit = arg_usize(args, "limit").unwrap_or(8).clamp(1, 20);
    let (center_lon, center_lat) = ctx.map_center();
    let near = LonLat {
        lon: arg_f64(args, "near_lon").unwrap_or(center_lon),
        lat: arg_f64(args, "near_lat").unwrap_or(center_lat),
    };
    let nav = ctx.nav()?;
    let results = nav.search(&query, Some(near), limit);
    if results.is_empty() {
        return Ok(format!("no results for '{query}'"));
    }
    let mut out = String::new();
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!(
            "{}. {} | {} | lon {:.5} lat {:.5}",
            i + 1,
            r.name,
            r.category.label(),
            r.pos.lon,
            r.pos.lat
        ));
        if !r.secondary.is_empty() {
            out.push_str(&format!(" | {}", r.secondary));
        }
        if let Some(d) = r.distance_m {
            out.push_str(&format!(" | {:.1} km away", d / 1000.0));
        }
        out.push('\n');
    }
    Ok(out)
}
