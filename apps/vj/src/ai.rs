//! VJ controls on the desktop AI bus.

use crate::{
    autopilot::AutoStyle,
    catalog::Tile,
    decks::DeckId,
};
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_asset_data::AssetId;
use makepad_widgets::makepad_platform::makepad_micro_serde::*;

const MAX_SEARCH_RESULTS: usize = 20;

pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "vj",
        "VJ",
        "The live VJ and DJ console. Inspect the program and catalog, cue visual clips, move the video or DJ fader, control autopilot and overlay mode, and operate the two music decks.",
    )
    .with_tool(ToolDef::new(
        "status",
        "What is live and next, the video fader and overlay, autopilot and its style, and the current BPM when known.",
        r#"{"type":"object","properties":{}}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "search",
        "Find up to 20 loaded visual or music catalog tiles whose available title or alias contains every query word. Returns item ids accepted by cue or deck_load.",
        r#"{"type":"object","properties":{"query":{"type":"string","description":"words matched case-insensitively against catalog title and alias"}},"required":["query"]}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "cue",
        "Cue a visual catalog tile as program content through the same CueEngine click path as the VJ grid.",
        r#"{"type":"object","properties":{"item":{"type":"string","description":"an ast_ item id from search"}},"required":["item"]}"#,
        Risk::Act,
    ))
    .with_tool(ToolDef::new(
        "fader",
        "Move the video program fader: 0 is deck A and 1 is deck B.",
        r#"{"type":"object","properties":{"value":{"type":"number","minimum":0,"maximum":1}},"required":["value"]}"#,
        Risk::Act,
    ))
    .with_tool(ToolDef::new(
        "next",
        "Take the ready next visual cue now, using the VJ's existing armed-cue transition path.",
        r#"{"type":"object","properties":{}}"#,
        Risk::Act,
    ))
    .with_tool(ToolDef::new(
        "autopilot",
        "Turn DJ autopilot on or off and optionally select its transition style. Valid styles are `outro` and `body`.",
        r#"{"type":"object","properties":{"on":{"type":"boolean"},"style":{"type":"string","enum":["outro","body"],"description":"outro or body"}},"required":["on"]}"#,
        Risk::Act,
    ))
    .with_tool(ToolDef::new(
        "overlay",
        "Turn visual overlay cue mode on or off.",
        r#"{"type":"object","properties":{"on":{"type":"boolean"}},"required":["on"]}"#,
        Risk::Act,
    ))
    .with_tool(ToolDef::new(
        "deck_play",
        "Start music deck A or B if it has a loaded track.",
        r#"{"type":"object","properties":{"deck":{"type":"string","enum":["A","B"]}},"required":["deck"]}"#,
        Risk::Act,
    ))
    .with_tool(ToolDef::new(
        "deck_stop",
        "Stop music deck A or B without unloading its track.",
        r#"{"type":"object","properties":{"deck":{"type":"string","enum":["A","B"]}},"required":["deck"]}"#,
        Risk::Act,
    ))
    .with_tool(ToolDef::new(
        "deck_load",
        "Load a music catalog tile onto deck A or B exactly like loading it from the DJ library.",
        r#"{"type":"object","properties":{"deck":{"type":"string","enum":["A","B"]},"item":{"type":"string","description":"an ast_ music item id from search"}},"required":["deck","item"]}"#,
        Risk::Act,
    ))
    .with_tool(ToolDef::new(
        "crossfade",
        "Move the music-deck crossfader: 0 is deck A and 1 is deck B.",
        r#"{"type":"object","properties":{"value":{"type":"number","minimum":0,"maximum":1}},"required":["value"]}"#,
        Risk::Act,
    ))
}

#[derive(Debug, PartialEq)]
pub enum Request {
    Status,
    Search(String),
    Cue(AssetId),
    Fader(f32),
    Next,
    Autopilot { on: bool, style: Option<AutoStyle> },
    Overlay(bool),
    DeckPlay(DeckId),
    DeckStop(DeckId),
    DeckLoad { deck: DeckId, item: AssetId },
    Crossfade(f32),
}

#[derive(DeJson)]
struct SearchArgs {
    query: String,
}

#[derive(DeJson)]
struct ItemArgs {
    item: String,
}

#[derive(DeJson)]
struct ValueArgs {
    value: f32,
}

#[derive(DeJson)]
struct AutopilotArgs {
    on: bool,
    style: Option<String>,
}

#[derive(DeJson)]
struct OnArgs {
    on: bool,
}

#[derive(DeJson)]
struct DeckArgs {
    deck: String,
}

#[derive(DeJson)]
struct DeckLoadArgs {
    deck: String,
    item: String,
}

fn args<T: DeJson>(call: &ServiceCall) -> Result<T, ToolResult> {
    T::deserialize_json_lenient(&call.args).map_err(|error| {
        ToolResult::refused(
            &call.call_id,
            format!("invalid arguments for vj.{}: {error:?}", call.tool),
        )
    })
}

fn item(call: &ServiceCall, text: &str) -> Result<AssetId, ToolResult> {
    text.trim().parse().map_err(|_| {
        ToolResult::refused(
            &call.call_id,
            format!("`{text}` is not a catalog item id from vj.search"),
        )
    })
}

fn deck(call: &ServiceCall, text: &str) -> Result<DeckId, ToolResult> {
    match text.trim().to_ascii_lowercase().as_str() {
        "a" => Ok(DeckId::A),
        "b" => Ok(DeckId::B),
        _ => Err(ToolResult::refused(&call.call_id, "deck must be A or B")),
    }
}

fn unit_value(call: &ServiceCall, value: f32) -> Result<f32, ToolResult> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(ToolResult::refused(
            &call.call_id,
            "value must be between 0 and 1",
        ))
    }
}

