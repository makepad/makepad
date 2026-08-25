//! In-process fake mixer, bound to UDP loopback.
//!
//! Every automated run and every test in this crate talks to THIS, never to
//! hardware. It speaks the documented dialect: GET (no args) answers with
//! the stored value, SET stores + echoes + fans out to `/xremote`
//! subscribers, `/xinfo`//`/status` answer identity, and `/meters`
//! subscriptions stream int16 blobs at ~50 ms for 10 seconds.
//!
//! It also acts as a tripwire: any datagram whose address matches the deny
//! list is recorded in `danger` and NOT processed. Tests assert that list
//! stays empty; if the client ever leaked a dangerous packet, the fake
//! would catch it loudly.
//!
//! The fake's own replies are jailed too: it refuses to send to any
//! non-loopback destination (it only ever answers observed senders, which
//! are loopback by construction, but the assert makes that a guarantee).

use crate::osc::{OscArg, OscMsg};
use crate::safety::{deny_term, BusN, BusPair, Ch, ChPair, Param};
use crate::units::db_to_level;
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct FakeConfig {
    /// Reply to `/xinfo` in the official argument order (version first)
    /// instead of the community order (ip first). Both must parse.
    pub xinfo_official_order: bool,
    /// Animate meters and wiggle a fader periodically, pushing to
    /// subscribers — simulates a second controller moving things.
    pub animate: bool,
}

impl Default for FakeConfig {
    fn default() -> Self {
        FakeConfig { xinfo_official_order: false, animate: true }
    }
}

pub struct FakeMixer {
    pub addr: SocketAddr,
    /// Every datagram the fake received, decoded (tests inspect this).
    pub received: Arc<Mutex<Vec<OscMsg>>>,
    /// Deny-listed addresses that arrived. MUST stay empty.
    pub danger: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for FakeMixer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

struct Sub {
    addr: SocketAddr,
    expires: Instant,
}

struct MeterSub {
    addr: SocketAddr,
    bank: u8,
    remaining: u32,
    next: Instant,
}

impl FakeMixer {
    pub fn spawn(cfg: FakeConfig) -> std::io::Result<FakeMixer> {
        let sock = UdpSocket::bind("127.0.0.1:0")?;
        sock.set_read_timeout(Some(Duration::from_millis(20)))?;
        let addr = sock.local_addr()?;
        let received = Arc::new(Mutex::new(Vec::new()));
        let danger = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (rx2, dg2, st2) = (received.clone(), danger.clone(), stop.clone());
        let join = std::thread::Builder::new()
            .name("fake-mixer".into())
            .spawn(move || run(sock, cfg, rx2, dg2, st2))?;
        Ok(FakeMixer { addr, received, danger, stop, join: Some(join) })
    }

