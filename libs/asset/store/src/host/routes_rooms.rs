//! Live game rooms over HTTP: claim, list, heartbeat, retire.
//!
//! The whole point of these four routes is that a person presses Play and
//! ends up where their friends already are. The registry ([`super::rooms`])
//! holds the truth; this file is only the door.
//!
//! Two disciplines carried from the neighbouring route files:
//! - the state thread is used for authentication and NOTHING else. Rooms
//!   live in memory behind their own mutex, so the room work happens on the
//!   connection thread — the same split `PUT /v1/job-profiles` uses.
//! - a room is capability data (its invite carries the room key), so the
//!   list is readable by any authenticated principal and writable only by
//!   the holder of the room's own token.

use super::api::{body_str, body_u64, Fail, RouteResult};
use super::http::{Conn, Head, Method, Resp};
use super::json::{obj, s, Value};
use super::rooms::{Claim, Room, RoomError, MAX_GAME_BYTES};
use super::routes::{call_state, is_read, method_not_allowed, secret_of, Outcome, RouteCtx};
use super::routes_control::read_json_body;
use super::util::{now_ms, rand16, to_hex};

pub fn dispatch(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    segs: &[&str],
) -> Option<RouteResult<Outcome>> {
    let m = head.method;
    let result = match segs {
        ["v1", "rooms"] if is_read(m) => rooms_list(head, rc),
        ["v1", "rooms"] if m == Method::Post => room_claim(conn, head, rc),
        ["v1", "rooms"] => method_not_allowed(),
        ["v1", "rooms", id, "heartbeat"] if m == Method::Post => {
            room_heartbeat(conn, head, rc, id)
        }
        ["v1", "rooms", id, "retire"] if m == Method::Post => room_retire(conn, head, rc, id),
        _ => return None,
    };
    Some(result)
}

/// Rooms need a caller this server knows, and nothing more. There is no
/// `Capability::Rooms`: hosting a game you can already download is not a
/// privileged act, and the room key inside the invite is what actually
/// gates entry to the world itself.
fn auth(head: &Head, rc: &RouteCtx) -> RouteResult<()> {
    let secret = secret_of(head)?;
    let now = now_ms();
    call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        Ok(())
    })?;
    Ok(())
}

fn room_value(room: &Room) -> Value {
    obj(vec![
        ("room", s(room.id.clone())),
        ("game", s(room.game.clone())),
        ("invite", s(room.invite.clone())),
        ("host", s(room.host.clone())),
        ("players", Value::Int(room.players as i64)),
        ("created_ms", Value::Int(room.created_ms.min(i64::MAX as u64) as i64)),
        ("expires_ms", Value::Int(room.expires_ms.min(i64::MAX as u64) as i64)),
    ])
}

fn refuse(e: RoomError) -> Fail {
    Fail::Http(e.http_status(), e.as_str())
}

/// `GET /v1/rooms` — every live room, or `?game=<id>` for the one that
/// matters to a player about to press Play. Expired rooms are never listed:
/// an address nobody is listening on is worse than an empty answer.
fn rooms_list(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    auth(head, rc)?;
    let game = head.query_get("game");
    if let Some(game) = game {
        if game.is_empty() || game.len() > MAX_GAME_BYTES {
            return Err(Fail::Http(400, "game id out of bounds"));
        }
    }
    let rooms = rc.rooms.live(game, now_ms());
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("rooms", Value::Arr(rooms.iter().map(room_value).collect()))]),
    )))
}

/// `POST /v1/rooms` — "I am about to host this game; is anybody already?"
///
/// One call answers both halves, because asking and taking have to be the
/// same atomic act: two people pressing Play in the same second must not
/// both come away believing they are the host.
///
/// - 201 `{"outcome":"claimed", …, "token":…}` — you are the host; keep the
///   token, heartbeat with it, retire with it.
/// - 200 `{"outcome":"occupied", …}` — somebody beat you to it; the room in
///   the body is where you go instead.
///
/// `replacing` names a room this caller actually tried to dial and could
/// not. It is the only way a live claim changes hands, and it exists so a
/// stale room can never become a wall every future player runs into.
fn room_claim(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    auth(head, rc)?;
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    let game = body_str(&body, "game")?.to_string();
    let invite = body_str(&body, "invite")?.to_string();
    let host = body_str(&body, "host")?.to_string();
    let ttl_ms = body_u64(&body, "ttl_ms").ok_or(Fail::Http(400, "ttl_ms required"))?;
    let replacing = match body.get("replacing") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_str()
                .ok_or(Fail::Http(400, "replacing must be a room id"))?
                .to_string(),
        ),
    };
    // Minted before the claim rather than inside it: a room id and token
    // come from the OS, and a server with no randomness must refuse outright
    // instead of handing out a room whose token anybody could guess.
    let minted = mint()?;
    let claim = rc
        .rooms
        .claim(
            &game,
            &invite,
            &host,
            ttl_ms,
            replacing.as_deref(),
            now_ms(),
            move || minted,
        )
        .map_err(refuse)?;
    Ok(Outcome::Resp(match claim {
        Claim::Mine { room, token } => Resp::json(
            201,
            &obj(vec![
                ("outcome", s("claimed")),
                ("room", room_value(&room)),
                ("token", s(token)),
            ]),
        ),
        Claim::Occupied { room } => Resp::json(
            200,
            &obj(vec![("outcome", s("occupied")), ("room", room_value(&room))]),
        ),
    }))
}

/// `POST /v1/rooms/{id}/heartbeat` — "still here". A 404 is not a failure
/// to report to anybody: it means the claim moved on, and the host's answer
/// is to claim again.
fn room_heartbeat(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    id: &str,
) -> RouteResult<Outcome> {
    auth(head, rc)?;
    let id = id.to_string();
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    let token = body_str(&body, "token")?.to_string();
    let ttl_ms = body_u64(&body, "ttl_ms").ok_or(Fail::Http(400, "ttl_ms required"))?;
    // Optional: the host's head count. Out-of-range counts are clamped by
    // the registry rather than refused — a heartbeat must never fail over
    // a decoration.
    let players = body_u64(&body, "players").map(|n| n.min(u32::MAX as u64) as u32);
    let room = rc
        .rooms
        .heartbeat_with(&id, &token, ttl_ms, players, now_ms())
        .map_err(refuse)?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("room", room_value(&room))]),
    )))
}

/// `POST /v1/rooms/{id}/retire` — the host left. Idempotent: a room already
/// gone answers 204 too, because a host that leaves and then exits runs both
/// paths and neither is wrong.
fn room_retire(conn: &mut Conn, head: &mut Head, rc: &RouteCtx, id: &str) -> RouteResult<Outcome> {
    auth(head, rc)?;
    let id = id.to_string();
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    let token = body_str(&body, "token")?.to_string();
    rc.rooms.retire(&id, &token, now_ms()).map_err(refuse)?;
    Ok(Outcome::Resp(Resp::empty(204)))
}

/// A room id and the secret that proves ownership of it. Both come from the
/// OS: a guessable room token would let any listener on the network retire
/// somebody else's game.
fn mint() -> RouteResult<(String, String)> {
    let id = to_hex(&rand16()?[..8]);
    let token = to_hex(&rand16()?);
    Ok((format!("room_{id}"), token))
}
