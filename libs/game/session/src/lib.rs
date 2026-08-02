//! Session — how a room of devices plays one game (game.md §Multiplayer model).
//!
//! One authority, three roles:
//!
//! - [`Session::Local`] — no network. Exactly the single-player path, and the
//!   reason a game written before multiplayer existed still runs unchanged.
//! - [`Session::Host`] — simulates everything, replicates the Shared tier,
//!   applies client input as *requests* to the players it already knows.
//! - [`Session::Client`] — simulates nothing. It applies host truth, runs the
//!   Derived tier locally (facing, animation), and sends its input.
//!
//! The trust direction is the audit's conclusion made structural: a client
//! cannot express authoritative state, because the only thing it can send is
//! an input frame or an intent.

pub mod replication;

use makepad_game_blocks::{Blocks, DriveInput};
use makepad_game_net::endpoint::{Client, ClientEvent, Host, HostConfig, HostEvent};
use makepad_game_net::protocol::{
    EntityDesc, GameEvent, InputFrame, Intent, LeaveReason, PlayerId as NetPlayerId,
};
use makepad_game_sim::{step_world, GameWorld, PlayerId, PlayerSource, TICK_DT};
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;

pub use makepad_game_net::endpoint::MAX_PLAYERS;

/// What the application learns about the room this tick.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    Joined { player: PlayerId, name: String },
    Left { player: PlayerId, name: String },
    Disconnected { reason: LeaveReason },
}

/// Host-side per-player bookkeeping the sim does not own.
struct HostPlayer {
    net_id: NetPlayerId,
    sim_id: PlayerId,
    /// Kept after a disconnect so a player who rejoins under the same client
    /// identity resumes their slot — and their car — instead of becoming a
    /// stranger. Bounded below so this cannot grow into a leak.
    connected: bool,
}

/// How many departed players keep their mapping. Bounded because net ids are
/// client-chosen: an authenticated peer could otherwise cycle identities to
/// grow the table.
const REMEMBERED_DEPARTURES: usize = MAX_PLAYERS;

pub struct HostSession {
    net: Host,
    players: Vec<HostPlayer>,
    /// Descriptors already delivered, so only genuine appearances go out.
    sent_descs: HashMap<u64, EntityDesc>,
}

pub struct ClientSession {
    net: Client,
    /// The sim player id this device drives (assigned by the host).
    pub local_player: PlayerId,
    joined: bool,
}

pub enum Session {
    Local,
    Host(HostSession),
    Client(ClientSession),
}

impl Default for Session {
    fn default() -> Self {
        Session::Local
    }
}

impl Session {
    pub fn host(name: &str, secret: &[u8]) -> io::Result<Self> {
        Self::host_with(HostConfig::new(name, secret))
    }

    pub fn host_with(config: HostConfig) -> io::Result<Self> {
        Ok(Session::Host(HostSession {
            net: Host::bind(config)?,
            players: Vec::new(),
            sent_descs: HashMap::new(),
        }))
    }

    pub fn join(
        client_id: u64,
        name: &str,
        host_tcp: SocketAddr,
        host_udp: SocketAddr,
        secret: &[u8],
        now: f64,
    ) -> io::Result<Self> {
        Ok(Session::Client(ClientSession {
            net: Client::connect(client_id, name, host_tcp, host_udp, secret, now)?,
            local_player: PlayerId::LOCAL,
            joined: false,
        }))
    }

    pub fn is_host(&self) -> bool {
        matches!(self, Session::Host(_))
    }

    pub fn is_client(&self) -> bool {
        matches!(self, Session::Client(_))
    }

    /// Host TCP/UDP addresses, for showing a join hint or wiring tests.
    pub fn host_addrs(&self) -> Option<(SocketAddr, SocketAddr)> {
        match self {
            Session::Host(h) => Some((h.net.tcp_addr(), h.net.udp_addr())),
            _ => None,
        }
    }

    pub fn host_mut(&mut self) -> Option<&mut Host> {
        match self {
            Session::Host(h) => Some(&mut h.net),
            _ => None,
        }
    }

    pub fn client_mut(&mut self) -> Option<&mut Client> {
        match self {
            Session::Client(c) => Some(&mut c.net),
            _ => None,
        }
    }

    /// Drain the network and fold it into the world *before* the tick runs:
    /// joins create players, input frames land on those players, and a client
    /// adopts the host's world.
    pub fn pre_tick(
        &mut self,
        world: &mut GameWorld,
        blocks: &mut Blocks,
        now: f64,
    ) -> Vec<SessionEvent> {
        match self {
            Session::Local => Vec::new(),
            Session::Host(h) => h.pre_tick(world, now),
            Session::Client(c) => c.pre_tick(world, blocks, now),
        }
    }

    /// Publish the results *after* the tick: the host replicates Shared state,
    /// a client has nothing to say.
    pub fn post_tick(&mut self, world: &GameWorld) {
        if let Session::Host(h) = self {
            h.post_tick(world);
        }
    }