    /// Number of datagrams received so far.
    pub fn received_count(&self) -> usize {
        self.received.lock().unwrap().len()
    }
}

/// Only ever answer loopback peers. The fake binds loopback so this cannot
/// fire, but if it ever did we want a hard stop, not a stray packet.
fn jailed_send(sock: &UdpSocket, dest: SocketAddr, bytes: &[u8]) {
    if !dest.ip().is_loopback() {
        panic!("fake mixer asked to send to non-loopback {}", dest);
    }
    let _ = sock.send_to(bytes, dest);
}

fn seed_scene() -> HashMap<String, Vec<OscArg>> {
    let mut m: HashMap<String, Vec<OscArg>> = HashMap::new();
    // Defaults for the whole whitelisted vocabulary.
    for p in Param::surface_sweep() {
        let addr = p.addr();
        let v = match p.spec() {
            crate::safety::ValueSpec::Float01 => {
                if addr.ends_with("/fader") || addr.ends_with("/level") {
                    OscArg::F(0.75)
                } else {
                    OscArg::F(0.5)
                }
            }
            crate::safety::ValueSpec::Int { .. } => {
                if addr.ends_with("/mix/on") || addr.ends_with("/mix/lr") || addr.ends_with("eq/on")
                {
                    OscArg::I(1)
                } else {
                    OscArg::I(0)
                }
            }
            crate::safety::ValueSpec::Name => OscArg::S(String::new()),
        };
        m.insert(addr, vec![v]);
    }
    // Phantom power exists on a real console; the fake stores it so that a
    // *hypothetical* write would be visible in test assertions. The client
    // can neither read nor write it.
    for c in Ch::all() {
        m.insert(format!("/headamp/{:02}/phantom", c.get()), vec![OscArg::I(0)]);
    }

    let mut set = |addr: String, v: OscArg| {
        m.insert(addr, vec![v]);
    };
    // The reference session, the way the real desk is actually configured:
    // console stereo links OFF for most pairs — the pairs exist by NAMING
    // CONVENTION (even half = "<odd> R", hard-panned L/R). One channel pair
    // (15-16) and one bus pair (1-2) are console-linked so both pairing
    // paths stay exercised.
    for p in ChPair::all() {
        let linked = p.get() == 15;
        set(Param::ChLink(p).addr(), OscArg::I(if linked { 1 } else { 0 }));
    }
    set(Param::BusLink(BusPair::new(1).unwrap()).addr(), OscArg::I(1));
    set(Param::BusLink(BusPair::new(3).unwrap()).addr(), OscArg::I(0));
    set(Param::BusLink(BusPair::new(5).unwrap()).addr(), OscArg::I(0));

    let ch = |n: u8| Ch::new(n).unwrap();
    let bus = |n: u8| BusN::new(n).unwrap();
    let names: &[(u8, &str, i32)] = &[
        (1, "TV", 7),
        (3, "M3", 3),
        (5, "DJ", 5),
        (7, "Mac", 6),
        (9, "Korg", 1),
        (11, "TR8S", 2),
        (13, "Drums", 4),
        (15, "Loopy", 3),
    ];
    for (n, name, color) in names {
        set(Param::ChConfigName(ch(*n)).addr(), OscArg::S(name.to_string()));
        set(Param::ChConfigColor(ch(*n)).addr(), OscArg::I(*color));
        // even half: "<name> R", same colour, hard-panned pair
        set(
            Param::ChConfigName(ch(*n + 1)).addr(),
            OscArg::S(format!("{} R", name)),
        );
        set(Param::ChConfigColor(ch(*n + 1)).addr(), OscArg::I(*color));
        set(Param::ChMixPan(ch(*n)).addr(), OscArg::F(0.0)); // L100
        set(Param::ChMixPan(ch(*n + 1)).addr(), OscArg::F(1.0)); // R100
    }
    let busnames: &[(u8, &str, i32)] =
        &[(1, "Speaker", 2), (3, "TV", 7), (5, "BassBaby", 4), (6, "Sub", 1)];
    for (n, name, color) in busnames {
        set(Param::BusConfigName(bus(*n)).addr(), OscArg::S(name.to_string()));
        set(Param::BusConfigColor(bus(*n)).addr(), OscArg::I(*color));
    }
    // bus 3-4 is a virtual pair by naming
    set(Param::BusConfigName(bus(4)).addr(), OscArg::S("TV R".into()));
    set(Param::BusConfigColor(bus(4)).addr(), OscArg::I(7));
    set(Param::LrConfigName.addr(), OscArg::S("Headphones".into()));
    set(Param::LrConfigColor.addr(), OscArg::I(6));

    let faders: &[(u8, f32)] = &[
        (1, -9.1),
        (3, -17.2),
        (5, -89.5),
        (7, -1.1),
        (9, -4.3),
        (11, -0.7),
        (13, -76.9),
    ];
    for (n, db) in faders {
        set(Param::ChMixFader(ch(*n)).addr(), OscArg::F(db_to_level(*db)));
        set(Param::ChMixFader(ch(*n + 1)).addr(), OscArg::F(db_to_level(*db)));
    }
    set(Param::ChMixFader(ch(15)).addr(), OscArg::F(0.0)); // closed: -inf
    set(Param::ChMixFader(ch(16)).addr(), OscArg::F(0.0));
    set(Param::ChMixOn(ch(15)).addr(), OscArg::I(0)); // and muted
    set(Param::ChMixOn(ch(16)).addr(), OscArg::I(0));
    set(Param::BusMixFader(bus(1)).addr(), OscArg::F(db_to_level(-8.0)));
    set(Param::BusMixFader(bus(3)).addr(), OscArg::F(db_to_level(-12.4)));
    set(Param::BusMixFader(bus(5)).addr(), OscArg::F(db_to_level(-6.2)));
    set(Param::BusMixFader(bus(6)).addr(), OscArg::F(db_to_level(-20.5)));
    set(Param::LrMixFader.addr(), OscArg::F(db_to_level(-8.9)));

    // Preamp gains (-12..+60 linear): a spread of plausible values.
    let gains: &[(u8, f32)] = &[
        (1, 0.0),
        (3, 6.3),
        (5, -12.0),
        (7, 15.0),
        (9, 8.8),
        (11, 12.5),
        (13, 30.0),
        (15, 16.5),
    ];
    for (n, db) in gains {
        let g = crate::units::unit_to_lin(-12.0, 60.0, *db);
        set(Param::HeadampGain(ch(*n)).addr(), OscArg::F(g));
        set(Param::HeadampGain(ch(*n + 1)).addr(), OscArg::F(g));
    }

    // A few non-flat EQs so curves show shape (g: 0.5 = flat).
    use crate::safety::{EqBand, EqLeaf};
    let band = |n: u8| EqBand::new(n).unwrap();
    // TV: low shelf -4 dB, presence bump +3 dB
    set(Param::ChEq(ch(1), band(1), EqLeaf::Type).addr(), OscArg::I(1));
    set(Param::ChEq(ch(1), band(1), EqLeaf::G).addr(), OscArg::F(0.37));
    set(Param::ChEq(ch(1), band(3), EqLeaf::G).addr(), OscArg::F(0.60));
    set(Param::ChEq(ch(1), band(3), EqLeaf::F).addr(), OscArg::F(0.70));
    // Drums: high shelf +5 dB, low cut
    set(Param::ChEq(ch(13), band(4), EqLeaf::Type).addr(), OscArg::I(4));
    set(Param::ChEq(ch(13), band(4), EqLeaf::G).addr(), OscArg::F(0.67));
    set(Param::ChEq(ch(13), band(1), EqLeaf::Type).addr(), OscArg::I(0));
    set(Param::ChEq(ch(13), band(1), EqLeaf::F).addr(), OscArg::F(0.15));
    // Korg: mid scoop -6 dB
    set(Param::ChEq(ch(9), band(2), EqLeaf::G).addr(), OscArg::F(0.30));
    set(Param::ChEq(ch(9), band(2), EqLeaf::F).addr(), OscArg::F(0.45));

    // Dynamics: Drums compressing 4:1 at -18, DJ gate at -45.
    use crate::safety::{DynLeaf, GateLeaf};
    set(Param::ChDyn(ch(13), DynLeaf::On).addr(), OscArg::I(1));
    set(Param::ChDyn(ch(13), DynLeaf::Thr).addr(), OscArg::F(0.70)); // -18 dB
    set(Param::ChDyn(ch(13), DynLeaf::Ratio).addr(), OscArg::I(6)); // 4:1
    set(Param::ChGate(ch(5), GateLeaf::On).addr(), OscArg::I(1));
    set(Param::ChGate(ch(5), GateLeaf::Thr).addr(), OscArg::F(0.4375)); // -45 dB
    set(Param::LrDyn(DynLeaf::On).addr(), OscArg::I(1));
    set(Param::LrDyn(DynLeaf::Thr).addr(), OscArg::F(0.83)); // -10 dB
    set(Param::LrDyn(DynLeaf::Ratio).addr(), OscArg::I(3)); // 2:1
    m
}

fn run(
    sock: UdpSocket,
    cfg: FakeConfig,
    received: Arc<Mutex<Vec<OscMsg>>>,
    danger: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
) {
    let mut state = seed_scene();
    let mut subs: Vec<Sub> = Vec::new();
    let mut meter_subs: Vec<MeterSub> = Vec::new();
    let start = Instant::now();
    let mut last_wiggle = Instant::now();
    let mut buf = [0u8; 2048];

    while !stop.load(Ordering::Relaxed) {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                let Ok(msg) = OscMsg::decode(&buf[..n]) else { continue };
                received.lock().unwrap().push(msg.clone());
                if let Some(term) = deny_term(&msg.addr) {
                    eprintln!(
                        "[fake-mixer] DANGER: received deny-listed {:?} (term {:?}) — dropped",
                        msg.addr, term
                    );
                    danger.lock().unwrap().push(msg.addr.clone());
                    continue;
                }
                handle(&sock, &cfg, &mut state, &mut subs, &mut meter_subs, msg, from);
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }

        let now = Instant::now();
        subs.retain(|s| s.expires > now);

        // Meter streams: ~50 ms cadence, 200 frames per subscription.
        for ms in meter_subs.iter_mut() {
            while ms.remaining > 0 && ms.next <= now {
                let t = start.elapsed().as_secs_f32();
                let blob = meter_blob(ms.bank, t, &state);
                let msg = OscMsg::with_args(
                    &format!("/meters/{}", ms.bank),
                    vec![OscArg::B(blob)],
                );
                jailed_send(&sock, ms.addr, &msg.encode());
                ms.remaining -= 1;
                ms.next += Duration::from_millis(50);
            }
        }
        meter_subs.retain(|ms| ms.remaining > 0);

        // A "second controller": wiggle the DJ fader every couple of
        // seconds and push the change to subscribers.
        if cfg.animate && last_wiggle.elapsed() > Duration::from_millis(2000) {
            last_wiggle = now;
            let t = start.elapsed().as_secs_f32();
            let v = 0.55 + 0.18 * (t * 0.35).sin();
            for addr in ["/ch/07/mix/fader", "/ch/08/mix/fader"] {
                state.insert(addr.to_string(), vec![OscArg::F(v)]);
                let bytes = OscMsg::with_args(addr, vec![OscArg::F(v)]).encode();
                for s in &subs {
                    jailed_send(&sock, s.addr, &bytes);
                }
            }
        }
    }
}