pub fn decode(call: &ServiceCall) -> Result<Request, ToolResult> {
    match call.tool.as_str() {
        "status" => Ok(Request::Status),
        "search" => {
            let args: SearchArgs = args(call)?;
            let query = args.query.trim().to_string();
            if query.is_empty() {
                return Err(ToolResult::refused(&call.call_id, "search needs a non-empty `query`"));
            }
            Ok(Request::Search(query))
        }
        "cue" => {
            let args: ItemArgs = args(call)?;
            Ok(Request::Cue(item(call, &args.item)?))
        }
        "fader" => {
            let args: ValueArgs = args(call)?;
            Ok(Request::Fader(unit_value(call, args.value)?))
        }
        "next" => Ok(Request::Next),
        "autopilot" => {
            let args: AutopilotArgs = args(call)?;
            let style = match args.style.as_deref().map(str::trim) {
                None => None,
                Some(style) if style.eq_ignore_ascii_case("outro") => Some(AutoStyle::Outro),
                Some(style) if style.eq_ignore_ascii_case("body") => Some(AutoStyle::Body),
                Some(_) => {
                    return Err(ToolResult::refused(
                        &call.call_id,
                        "autopilot style must be `outro` or `body`",
                    ))
                }
            };
            Ok(Request::Autopilot { on: args.on, style })
        }
        "overlay" => {
            let args: OnArgs = args(call)?;
            Ok(Request::Overlay(args.on))
        }
        "deck_play" => {
            let args: DeckArgs = args(call)?;
            Ok(Request::DeckPlay(deck(call, &args.deck)?))
        }
        "deck_stop" => {
            let args: DeckArgs = args(call)?;
            Ok(Request::DeckStop(deck(call, &args.deck)?))
        }
        "deck_load" => {
            let args: DeckLoadArgs = args(call)?;
            Ok(Request::DeckLoad {
                deck: deck(call, &args.deck)?,
                item: item(call, &args.item)?,
            })
        }
        "crossfade" => {
            let args: ValueArgs = args(call)?;
            Ok(Request::Crossfade(unit_value(call, args.value)?))
        }
        other => Err(ToolResult::refused(
            &call.call_id,
            format!(
                "vj has no tool `{other}`; it has status, search, cue, fader, next, autopilot, overlay, deck_play, deck_stop, deck_load, crossfade"
            ),
        )),
    }
}

fn words(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

pub fn search<'a>(tiles: impl IntoIterator<Item = &'a Tile>, query: &str) -> Vec<&'a Tile> {
    let wanted = words(query);
    tiles
        .into_iter()
        .filter(|tile| {
            let mut available = words(&tile.title);
            if let Some(alias) = tile.alias.as_deref() {
                available.extend(words(alias));
            }
            wanted.iter().all(|word| available.contains(word))
        })
        .take(MAX_SEARCH_RESULTS)
        .collect()
}

pub fn style_name(style: AutoStyle) -> &'static str {
    match style {
        AutoStyle::Outro => "outro",
        AutoStyle::Body => "body",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::TileState;
    use makepad_ai_services::wire::ToolOutcome;

    fn call(tool: &str, args: &str) -> ServiceCall {
        ServiceCall { call_id: "c1".into(), tool: tool.into(), args: args.into() }
    }

    fn tile(seed: u8, title: &str, alias: Option<&str>) -> Tile {
        Tile {
            asset: AssetId::from_bytes([seed; 16]),
            title: title.into(),
            alias: alias.map(str::to_string),
            live: true,
            kind: None,
            revision: None,
            media: None,
            source: None,
            thumb: None,
            state: TileState::Ready,
        }
    }

    #[test]
    fn manifest_validates_with_the_declared_risks() {
        let manifest = manifest();
        assert_eq!(manifest.id, "vj");
        manifest.validate().expect("a manifest the wire accepts");
        assert_eq!(manifest.tools.len(), 11);
        assert_eq!(manifest.tool("status").unwrap().risk, Risk::Read);
        assert_eq!(manifest.tool("search").unwrap().risk, Risk::Read);
        assert!(manifest.tools.iter().skip(2).all(|tool| tool.risk == Risk::Act));
    }

    #[test]
    fn unknown_tools_are_refused() {
        let result = decode(&call("lights_out", "{}")).unwrap_err();
        assert_eq!(result.outcome, ToolOutcome::Refused);
        assert!(result.text.contains("no tool `lights_out`"));
    }

    #[test]
    fn fader_refuses_values_outside_the_unit_interval() {
        for value in ["-0.01", "1.01"] {
            let result = decode(&call("fader", &format!(r#"{{"value":{value}}}"#))).unwrap_err();
            assert_eq!(result.outcome, ToolOutcome::Refused);
        }
        assert_eq!(decode(&call("fader", r#"{"value":0.25}"#)).unwrap(), Request::Fader(0.25));
    }

    #[test]
    fn search_matches_every_word_over_a_tiny_catalog() {
        let tiles = [
            tile(1, "Blue Tunnel", Some("loops/night-drive")),
            tile(2, "Blue Sky", Some("stills/day")),
            tile(3, "Red Tunnel", Some("loops/night-drive")),
        ];
        let hits = search(tiles.iter(), "blue night");
        assert_eq!(hits.iter().map(|tile| tile.title.as_str()).collect::<Vec<_>>(), vec!["Blue Tunnel"]);
        assert_eq!(search(tiles.iter(), "night-drive").len(), 2, "grid tokenization treats punctuation as a word boundary");
        assert!(search(tiles.iter(), "tun").is_empty(), "grid search matches words, not substrings");
    }
}
