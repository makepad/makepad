//! Live game rooms: the rendezvous behind `/v1/rooms`.
//!
//! A room is two people playing the same game in the same world right now.
//! Finding one is the problem this module exists to solve: the players'
//! machines talk to each other over direct LAN sockets, so a joiner needs an
//! address it cannot guess. UDP broadcast is the traditional answer and a
//! poor one — it dies at the first managed switch. Every player's app is
//! already attached to THIS server (it is where the game itself came from),
//! so this server is the meeting point that already exists.
//!
//! A room is NOT a catalog asset. It is a running process on somebody's desk
//! that will be gone when they close the lid, so it lives in memory with a
//! lease, exactly like [`super::profiles`]: an entry past its expiry is
//! dropped on the next read or write, and a host that crashes stops
//! advertising by itself within one lease. Nothing is persisted — a restarted
//! server re-learns the truth from the next heartbeat, which was the only
//! authoritative source all along.
//!
//! ## One room per game — the claim
//!
//! The map is keyed by GAME, not by room: two people who press Play on the
//! same game must land in ONE world, and the only way to guarantee that
//! against a simultaneous press is to make claiming the game an atomic
//! first-write-wins operation. The loser is told who won and joins them.
//!
//! ## Never a dead end
//!
//! A claim that nobody can dial is worse than no claim: every future player
//! would be sent at a host that is not there. A caller that actually tried to
//! reach a room and failed may take the claim from it by naming it in
//! `replacing` — one atomic replace, so the failing joiner becomes the new
//! host instead of hitting the same wall forever. The displaced host loses
//! only its ADVERTISEMENT: its own session and whoever is already in it carry
//! on untouched, and its next heartbeat tells it the claim moved.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// Most games that may advertise a live room at once. A room costs a few
/// hundred bytes; this is a bound against a stuck client, not a product
/// limit.
pub const MAX_ROOMS: usize = 64;
/// Sanity bound on a reported player count; a room is a LAN game, not a
/// stadium.
pub const MAX_PLAYERS: u32 = 256;
/// Longest game id a room may name.
pub const MAX_GAME_BYTES: usize = 64;
/// Longest invite string. `ip:tcp:udp#key` is ~90 bytes for IPv4 and ~110
/// for a bracketed IPv6 literal; the ceiling leaves room without inviting
/// somebody to smuggle a document through it.
pub const MAX_INVITE_BYTES: usize = 200;
/// Longest host display name ("rik's laptop").
pub const MAX_HOST_BYTES: usize = 64;
/// Lease bounds. The floor keeps a client from advertising a room that
/// expires before anyone can read it; the ceiling keeps a crashed host from
/// haunting the list.
pub const MIN_TTL_MS: u64 = 5_000;
pub const MAX_TTL_MS: u64 = 10 * 60 * 1000;

/// One live room, as the registry holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Room {
    /// `room_<16 lowercase hex>`, minted here.
    pub id: String,
    /// Opaque to this server: whatever the client calls the game it is
    /// playing. The server never interprets it — it is only ever compared.
    pub game: String,
    /// Everything a peer needs to dial the host, in the client's own
    /// spelling. The server does not parse it; a room key is capability
    /// data and this server is not the one enforcing it.
    pub invite: String,
    /// Who to say the room belongs to ("Joined rik's room").
    pub host: String,
    /// People in the world right now, host included — the host reports it
    /// on every heartbeat, so a games list can say "2 playing" beside the
    /// row. Starts at 1 (the host) on claim.
    pub players: u32,
    pub created_ms: u64,
    pub expires_ms: u64,
}

/// What a claim attempt produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claim {
    /// The caller now holds the game's claim. The token is its proof for
    /// heartbeat and retire, and is handed out exactly once.
    Mine { room: Room, token: String },
    /// Somebody else got there first. The caller joins this room instead of
    /// hosting a second world nobody would find.
    Occupied { room: Room },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomError {
    TooManyRooms,
    BadTtl,
    BadGame,
    BadInvite,
    BadHost,
    NoSuchRoom,
    NotYourRoom,
}