fn handle(
    sock: &UdpSocket,
    cfg: &FakeConfig,
    state: &mut HashMap<String, Vec<OscArg>>,
    subs: &mut Vec<Sub>,
    meter_subs: &mut Vec<MeterSub>,
    msg: OscMsg,
    from: SocketAddr,
) {
    match msg.addr.as_str() {
        "/xinfo" | "/info" => {
            let args = if cfg.xinfo_official_order {
                // official 4-pager order: server_version, name, model, version
                vec![
                    OscArg::S("2.08".into()),
                    OscArg::S("FAKE18".into()),
                    OscArg::S("XR18".into()),
                    OscArg::S("1.30".into()),
                ]
            } else {
                // community-observed order: ip, name, model, firmware
                vec![
                    OscArg::S("127.0.0.1".into()),
                    OscArg::S("FAKE18".into()),
                    OscArg::S("XR18".into()),
                    OscArg::S("1.30".into()),
                ]
            };
            jailed_send(sock, from, &OscMsg::with_args("/xinfo", args).encode());
        }
        "/status" => {
            let args = vec![
                OscArg::S("active".into()),
                OscArg::S("127.0.0.1".into()),
                OscArg::S("FAKE18".into()),
            ];
            jailed_send(sock, from, &OscMsg::with_args("/status", args).encode());
        }
        "/xremote" => {
            let expires = Instant::now() + Duration::from_secs(10);
            if let Some(s) = subs.iter_mut().find(|s| s.addr == from) {
                s.expires = expires;
            } else if subs.len() < 8 {
                subs.push(Sub { addr: from, expires });
            }
        }
        "/meters" => {
            // ,s "/meters/N" (optional ,i channel — ignored here)
            if let Some(OscArg::S(id)) = msg.args.first() {
                if let Some(bank) = id.strip_prefix("/meters/").and_then(|b| b.parse::<u8>().ok())
                {
                    meter_subs.retain(|m| !(m.addr == from && m.bank == bank));
                    meter_subs.push(MeterSub {
                        addr: from,
                        bank,
                        remaining: 200,
                        next: Instant::now(),
                    });
                }
            }
        }
        _ => {
            if msg.args.is_empty() {
                // GET: answer if we know the address; silence otherwise.
                if let Some(args) = state.get(&msg.addr) {
                    let reply = OscMsg::with_args(&msg.addr, args.clone());
                    jailed_send(sock, from, &reply.encode());
                }
            } else {
                // SET: store, echo to the setter, fan out to the OTHER
                // subscribers (that is how the real console behaves).
                if state.contains_key(&msg.addr) {
                    state.insert(msg.addr.clone(), msg.args.clone());
                    let bytes = msg.encode();
                    jailed_send(sock, from, &bytes);
                    for s in subs.iter() {
                        if s.addr != from {
                            jailed_send(sock, s.addr, &bytes);
                        }
                    }
                }
            }
        }
    }
}

