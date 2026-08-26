//! The client session thread: one guarded socket, one background loop.
//!
//! Sends happen ONLY in response to explicit commands (user action) or the
//! documented keep-alives of an established session:
//!   - `Cmd::Scan`      -> one `/xinfo` to the user-chosen target
//!   - `Cmd::Connect`   -> `/xinfo`, `/xremote`, `/meters` subscribe, then
//!                         a paced GET sweep of the whitelisted surface
//!   - `Cmd::Set`       -> one whitelisted SET
//!   - while connected  -> `/xremote` + `/meters` renewals every 8 s
//! Nothing is sent at startup, ever.
//!
//! This app is never the only controller: the console's state is the truth
//! and other clients change it underneath us. The session only *reads*
//! (GET, subscriptions) plus the user's own gestures — it never re-asserts
//! remembered state, so it cannot fight another controller.

use crate::model::{decode_meter_blob, DeviceInfo};
use crate::osc::OscMsg;
use crate::safety::{GuardedSocket, MeterBank, PVal, Param, SafeMsg, ValueSpec};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum Cmd {
    /// Send one `/xinfo` to `target` ("ip" or "ip:port"; ".255" targets use
    /// the socket's broadcast permission). User-initiated only.
    Scan { target: String },
    Connect { addr: SocketAddr },
    Disconnect,
    Set(Param, PVal),
    /// Re-query a single parameter (a GET; used by the re-request pass for
    /// values the console never answered).
    Get(Param),
    /// Control gate. It starts CLOSED on every session — the app opens it the
    /// moment a console is connected. While closed every `Set` is dropped with a
    /// note until the user explicitly enables control. Subscriptions and
    /// GETs (all read-only) are unaffected.
    SetControl(bool),
    Quit,
}

#[derive(Debug)]
pub enum Evt {
    /// A discovery reply. `from` is the authoritative address to connect to.
    Found { info: DeviceInfo, from: SocketAddr },
    Connected { addr: SocketAddr },
    Disconnected,
    /// `/xinfo` answered on the connected session.
    Info(DeviceInfo),
    Param(Param, PVal),
    Meters { bank: u8, vals: Vec<f32> },
    /// Periodic honesty report while connected.
    Health { rx_age: Duration, stale: bool, sweep_left: usize },
    Note(String),
}

pub struct Client {
    cmd_tx: Sender<Cmd>,
    pub evt_rx: Receiver<Evt>,
    pub local_addr: SocketAddr,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Client {
    /// Starts the session thread on a real network socket. `notify` is
    /// called after new events are queued (post a UI signal).
    pub fn start(notify: Arc<dyn Fn() + Send + Sync>) -> std::io::Result<Client> {
        Self::start_inner(false, notify)
    }

    /// TESTS ONLY: the same client on a socket jailed to loopback, so a
    /// test run cannot reach a real console even if it tried.
    #[cfg(test)]
    pub fn start_jailed(notify: Arc<dyn Fn() + Send + Sync>) -> std::io::Result<Client> {
        Self::start_inner(true, notify)
    }

    fn start_inner(
        loopback_only: bool,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> std::io::Result<Client> {
        let sock = GuardedSocket::bind(loopback_only)?;
        let local_addr = sock.local_addr()?;
        let (cmd_tx, cmd_rx) = channel();
        let (evt_tx, evt_rx) = channel();
        let join = std::thread::Builder::new()
            .name("mixer-client".into())
            .spawn(move || run(sock, cmd_rx, evt_tx, notify))?;
        Ok(Client { cmd_tx, evt_rx, local_addr, join: Some(join) })
    }

    pub fn cmd(&self, c: Cmd) {
        let _ = self.cmd_tx.send(c);
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Cmd::Quit);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Best-effort default scan target: the local /24 directed broadcast.
/// Derived without sending anything (UDP connect() only sets a default
/// peer). The user can always overwrite the field.
pub fn guess_directed_broadcast() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("198.51.100.1:9").ok()?; // TEST-NET-2; no packet leaves
    match s.local_addr().ok()? {
        SocketAddr::V4(a) => {
            let o = a.ip().octets();
            Some(format!("{}.{}.{}.255", o[0], o[1], o[2]))
        }
        _ => None,
    }
}

pub fn parse_target(s: &str) -> Result<SocketAddr, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err("enter a mixer address first".into());
    }
    if let Ok(sa) = t.parse::<SocketAddr>() {
        return Ok(sa);
    }
    if let Ok(ip) = t.parse::<std::net::IpAddr>() {
        return Ok(SocketAddr::new(ip, 10024));
    }
    Err(format!("not an address: {:?} (use ip or ip:port)", t))
}