    /// Should this device run physics? A client's world is host truth plus the
    /// Derived tier — stepping it locally would fight the incoming state.
    pub fn simulates(&self) -> bool {
        !self.is_client()
    }

    /// Run one full tick in whichever role this session holds. Local and host
    /// simulate; a client only advances Derived state.
    pub fn tick(&mut self, world: &mut GameWorld, blocks: &mut Blocks, now: f64) -> Vec<SessionEvent> {
        let events = self.pre_tick(world, blocks, now);
        if self.simulates() {
            world.sync_local_player();
            // Turn each remote player's replicated input into block intent.
            // Deliberately NOT the local player: the host app already fills
            // `blocks.player_input` from its own devices, and reproducing that
            // here with a different (equally correct) trig call would change
            // single-player numbers — which input tapes would catch.
            // Bots are left alone too; script drives them.
            if self.is_host() {
                let remotes: Vec<PlayerId> = world.players.remotes().map(|p| p.id).collect();
                for player in remotes {
                    let intent = drive_input_for(world, player);
                    blocks.player_inputs.insert(player, intent);
                }
                blocks
                    .player_inputs
                    .retain(|id, _| world.players.get(*id).is_some());
            }
            if !blocks.is_empty() {
                blocks.pre_step(world);
            }
            step_world(world);
            world.tick += 1;
            world.time += TICK_DT as f64;
            if !blocks.is_empty() {
                blocks.post_step(world);
            }
            let alive: Vec<u64> = world.entities.iter().map(|e| e.id).collect();
            world
                .players
                .reconcile(|id| alive.binary_search(&id).is_ok());
        } else {
            replication::derive_local(world, TICK_DT);
            world.tick += 1;
            world.time += TICK_DT as f64;
            if !blocks.is_empty() {
                blocks.post_step(world);
            }
        }
        self.post_tick(world);
        events
    }
}

impl HostSession {
    fn sim_id_of(&self, net_id: NetPlayerId) -> Option<PlayerId> {
        self.players
            .iter()
            .find(|p| p.net_id == net_id)
            .map(|p| p.sim_id)
    }

    fn pre_tick(&mut self, world: &mut GameWorld, now: f64) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        for event in self.net.pump(now) {
            match event {
                HostEvent::Joined { player, name } => {
                    // A rejoining player keeps their sim slot (and their car),
                    // which is why the net layer resets sequence state instead
                    // of the game inventing a new identity.
                    let sim_id = match self.sim_id_of(player) {
                        Some(existing) => {
                            world.players.add_with_id(existing, name.clone(), PlayerSource::Remote);
                            if let Some(slot) = self.players.iter_mut().find(|p| p.net_id == player) {
                                slot.connected = true;
                            }
                            existing
                        }
                        None => {
                            let sim_id = world.players.add(name.clone(), PlayerSource::Remote);
                            self.players.push(HostPlayer {
                                net_id: player,
                                sim_id,
                                connected: true,
                            });
                            self.forget_old_departures();
                            sim_id
                        }
                    };
                    out.push(SessionEvent::Joined {
                        player: sim_id,
                        name,
                    });
                }
                HostEvent::Left { player, .. } => {
                    if let Some(sim_id) = self.sim_id_of(player) {
                        let name = world
                            .players
                            .get(sim_id)
                            .map(|p| p.name.clone())
                            .unwrap_or_default();
                        // Free the body so the world does not keep a ghost car
                        // driving itself around the track.
                        if let Some(p) = world.players.get(sim_id) {
                            let entity = p.entity;
                            if entity != 0 {
                                world.entities.retain(|e| e.id != entity);
                            }
                        }
                        world.players.remove(sim_id);
                        if let Some(slot) = self.players.iter_mut().find(|p| p.sim_id == sim_id) {
                            slot.connected = false;
                        }
                        self.forget_old_departures();
                        out.push(SessionEvent::Left {
                            player: sim_id,
                            name,
                        });
                    }
                }
                HostEvent::Input { player, frame } => {
                    if let Some(sim_id) = self.sim_id_of(player) {
                        if let Some(p) = world.players.get_mut(sim_id) {
                            p.input
                                .apply_wire(frame.buttons, frame.axis_x, frame.axis_z, frame.cam_yaw);
                        }
                    }
                }
                HostEvent::Intent { player, intent } => {
                    if let (Some(sim_id), Intent::Respawn) = (self.sim_id_of(player), &intent) {
                        // Intent is a request: the host decides what respawn
                        // means, the client only gets to ask.
                        if let Some(p) = world.players.get(sim_id) {
                            let _ = p;
                        }
                    }
                }
            }
        }
        out
    }

    fn post_tick(&mut self, world: &GameWorld) {
        // Descriptors: only what actually appeared, vanished or was restyled.
        // The scan is O(entities) but allocation-free in the steady state —
        // `desc_matches` compares in place rather than rebuilding (which would
        // clone every tag string, every tick).
        //
        // An earlier version gated this on `render_rev`, which was wrong:
        // that counter tracks STATIC-visible changes by design, so spawning a
        // car — the entity a race is actually made of — never tripped it and
        // clients saw poses for entities they could not build.
        let mut fresh = Vec::new();
        for e in &world.entities {
            match self.sent_descs.get(&e.id) {
                Some(seen) if replication::desc_matches(seen, e) => {}
                _ => fresh.push(replication::desc_of(e)),
            }
        }
        if self.sent_descs.len() != world.entities.len() || !fresh.is_empty() {
            let gone: Vec<u64> = self
                .sent_descs
                .keys()
                .copied()
                .filter(|id| world.entity(*id).is_none())
                .collect();
            for id in gone {
                self.sent_descs.remove(&id);
                self.net.broadcast_event(world.tick, GameEvent::Remove { id });
            }
        }
        if !fresh.is_empty() {
            for desc in &fresh {
                self.sent_descs.insert(desc.id, desc.clone());
            }
            self.net.broadcast_descriptors(world.tick, fresh);
            // Joiners get the world as it is now, not as it was at startup.
            self.net
                .set_descriptors(world.tick, replication::collect_descs(world));
        }

        let states = replication::collect_states(world);
        self.net.set_snapshot(world.tick, states.clone());
        self.net.broadcast_state(world.tick, &states);
    }

    /// Keep the departed-player table bounded.
    fn forget_old_departures(&mut self) {
        let departed = self.players.iter().filter(|p| !p.connected).count();
        if departed <= REMEMBERED_DEPARTURES {
            return;
        }
        let mut to_drop = departed - REMEMBERED_DEPARTURES;
        self.players.retain(|p| {
            if !p.connected && to_drop > 0 {
                to_drop -= 1;
                return false;
            }
            true
        });
    }
}

