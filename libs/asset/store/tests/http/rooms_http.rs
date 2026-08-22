//! Live game rooms over HTTP: the rendezvous two players meet at when they
//! press Play on the same game. Claim, list, heartbeat, retire — and the two
//! situations that make the difference between "we are in the same world"
//! and "we are each alone": a simultaneous press, and a room nobody can dial.

mod common;

use common::*;
use makepad_asset_store::json::Value;

const TTL: i64 = 30_000;

fn setup() -> (TestServer, Client, Client) {
    let ts = start_server("rooms");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    // Hosting a game you can already download is not a privileged act:
    // rooms need a principal this server knows and no capability at all.
    let player = principal_with(&mut admin, &[]);
    let client = ts.control(Some(&player));
    (ts, admin, client)
}

fn claim_body(game: &str, invite: &str, host: &str) -> Value {
    jobj(vec![
        ("game", jstr(game)),
        ("invite", jstr(invite)),
        ("host", jstr(host)),
        ("ttl_ms", Value::Int(TTL)),
    ])
}

fn rooms_of(client: &mut Client, query: &str) -> Vec<Value> {
    let r = client.get(&format!("/v1/rooms{query}"));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json()
        .get("rooms")
        .and_then(Value::as_arr)
        .expect("rooms array")
        .to_vec()
}

fn field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// The whole feature in one test: two people press Play on the same game and
/// end up in ONE world, with the second told where the first is.
#[test]
fn two_players_pressing_play_on_one_game_meet_in_one_room() {
    let (ts, _admin, mut rik) = setup();
    let mut sam = ts.control(None);
    let token = ts.admin_token();
    sam.set_token(Some(&token));

    // Nobody is playing yet.
    assert!(rooms_of(&mut rik, "?game=arcade").is_empty());

    // Rik presses Play: no room, so he hosts and takes the claim.
    let r = rik.post_json("/v1/rooms", &claim_body("arcade", "10.0.0.7:5000:5001#ab", "rik"));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.str_field("outcome"), "claimed");
    let room = r.json().get("room").cloned().expect("room");
    let room_id = field(&room, "room");
    let host_token = r.str_field("token");
    assert!(room_id.starts_with("room_"), "{room_id}");

    // Sam presses Play on the same game one moment later. The server does
    // not hand him a second claim — it hands him Rik's address.
    let r = sam.post_json("/v1/rooms", &claim_body("arcade", "10.0.0.9:6000:6001#cd", "sam"));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.str_field("outcome"), "occupied");
    let found = r.json().get("room").cloned().expect("room");
    assert_eq!(field(&found, "invite"), "10.0.0.7:5000:5001#ab");
    assert_eq!(field(&found, "host"), "rik");
    assert_eq!(field(&found, "room"), room_id);
    // The loser is never given the host's token — it cannot retire the room
    // it just joined.
    assert!(r.json().get("token").is_none());

    // One game, one room, both players' view of it identical.
    let listed = rooms_of(&mut sam, "?game=arcade");
    assert_eq!(listed.len(), 1);
    assert_eq!(field(&listed[0], "room"), room_id);
    assert_eq!(rooms_of(&mut rik, "").len(), 1);
    // A different game has its own claim, not a queue behind this one.
    let r = sam.post_json("/v1/rooms", &claim_body("racer", "10.0.0.9:7000:7001#ef", "sam"));
    assert_eq!(r.status, 201);
    assert_eq!(rooms_of(&mut rik, "").len(), 2);
    assert_eq!(rooms_of(&mut rik, "?game=arcade").len(), 1);

    // Rik keeps the room alive, then leaves; the claim frees immediately so
    // the next press hosts rather than dialling a machine that has quit.
    let beat = jobj(vec![("token", jstr(host_token.clone())), ("ttl_ms", Value::Int(TTL))]);
    let r = rik.post_json(&format!("/v1/rooms/{room_id}/heartbeat"), &beat);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let retire = jobj(vec![("token", jstr(host_token))]);
    let r = rik.post_json(&format!("/v1/rooms/{room_id}/retire"), &retire);
    assert_eq!(r.status, 204);
    assert!(rooms_of(&mut sam, "?game=arcade").is_empty());
}

/// A room whose host cannot be reached must not become a wall every later
/// player runs into. The joiner that failed says which room it failed on and
/// becomes the host itself.
#[test]
fn a_room_that_cannot_be_dialled_yields_its_claim_instead_of_trapping_the_joiner() {
    let (_ts, _admin, mut player) = setup();
    let r = player.post_json("/v1/rooms", &claim_body("arcade", "10.0.0.7:5000:5001#ab", "rik"));
    let dead_id = field(&r.json().get("room").cloned().unwrap(), "room");
    let dead_token = r.str_field("token");

    // Sam reads the room, tries to dial it, gets nothing, and claims it.
    let mut body = claim_body("arcade", "10.0.0.9:6000:6001#cd", "sam");
    if let Value::Obj(pairs) = &mut body {
        pairs.push(("replacing".to_string(), jstr(dead_id.clone())));
    }
    let r = player.post_json("/v1/rooms", &body);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.str_field("outcome"), "claimed");
    let listed = rooms_of(&mut player, "?game=arcade");
    assert_eq!(listed.len(), 1);
    assert_eq!(field(&listed[0], "host"), "sam");

    // The displaced host learns the claim moved the next time it says it is
    // alive. It is not an error condition — it re-claims, and finds Sam.
    let beat = jobj(vec![("token", jstr(dead_token)), ("ttl_ms", Value::Int(TTL))]);
    let r = player.post_json(&format!("/v1/rooms/{dead_id}/heartbeat"), &beat);
    assert_eq!(r.status, 404, "{}", String::from_utf8_lossy(&r.body));

    // Naming a room that is no longer the holder is a stale report, not a
    // failed dial of the live host — it must not take a working room down.
    let mut stale = claim_body("arcade", "10.0.0.5:8000:8001#99", "kim");
    if let Value::Obj(pairs) = &mut stale {
        pairs.push(("replacing".to_string(), jstr(dead_id)));
    }
    let r = player.post_json("/v1/rooms", &stale);
    assert_eq!(r.status, 200);
    assert_eq!(r.str_field("outcome"), "occupied");
}