impl RoomError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooManyRooms => "too many live rooms",
            Self::BadTtl => "ttl out of bounds",
            Self::BadGame => "game id out of bounds",
            Self::BadInvite => "invite out of bounds",
            Self::BadHost => "host name out of bounds",
            Self::NoSuchRoom => "no such room",
            Self::NotYourRoom => "not your room",
        }
    }

    /// Whether this refusal is the caller's fault (400) or a room that is
    /// simply not there any more (404).
    pub fn http_status(self) -> u16 {
        match self {
            Self::NoSuchRoom => 404,
            Self::NotYourRoom => 403,
            _ => 400,
        }
    }
}

/// Held room, plus the secret only its host knows.
#[derive(Clone, Debug)]
struct Entry {
    room: Room,
    token: String,
}

/// In-memory, lease-expiring room advertisements, keyed by GAME so that the
/// claim and the room are the same fact. Cheap to lock: every operation is a
/// small map walk under one mutex, never a store call — this must not touch
/// the state thread.
#[derive(Debug, Default)]
pub struct RoomRegistry {
    games: Mutex<BTreeMap<String, Entry>>,
}

impl RoomRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take the game's claim, or learn who holds it.
    ///
    /// `replacing` is the anti-dead-end door: a caller that tried to dial
    /// that exact room and could not may take the claim from it. Naming a
    /// room that is not the current holder is not an error — it means the
    /// claim already moved on, and the caller is answered with whoever holds
    /// it now.
    ///
    /// `mint` produces the room id and the host token; it is passed in so
    /// this module stays free of randomness (and so tests are deterministic).
    pub fn claim(
        &self,
        game: &str,
        invite: &str,
        host: &str,
        ttl_ms: u64,
        replacing: Option<&str>,
        now_ms: u64,
        mint: impl FnOnce() -> (String, String),
    ) -> Result<Claim, RoomError> {
        check_ttl(ttl_ms)?;
        check_text(game, MAX_GAME_BYTES).map_err(|_| RoomError::BadGame)?;
        check_text(invite, MAX_INVITE_BYTES).map_err(|_| RoomError::BadInvite)?;
        check_text(host, MAX_HOST_BYTES).map_err(|_| RoomError::BadHost)?;
        let mut games = self.games.lock().unwrap_or_else(|e| e.into_inner());
        games.retain(|_, e| e.room.expires_ms > now_ms);
        if let Some(held) = games.get(game) {
            // The one case where a live claim yields: the caller reached for
            // this exact room and could not get there.
            let unreachable = replacing.is_some_and(|id| id == held.room.id);
            if !unreachable {
                return Ok(Claim::Occupied { room: held.room.clone() });
            }
        } else if games.len() >= MAX_ROOMS {
            return Err(RoomError::TooManyRooms);
        }
        let (id, token) = mint();
        let room = Room {
            id,
            game: game.to_string(),
            invite: invite.to_string(),
            host: host.to_string(),
            players: 1,
            created_ms: now_ms,
            expires_ms: now_ms.saturating_add(ttl_ms),
        };
        games.insert(
            game.to_string(),
            Entry { room: room.clone(), token: token.clone() },
        );
        Ok(Claim::Mine { room, token })
    }

    /// Renew a lease. `NoSuchRoom` means the claim is gone — expired, or
    /// taken by a joiner that could not reach this host — and the caller's
    /// answer is to claim again, not to retry.
    pub fn heartbeat(
        &self,
        id: &str,
        token: &str,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Result<Room, RoomError> {
        self.heartbeat_with(id, token, ttl_ms, None, now_ms)
    }

    /// A heartbeat that also reports how many people are in the world.
    /// `None` leaves the last count alone (an older client).
    pub fn heartbeat_with(
        &self,
        id: &str,
        token: &str,
        ttl_ms: u64,
        players: Option<u32>,
        now_ms: u64,
    ) -> Result<Room, RoomError> {
        check_ttl(ttl_ms)?;
        let mut games = self.games.lock().unwrap_or_else(|e| e.into_inner());
        games.retain(|_, e| e.room.expires_ms > now_ms);
        let entry = games
            .values_mut()
            .find(|e| e.room.id == id)
            .ok_or(RoomError::NoSuchRoom)?;
        if !constant_time_eq(entry.token.as_bytes(), token.as_bytes()) {
            return Err(RoomError::NotYourRoom);
        }
        entry.room.expires_ms = now_ms.saturating_add(ttl_ms);
        if let Some(players) = players {
            entry.room.players = players.clamp(1, MAX_PLAYERS);
        }
        Ok(entry.room.clone())
    }

    /// Give the claim up (the host left, or closed the lid politely).
    /// Retiring a room that is already gone is a no-op, never an error: a
    /// host that leaves twice must not be told off for it.
    pub fn retire(&self, id: &str, token: &str, now_ms: u64) -> Result<(), RoomError> {
        let mut games = self.games.lock().unwrap_or_else(|e| e.into_inner());
        games.retain(|_, e| e.room.expires_ms > now_ms);
        let Some((game, entry)) = games.iter().find(|(_, e)| e.room.id == id) else {
            return Ok(());
        };
        if !constant_time_eq(entry.token.as_bytes(), token.as_bytes()) {
            return Err(RoomError::NotYourRoom);
        }
        let game = game.clone();
        games.remove(&game);
        Ok(())
    }

    /// Every live room, or the one live room for `game`. Ordered by game id
    /// so the answer is stable across reads.
    pub fn live(&self, game: Option<&str>, now_ms: u64) -> Vec<Room> {
        let mut games = self.games.lock().unwrap_or_else(|e| e.into_inner());
        games.retain(|_, e| e.room.expires_ms > now_ms);
        games
            .values()
            .filter(|e| game.is_none_or(|g| e.room.game == g))
            .map(|e| e.room.clone())
            .collect()
    }
}