impl ClientSession {
    fn pre_tick(
        &mut self,
        world: &mut GameWorld,
        blocks: &mut Blocks,
        now: f64,
    ) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        let mut dirty = false;
        for event in self.net.pump(now) {
            match event {
                ClientEvent::Welcome { player, tick } => {
                    self.joined = true;
                    // The host's id for us; the local slot stays player 0 for
                    // rendering and input, and this is the identity our input
                    // travels under.
                    self.local_player = PlayerId(player.0 as u32);
                    world.tick = tick;
                    dirty = true;
                }
                ClientEvent::Descriptors { .. } | ClientEvent::State { .. } => dirty = true,
                ClientEvent::Event { event, .. } => {
                    if let GameEvent::Remove { .. } = event {
                        dirty = true;
                    }
                }
                ClientEvent::Disconnected { reason } => {
                    out.push(SessionEvent::Disconnected { reason });
                }
            }
        }
        if dirty {
            let descs: Vec<EntityDesc> = self.net.descs.values().cloned().collect();
            replication::apply_world(world, descs.into_iter(), &self.net.entities);
            blocks.reconcile(world);
        }

        // Send this device's intent for the tick.
        if self.joined {
            world.sync_local_player();
            let input = &world.players.local().input;
            let (axis_x, axis_z) = input.axes();
            self.net.send_input(InputFrame {
                tick: world.tick,
                axis_x: axis_x as f32,
                axis_z: axis_z as f32,
                cam_yaw: input.cam_yaw,
                buttons: input.buttons(),
            });
        }
        let _ = now;
        out
    }
}

/// The local player's control intent in the shape blocks consume — the same
/// struct whether it came from this device, a network packet, or a bot script.
pub fn drive_input_for(world: &GameWorld, player: PlayerId) -> DriveInput {
    use makepad_live_id::{live_id, LiveId};
    let held = |name: LiveId| world.action_held_for(player, name);
    let (axis_x, axis_z) = match world.players.get(player) {
        Some(p) if !player.is_local_slot() => p.input.axes(),
        _ => {
            let key = |name: LiveId| world.held.contains(&name);
            let x = ((key(live_id!(right)) as i8 - key(live_id!(left)) as i8) as f64
                + world.pad.axis_x)
                .clamp(-1.0, 1.0);
            let z = ((key(live_id!(down)) as i8 - key(live_id!(up)) as i8) as f64
                + world.pad.axis_z)
                .clamp(-1.0, 1.0);
            (x, z)
        }
    };
    let (move_x, move_z) = world.player_move(player);
    DriveInput {
        steer: axis_x as f32,
        throttle: -axis_z as f32,
        brake: if held(live_id!(grab)) { 1.0 } else { 0.0 },
        handbrake: if held(live_id!(shoot)) { 1.0 } else { 0.0 },
        move_x: move_x as f32,
        move_z: move_z as f32,
        jump: held(live_id!(jump)),
        jump_pressed: world.action_pressed_for(player, live_id!(jump)),
        pitch: -axis_z as f32,
        roll: axis_x as f32,
    }
}