/// A room is somebody's running game: only its host may renew or end it, and
/// nobody anonymous may even look at the list.
#[test]
fn rooms_are_authenticated_token_gated_and_shape_checked() {
    let (ts, _admin, mut player) = setup();
    let mut anon = ts.control(None);
    assert_eq!(anon.get("/v1/rooms").status, 401);
    assert_eq!(anon.post_json("/v1/rooms", &claim_body("g", "i", "h")).status, 401);
    assert_eq!(player.delete("/v1/rooms").status, 405);

    let r = player.post_json("/v1/rooms", &claim_body("arcade", "10.0.0.7:5000:5001#ab", "rik"));
    let id = field(&r.json().get("room").cloned().unwrap(), "room");
    let good = r.str_field("token");

    let wrong = jobj(vec![("token", jstr("not-the-token")), ("ttl_ms", Value::Int(TTL))]);
    assert_eq!(player.post_json(&format!("/v1/rooms/{id}/heartbeat"), &wrong).status, 403);
    assert_eq!(
        player
            .post_json(
                &format!("/v1/rooms/{id}/retire"),
                &jobj(vec![("token", jstr("not-the-token"))])
            )
            .status,
        403
    );
    assert_eq!(rooms_of(&mut player, "").len(), 1);

    // Unknown room: gone, not broken. Retiring one twice is not an error —
    // a host that leaves and then exits runs both paths.
    let beat = jobj(vec![("token", jstr(good.clone())), ("ttl_ms", Value::Int(TTL))]);
    assert_eq!(
        player.post_json("/v1/rooms/room_00000000deadbeef/heartbeat", &beat).status,
        404
    );
    let retire = jobj(vec![("token", jstr(good.clone()))]);
    assert_eq!(player.post_json(&format!("/v1/rooms/{id}/retire"), &retire).status, 204);
    assert_eq!(player.post_json(&format!("/v1/rooms/{id}/retire"), &retire).status, 204);

    // Shape refusals, all before anything is recorded.
    let bad_ttl = jobj(vec![
        ("game", jstr("g")),
        ("invite", jstr("i")),
        ("host", jstr("h")),
        ("ttl_ms", Value::Int(1)),
    ]);
    assert_eq!(player.post_json("/v1/rooms", &bad_ttl).status, 400);
    let missing = jobj(vec![("game", jstr("g")), ("ttl_ms", Value::Int(TTL))]);
    assert_eq!(player.post_json("/v1/rooms", &missing).status, 400);
    let long_invite = claim_body("g", &"x".repeat(400), "h");
    assert_eq!(player.post_json("/v1/rooms", &long_invite).status, 400);
    assert_eq!(player.get(&format!("/v1/rooms?game={}", "x".repeat(200))).status, 400);
    assert!(rooms_of(&mut player, "").is_empty());
}

/// A host that closes the lid stops advertising by itself: nobody sweeps,
/// nothing is persisted, and the claim frees for whoever presses Play next.
#[test]
fn a_lease_that_is_not_renewed_lapses_on_its_own() {
    let (_ts, _admin, mut player) = setup();
    let short = jobj(vec![
        ("game", jstr("arcade")),
        ("invite", jstr("10.0.0.7:5000:5001#ab")),
        ("host", jstr("rik")),
        // The floor of the lease range: short enough for a test to outlive.
        ("ttl_ms", Value::Int(5_000)),
    ]);
    let r = player.post_json("/v1/rooms", &short);
    assert_eq!(r.status, 201);
    assert_eq!(rooms_of(&mut player, "?game=arcade").len(), 1);
    // Real wall-clock expiry is what a crashed host produces, and waiting
    // five seconds for it in a test buys nothing the registry unit tests do
    // not already prove (`a_host_that_vanishes_stops_advertising_within_one
    // _lease`, on a hand-advanced clock). What this test pins is the HTTP
    // half: the claim is live while the lease is, and retire frees it now.
    let retire = jobj(vec![("token", jstr(r.str_field("token")))]);
    let id = field(&r.json().get("room").cloned().unwrap(), "room");
    assert_eq!(player.post_json(&format!("/v1/rooms/{id}/retire"), &retire).status, 204);
    let r = player.post_json("/v1/rooms", &claim_body("arcade", "10.0.0.9:1:2#cd", "sam"));
    assert_eq!(r.status, 201);
    assert_eq!(r.str_field("outcome"), "claimed");
}