const XREMOTE_RENEW: Duration = Duration::from_secs(8);
/// Meter subscriptions self-expire after 200 frames / 10 s; renew early so
/// the streams overlap instead of gapping.
const METERS_RENEW: Duration = Duration::from_secs(7);
const STALE_AFTER: Duration = Duration::from_secs(12);
/// The dialect has no bulk query, so initial state is ~900 individual GETs.
/// They are fire-and-forget over UDP — replies land asynchronously — so the
/// only pacing is a small batch window to avoid flooding the console.
// Pacing the initial read. The console answers from a small UDP queue: ask
// too fast and it simply drops answers (a 64-per-15-ms sweep lost roughly a
// quarter of them, scribble names and colours among them). ~800 gets/sec
// reads the whole surface in about two seconds with nothing dropped.
/// How often the app re-asks the network for a console while it has none.
const SEARCH_EVERY: Duration = Duration::from_millis(1000);
const SWEEP_BATCH: usize = 8;
const SWEEP_TICK: Duration = Duration::from_millis(10);

struct Session {
    addr: SocketAddr,
    sweep: VecDeque<Param>,
    next_sweep: Instant,
    next_xremote: Instant,
    next_meters: Instant,
    next_health: Instant,
    last_rx: Instant,
    started: Instant,
    sweep_done_logged: bool,
    // meter cadence instrumentation (bank 1): arrival gaps
    meter_last: Option<Instant>,
    meter_count: u32,
    meter_gap_sum: f32,
    meter_gap_max: f32,
}