/// Synthesized meter banks. `/meters/1`: 40 slots (16 ch pre, aux L/R,
/// fx1-4 L/R, bus1-6, fxsend1-4, main L/R, monitor L/R). `/meters/6`: 39
/// gain-reduction slots. Little-endian count + little-endian int16 samples.
fn meter_blob(bank: u8, t: f32, state: &HashMap<String, Vec<OscArg>>) -> Vec<u8> {
    let fader = |addr: &str| -> f32 {
        match state.get(addr) {
            Some(v) => match v.first() {
                Some(OscArg::F(f)) => *f,
                _ => 0.0,
            },
            None => 0.0,
        }
    };
    let count: usize = if bank == 6 { 39 } else { 40 };
    let mut vals = vec![i16::MIN; count];
    if bank == 1 {
        for i in 0..16usize {
            // Channels with an open fader "play"; each gets its own motion.
            let f = fader(&format!("/ch/{:02}/mix/fader", i + 1));
            if f > 0.01 {
                let phase = i as f32 * 1.7;
                let wob = (t * (1.1 + i as f32 * 0.13) + phase).sin() * 0.5 + 0.5;
                let burst = ((t * 0.43 + phase).sin() * 3.0).max(0.0).min(1.0);
                let db = -54.0 + 40.0 * wob * (0.35 + 0.65 * burst);
                vals[i] = (db * 256.0) as i16;
            }
        }
        for (k, b) in (26..32).enumerate() {
            let f = fader(&format!("/bus/{}/mix/fader", k + 1));
            if f > 0.01 {
                let db = -40.0 + 26.0 * ((t * 0.9 + k as f32).sin() * 0.5 + 0.5);
                vals[b] = (db * 256.0) as i16;
            }
        }
        let main = -24.0 + 16.0 * ((t * 1.3).sin() * 0.5 + 0.5);
        vals[36] = (main * 256.0) as i16;
        vals[37] = ((main - 1.5) * 256.0) as i16;
    } else if bank == 6 {
        // Occasional gain reduction on the compressing channels.
        let gr = ((t * 2.1).sin() * 6.0).min(0.0);
        vals[16 + 12] = (gr * 256.0) as i16; // ch 13 dyn GR
        vals[38] = ((gr * 0.5) * 256.0) as i16; // LR dyn GR
        for v in vals.iter_mut() {
            if *v == i16::MIN {
                *v = 0;
            }
        }
    }
    let mut payload = Vec::with_capacity(4 + count * 2);
    payload.extend_from_slice(&(count as i32).to_le_bytes());
    for v in vals {
        payload.extend_from_slice(&v.to_le_bytes());
    }
    payload
}
