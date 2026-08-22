//! The try-it harness: one REAL turn against a running Asset Server and a
//! real fleet box, with no window in the way.
//!
//! Ignored by default (it needs a live store and a live GPU). Run it with:
//!
//! ```text
//! MAKEPAD_LIVE_STORE=127.0.0.1:55463:55464 \
//! MAKEPAD_LIVE_TOKEN_FILE=local/asset-ui/asset-server/admin-token \
//! cargo test -p makepad-asset-chat-ui --test live_feed -- --ignored --nocapture
//! ```
//!
//! It proves the things a screenshot cannot: that the reply STREAMS, that
//! the rate meter reads the serving box's own token counts, and that
//! Escape's cancel really ends a turn in flight.

use makepad_asset_chat_ui::feed::{ChatFeed, FeedConfig, NoClientTools};
use makepad_asset_chat_ui::transcript::{ChatData, ChatRole, CHAT};
use makepad_asset_client::ApiEndpoints;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

fn live_config(cache: &str) -> Option<FeedConfig> {
    let spec = std::env::var("MAKEPAD_LIVE_STORE").ok()?;
    let parts: Vec<&str> = spec.split(':').collect();
    let [ip, control, data] = parts.as_slice() else {
        panic!("MAKEPAD_LIVE_STORE must be ip:control:data");
    };
    let control: SocketAddr = format!("{ip}:{control}").parse().expect("control addr");
    let data: SocketAddr = format!("{ip}:{data}").parse().expect("data addr");
    let token = std::env::var("MAKEPAD_LIVE_TOKEN_FILE")
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.trim().to_string());
    Some(FeedConfig::new(
        ApiEndpoints { control, data },
        token,
        std::env::temp_dir().join(format!("mp_chat_ui_live_{}_{cache}", std::process::id())),
        "gen",
        "gen",
    ))
}

fn wait_until(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if done() {
            return;
        }
        // The feed reports every failure as a system line; waiting out the
        // full timeout on one is a slow way to read a message we already
        // have.
        if let Ok(data) = CHAT.read() {
            if let Some(bad) = data.messages.iter().find(|m| m.role == ChatRole::System) {
                panic!("the feed refused the turn: {}", bad.text);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let data = CHAT.read().unwrap();
    panic!(
        "timed out waiting for {what}; activity={:?} streaming={} messages={:?}",
        data.activity,
        data.is_streaming,
        data.messages.iter().map(|m| (m.role, m.text.clone())).collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "needs a live Asset Server and fleet box (MAKEPAD_LIVE_STORE)"]
fn a_live_turn_streams_and_reports_a_real_rate() {
    let Some(cfg) = live_config("turn") else {
        eprintln!("MAKEPAD_LIVE_STORE unset — nothing to talk to");
        return;
    };
    ChatData::clear();
    let feed = ChatFeed::start(cfg, Box::new(NoClientTools));
    // Long enough to be MEASURED: a one-delta reply lands inside a single
    // sample and has no honest rate to report.
    feed.send("count from 1 to 40, one number per line, nothing else.".into(), Vec::new());

    // The live readout exists WHILE it streams; that is the whole point of
    // the meter (a number that only appears at the end teaches nothing).
    // A cold 27B box spends real time on the first session: the provider
    // probe, the assembled context, then prefill.
    let mut live: Option<String> = None;
    wait_until("the turn to land", 240, || {
        if let Some(rate) = ChatData::live_rate_label() {
            live = Some(rate);
        }
        !ChatData::is_streaming() && ChatData::item_count() > 1
    });
    println!("live rate while streaming: {live:?}");
    assert!(live.is_some(), "the meter must read a rate DURING the reply");
    let data = CHAT.read().unwrap();
    let reply = data
        .messages
        .iter()
        .rev()
        .find(|m| m.role == ChatRole::Assistant)
        .expect("an assistant reply");
    println!("reply: {}", reply.text);
    println!("meta: {:?}", reply.meta);
    assert!(!reply.text.trim().is_empty());
    let meta = reply.meta.as_deref().expect("a rate footnote on the landed reply");
    assert!(meta.contains("tok/s"), "{meta}");
    assert!(
        !meta.starts_with('~'),
        "a real serving box counts its own tokens — an estimate means the \
         serving block never arrived: {meta}"
    );
    drop(data);
    ChatData::clear();
}

#[test]
#[ignore = "needs a live Asset Server and fleet box (MAKEPAD_LIVE_STORE)"]
fn escape_ends_a_live_turn_in_flight() {
    let Some(cfg) = live_config("cancel") else {
        eprintln!("MAKEPAD_LIVE_STORE unset — nothing to talk to");
        return;
    };
    ChatData::clear();
    let feed = ChatFeed::start(cfg, Box::new(NoClientTools));
    feed.send("write a long detailed essay about rust ownership.".into(), Vec::new());
    // A cold 27B box spends real time on the first session: the provider
    // probe, the assembled context, then prefill.
    wait_until("the first token", 240, || {
        CHAT.read().map(|d| !d.streaming_text.is_empty()).unwrap_or(false)
    });
    feed.cancel();
    // Cancel is the broker's route, not a local flag: the turn has to STOP,
    // and it has to stop soon enough to be worth pressing.
    wait_until("the cancelled turn to stop", 30, || !ChatData::is_streaming());
    println!("cancelled after: {:?}", ChatData::item_count());
    ChatData::clear();
}