fn check_ttl(ttl_ms: u64) -> Result<(), RoomError> {
    if (MIN_TTL_MS..=MAX_TTL_MS).contains(&ttl_ms) {
        Ok(())
    } else {
        Err(RoomError::BadTtl)
    }
}

fn check_text(text: &str, max: usize) -> Result<(), ()> {
    if text.is_empty() || text.len() > max || text.chars().any(char::is_control) {
        return Err(());
    }
    Ok(())
}

/// Room tokens are secrets; comparing them must not leak their prefix
/// through timing. Same discipline the auth store uses for bearer tokens.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minter(n: u32) -> impl FnOnce() -> (String, String) {
        move || (format!("room_{n:016x}"), format!("secret{n}"))
    }

    fn claim(
        reg: &RoomRegistry,
        game: &str,
        host: &str,
        n: u32,
        now: u64,
    ) -> Result<Claim, RoomError> {
        reg.claim(game, "10.0.0.7:1:2#ab", host, 30_000, None, now, minter(n))
    }

    #[test]
    fn two_people_pressing_play_at_once_land_in_one_world() {
        let reg = RoomRegistry::new();
        // Both presses race. The registry serialises them, so exactly one
        // becomes the host and the other is handed the winner's address.
        let first = claim(&reg, "arcade", "rik", 1, 1_000).unwrap();
        let second = claim(&reg, "arcade", "sam", 2, 1_000).unwrap();
        let Claim::Mine { room: won, token } = first else {
            panic!("the first press must take the claim");
        };
        let Claim::Occupied { room: found } = second else {
            panic!("the second press must be sent to the first");
        };
        assert_eq!(found, won);
        assert_eq!(found.host, "rik");
        // The loser was told where to go and was NOT given the host token.
        assert_eq!(reg.live(Some("arcade"), 1_000), vec![won.clone()]);
        // A different game is a different claim, not a queue.
        assert!(matches!(claim(&reg, "racer", "sam", 3, 1_000), Ok(Claim::Mine { .. })));
        assert_eq!(reg.live(None, 1_000).len(), 2);
        assert_eq!(reg.live(Some("arcade"), 1_000), vec![won]);
        assert!(reg.heartbeat("room_0000000000000001", &token, 30_000, 2_000).is_ok());
    }

    #[test]
    fn a_room_nobody_can_dial_does_not_trap_the_next_player() {
        let reg = RoomRegistry::new();
        let Claim::Mine { room: dead, token } = claim(&reg, "arcade", "rik", 1, 1_000).unwrap()
        else {
            panic!("first claim");
        };
        // A joiner reads the room, fails to reach it, and says so by name.
        // It becomes the host rather than hitting the same wall forever.
        let taken = reg
            .claim(
                "arcade",
                "10.0.0.9:3:4#cd",
                "sam",
                30_000,
                Some(&dead.id),
                2_000,
                minter(2),
            )
            .unwrap();
        let Claim::Mine { room: live, .. } = taken else {
            panic!("an unreachable room must yield its claim");
        };
        assert_eq!(live.host, "sam");
        assert_eq!(reg.live(Some("arcade"), 2_000), vec![live]);
        // The displaced host learns the claim moved at its next heartbeat —
        // it is not told it is broken, only that it no longer advertises.
        assert_eq!(
            reg.heartbeat(&dead.id, &token, 30_000, 2_000),
            Err(RoomError::NoSuchRoom)
        );
        // Naming a room that is no longer the holder does NOT take the
        // claim: that is a stale report, not a failed dial of the live host.
        let stale = reg
            .claim("arcade", "10.0.0.5:5:6#ef", "kim", 30_000, Some(&dead.id), 3_000, minter(3))
            .unwrap();
        assert!(matches!(stale, Claim::Occupied { .. }));
    }

    #[test]
    fn a_host_that_vanishes_stops_advertising_within_one_lease() {
        let reg = RoomRegistry::new();
        let Claim::Mine { room, token } = reg
            .claim("arcade", "10.0.0.7:1:2#ab", "rik", 10_000, None, 1_000, minter(1))
            .unwrap()
        else {
            panic!("claim");
        };
        assert_eq!(reg.live(None, 5_000).len(), 1);
        // A heartbeat pushes the lease out; silence lets it lapse.
        reg.heartbeat(&room.id, &token, 10_000, 9_000).unwrap();
        assert_eq!(reg.live(None, 15_000).len(), 1);
        assert!(reg.live(None, 19_001).is_empty());
        // And the claim is free again for whoever presses Play next.
        assert!(matches!(claim(&reg, "arcade", "sam", 2, 20_000), Ok(Claim::Mine { .. })));
    }

    #[test]
    fn only_the_host_may_renew_or_retire_its_room() {
        let reg = RoomRegistry::new();
        let Claim::Mine { room, token } = claim(&reg, "arcade", "rik", 1, 1_000).unwrap() else {
            panic!("claim");
        };
        assert_eq!(
            reg.heartbeat(&room.id, "not-the-token", 30_000, 1_000),
            Err(RoomError::NotYourRoom)
        );
        assert_eq!(reg.retire(&room.id, "not-the-token", 1_000), Err(RoomError::NotYourRoom));
        assert_eq!(reg.live(None, 1_000).len(), 1);
        reg.retire(&room.id, &token, 1_000).unwrap();
        assert!(reg.live(None, 1_000).is_empty());
        // Leaving twice is not an error — a host that closes and exits runs
        // both paths, and neither should shout.
        reg.retire(&room.id, &token, 1_000).unwrap();
        assert_eq!(reg.heartbeat("room_00000000deadbeef", &token, 30_000, 1_000), Err(RoomError::NoSuchRoom));
    }

    #[test]
    fn advertisements_are_bounded_in_every_direction() {
        let reg = RoomRegistry::new();
        for i in 0..MAX_ROOMS {
            claim(&reg, &format!("game{i}"), "rik", i as u32, 1_000).unwrap();
        }
        assert_eq!(claim(&reg, "one-too-many", "rik", 999, 1_000), Err(RoomError::TooManyRooms));
        // A re-claim of a game already listed is a renewal path, not a new
        // slot, so a full registry never locks its own hosts out.
        assert!(matches!(claim(&reg, "game0", "rik", 0, 1_000), Ok(Claim::Occupied { .. })));
        let reg = RoomRegistry::new();
        let bad = |game: &str, invite: &str, host: &str, ttl: u64| {
            reg.claim(game, invite, host, ttl, None, 1_000, minter(1))
        };
        assert_eq!(bad("g", "i", "h", MIN_TTL_MS - 1), Err(RoomError::BadTtl));
        assert_eq!(bad("g", "i", "h", MAX_TTL_MS + 1), Err(RoomError::BadTtl));
        assert_eq!(bad("", "i", "h", 30_000), Err(RoomError::BadGame));
        assert_eq!(bad("g", "", "h", 30_000), Err(RoomError::BadInvite));
        assert_eq!(bad("g", "i", "", 30_000), Err(RoomError::BadHost));
        assert_eq!(bad("g", &"x".repeat(MAX_INVITE_BYTES + 1), "h", 30_000), Err(RoomError::BadInvite));
        assert_eq!(bad("g", "i", "line\nbreak", 30_000), Err(RoomError::BadHost));
    }
}