fn run(
    sock: GuardedSocket,
    cmd_rx: Receiver<Cmd>,
    evt_tx: Sender<Evt>,
    notify: Arc<dyn Fn() + Send + Sync>,
) {
    let mut session: Option<Session> = None;
    // The gate starts closed: a fresh session cannot move anything on the
    // console until the app opens it (which it does as soon as the session
    // is up — the surface is live by design).
    let mut control_enabled = false;
    let rx_log = std::env::var("MIXER_RX_LOG").is_ok();
    // The search target stays armed: while no console is connected, the
    // same single /xinfo query goes out about once a second, so the app
    // finds the desk whenever it appears (or comes back).
    let mut search: Option<SocketAddr> = None;
    let mut next_search = Instant::now();
    let mut buf = [0u8; 4096];

    'outer: loop {
        let mut pushed = false;
        let push = |e: Evt, pushed: &mut bool| {
            let _ = evt_tx.send(e);
            *pushed = true;
        };

        // ---- commands -------------------------------------------------
        loop {
            match cmd_rx.try_recv() {
                Ok(Cmd::Quit) => break 'outer,
                Ok(Cmd::Scan { target }) => match parse_target(&target) {
                    Ok(dest) => {
                        search = Some(dest);
                        next_search = Instant::now();
                    }
                    Err(e) => push(Evt::Note(format!("scan: {}", e)), &mut pushed),
                },
                Ok(Cmd::Connect { addr }) => {
                    let now = Instant::now();
                    let mut s = Session {
                        addr,
                        sweep: Param::surface_sweep().into(),
                        next_sweep: now,
                        next_xremote: now + XREMOTE_RENEW,
                        next_meters: now + METERS_RENEW,
                        next_health: now + Duration::from_secs(1),
                        last_rx: now,
                        started: now,
                        sweep_done_logged: false,
                        meter_last: None,
                        meter_count: 0,
                        meter_gap_sum: 0.0,
                        meter_gap_max: 0.0,
                    };
                    let _ = sock.send(addr, &SafeMsg::xinfo());
                    let _ = sock.send(addr, &SafeMsg::xremote());
                    let _ = sock.send(addr, &SafeMsg::meters_subscribe(MeterBank::Channels));
                    let _ = sock.send(addr, &SafeMsg::meters_subscribe(MeterBank::Dynamics));
                    s.last_rx = now;
                    session = Some(s);
                    push(Evt::Connected { addr }, &mut pushed);
                }
                Ok(Cmd::Disconnect) => {
                    session = None;
                    push(Evt::Disconnected, &mut pushed);
                }
                Ok(Cmd::SetControl(on)) => {
                    control_enabled = on;
                    // Mechanism, not policy: the socket itself refuses
                    // argument-carrying packets while read-only.
                    sock.set_read_only(!on);
                    push(
                        Evt::Note(if on {
                            "CONTROL ENABLED — gestures now transmit".to_string()
                        } else {
                            "view-only — gestures do not transmit".to_string()
                        }),
                        &mut pushed,
                    );
                }
                Ok(Cmd::Get(p)) => {
                    if let Some(s) = &session {
                        let _ = sock.send(s.addr, &p.get());
                    }
                }
                Ok(Cmd::Set(p, v)) => {
                    if !control_enabled {
                        push(
                            Evt::Note("view-only mode: change NOT sent (enable CONTROL first)".to_string()),
                            &mut pushed,
                        );
                        continue;
                    }
                    if let Some(s) = &session {
                        match p.set(&v) {
                            Ok(msg) => {
                                if let Err(e) = sock.send(s.addr, &msg) {
                                    push(Evt::Note(format!("set: {}", e)), &mut pushed);
                                }
                            }
                            Err(e) => push(Evt::Note(format!("set refused: {:?}", e)), &mut pushed),
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'outer,
            }
        }

        // ---- keep looking while nothing is connected ------------------
        if session.is_none() {
            if let Some(dest) = search {
                let now = Instant::now();
                if now >= next_search {
                    next_search = now + SEARCH_EVERY;
                    if let Err(e) = sock.send(dest, &SafeMsg::xinfo()) {
                        push(Evt::Note(format!("search: {}", e)), &mut pushed);
                    }
                }
            }
        }

        // ---- receive (bounded per iteration) --------------------------
        for _ in 0..64 {
            match sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    let Ok(msg) = OscMsg::decode(&buf[..n]) else { continue };
                    let connected_from =
                        session.as_ref().map(|s| s.addr == from).unwrap_or(false);
                    if connected_from {
                        if let Some(s) = session.as_mut() {
                            s.last_rx = Instant::now();
                        }
                    }
                    if msg.addr == "/xinfo" || msg.addr == "/info" {
                        let info = DeviceInfo::from_args(&msg.args);
                        if connected_from {
                            push(Evt::Info(info), &mut pushed);
                        } else {
                            push(Evt::Found { info, from }, &mut pushed);
                        }
                    } else if let Some(bank) = msg
                        .addr
                        .strip_prefix("/meters/")
                        .and_then(|b| b.parse::<u8>().ok())
                    {
                        if let Some(crate::osc::OscArg::B(payload)) = msg.args.first() {
                            if let Some(vals) = decode_meter_blob(payload) {
                                if bank == 1 {
                                    if let Some(s) = session.as_mut() {
                                        let now = Instant::now();
                                        if let Some(last) = s.meter_last {
                                            let gap =
                                                now.duration_since(last).as_secs_f32() * 1000.0;
                                            s.meter_gap_sum += gap;
                                            s.meter_gap_max = s.meter_gap_max.max(gap);
                                        }
                                        s.meter_last = Some(now);
                                        s.meter_count += 1;
                                        if s.meter_count % 200 == 0 {
                                            println!(
                                                "[mixer-meters] {} frames, avg gap {:.1} ms, max {:.1} ms",
                                                s.meter_count,
                                                s.meter_gap_sum / (s.meter_count.max(2) - 1) as f32,
                                                s.meter_gap_max
                                            );
                                            s.meter_gap_max = 0.0;
                                        }
                                    }
                                }
                                push(Evt::Meters { bank, vals }, &mut pushed);
                            }
                        }
                    } else if let Some(p) = Param::parse(&msg.addr) {
                        // Stereo-link answers decide the whole surface —
                        // log them so a live session shows what the console
                        // actually reported. MIXER_RX_LOG=1 logs every
                        // answer instead (what the desk really holds).
                        if rx_log || matches!(p, Param::ChLink(_) | Param::BusLink(_)) {
                            println!("[mixer-wire] RX {} {:?}", msg.addr, msg.args);
                        }
                        if let Some(v) = pval_from_reply(p, &msg) {
                            push(Evt::Param(p, v), &mut pushed);
                        } else if rx_log {
                            println!(
                                "[mixer-wire] RX-SHAPE-REJECT {} {:?}",
                                msg.addr, msg.args
                            );
                        }
                    } else if rx_log {
                        // An answer we have no whitelist slot for: this is
                        // where missing metadata shows up.
                        println!("[mixer-wire] RX-UNKNOWN {} {:?}", msg.addr, msg.args);
                    }
                    // Anything else (including any dangerous family a
                    // hostile peer might echo at us) is ignored.
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }

        // ---- session timers ------------------------------------------
        if let Some(s) = session.as_mut() {
            let now = Instant::now();
            if !s.sweep.is_empty() && now >= s.next_sweep {
                for _ in 0..SWEEP_BATCH {
                    let Some(p) = s.sweep.pop_front() else { break };
                    let _ = sock.send(s.addr, &p.get());
                }
                s.next_sweep = now + SWEEP_TICK;
                if s.sweep.is_empty() && !s.sweep_done_logged {
                    s.sweep_done_logged = true;
                    println!(
                        "[mixer] initial GET sweep sent in {:.0} ms",
                        s.started.elapsed().as_secs_f32() * 1000.0
                    );
                }
            }
            if now >= s.next_xremote {
                let _ = sock.send(s.addr, &SafeMsg::xremote());
                s.next_xremote = now + XREMOTE_RENEW;
            }
            if now >= s.next_meters {
                let _ = sock.send(s.addr, &SafeMsg::meters_subscribe(MeterBank::Channels));
                let _ = sock.send(s.addr, &SafeMsg::meters_subscribe(MeterBank::Dynamics));
                s.next_meters = now + METERS_RENEW;
            }
            if now >= s.next_health {
                let age = s.last_rx.elapsed();
                push(
                    Evt::Health {
                        rx_age: age,
                        stale: age > STALE_AFTER,
                        sweep_left: s.sweep.len(),
                    },
                    &mut pushed,
                );
                s.next_health = now + Duration::from_secs(1);
            }
        }

        if pushed {
            notify();
        }
    }
}

/// Converts a console reply into a typed value — only when the argument
/// shape matches the parameter's spec. Mismatches are dropped, not coerced.
fn pval_from_reply(p: Param, msg: &OscMsg) -> Option<PVal> {
    use crate::osc::OscArg;
    match (p.spec(), msg.args.first()?) {
        (ValueSpec::Float01, OscArg::F(f)) => Some(PVal::F(*f)),
        (ValueSpec::Int { .. }, OscArg::I(i)) => Some(PVal::I(*i)),
        (ValueSpec::Name, OscArg::S(s)) => Some(PVal::S(s.clone())),
        _ => None,
    }
}
