//! Live-wire safety tests: a real client socket against the in-process fake
//! mixer, over UDP loopback only. These are the tests the whole lane hangs
//! on — they prove that every forbidden address is refused BEFORE the
//! socket, that the whitelist is the only way onto the wire, and that the
//! normal session flow behaves.

use crate::client::{parse_target, Client, Cmd, Evt};
use crate::fake::{FakeConfig, FakeMixer};
use crate::model::{DeviceInfo, MixerModel};
use crate::osc::{OscArg, OscMsg};
use crate::safety::{
    Ch, GuardedSocket, MeterBank, PVal, Param, Refused, SafeMsg,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Every address family from the danger table. The tests push each one at
/// the transmit chokepoint and assert NOTHING reaches the fake's socket.
const FORBIDDEN: &[&str] = &[
    "/headamp/01/phantom",
    "/headamp/16/phantom",
    "/-snap/load",
    "/-snap/save",
    "/-snap/01/name",
    "/snap/load",
    "/-action/initall",
    "/-action/clearsolo",
    "/-action/setclock",
    "/-prefs/clockrate",
    "/-prefs/usbifcmode",
    "/-prefs/ap/ssid",
    "/-stat/tape/state",
    "/-stat/solosw/01",
    "/-stat/usb/path",
    "/-libs/ch/01",
    "/-show/prepos",
    "/showdump",
    "/load",
    "/save",
    "/-usb/path",
    "/routing/main/01",
    "/config/routing/IN",
    "/ch/01/config/insrc",
    "/ch/01/preamp/rtnsrc",
];

fn wait_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn drain_until<F: FnMut(&Evt) -> bool>(
    client: &Client,
    timeout: Duration,
    mut pred: F,
) -> Vec<Evt> {
    let start = Instant::now();
    let mut got = Vec::new();
    while start.elapsed() < timeout {
        match client.evt_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(e) => {
                let done = pred(&e);
                got.push(e);
                if done {
                    return got;
                }
            }
            Err(_) => {}
        }
    }
    got
}

// ---------------------------------------------------------------------------
// The deny list, exercised at the socket boundary.
// ---------------------------------------------------------------------------

#[test]
fn deny_list_blocks_every_forbidden_address_before_the_socket() {
    let fake = FakeMixer::spawn(FakeConfig { animate: false, ..Default::default() }).unwrap();
    let sock = GuardedSocket::bind(true).unwrap();

    for addr in FORBIDDEN {
        // A malicious/buggy caller tries a SET (argument = write!) ...
        let set = OscMsg::with_args(addr, vec![OscArg::I(1)]);
        let r = sock.hostile_transmit(fake.addr, &set);
        assert!(
            matches!(r, Err(Refused::Denied { .. })),
            "SET to {} must be refused at the transmit gate, got {:?}",
            addr,
            r
        );
        // ... and a "harmless" GET of the same address is refused too.
        let get = OscMsg::query(addr);
        let r = sock.hostile_transmit(fake.addr, &get);
        assert!(
            matches!(r, Err(Refused::Denied { .. })),
            "GET of {} must be refused at the transmit gate",
            addr
        );
        println!("DENY ok: {} (set+get refused before socket)", addr);
    }

    // Give any leaked datagram time to arrive, then assert the wire stayed
    // silent AND the fake's own danger tripwire never fired.
    wait_ms(150);
    assert_eq!(
        fake.received_count(),
        0,
        "forbidden addresses reached the socket: {:?}",
        fake.received.lock().unwrap()
    );
    assert!(fake.danger.lock().unwrap().is_empty());

    // Positive control: the SAME socket, the SAME path, a whitelisted
    // message — exactly one datagram arrives. This proves the deny
    // assertions above weren't passing vacuously. (The socket boots in
    // stage-1 read-only mode, so writes need the explicit flip.)
    sock.set_read_only(false);
    let msg = Param::ChMixFader(Ch::new(1).unwrap()).set(&PVal::F(0.75)).unwrap();
    sock.send(fake.addr, &msg).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while fake.received_count() < 1 && Instant::now() < deadline {
        wait_ms(10);
    }
    let rx = fake.received.lock().unwrap();
    assert_eq!(rx.len(), 1, "expected exactly the one whitelisted packet");
    assert_eq!(rx[0].addr, "/ch/01/mix/fader");
    assert_eq!(rx[0].args, vec![OscArg::F(0.75)]);
    println!("POSITIVE CONTROL ok: whitelisted /ch/01/mix/fader reached the fake");
}

#[test]
fn loopback_jail_refuses_real_network_destinations() {
    let sock = GuardedSocket::bind(true).unwrap();
    // TEST-NET-1: documentation range, never assigned locally. If the jail
    // failed this packet still could not reach anyone's hardware.
    let dest: std::net::SocketAddr = "192.0.2.1:10024".parse().unwrap();
    let r = sock.send(dest, &SafeMsg::xinfo());
    assert!(
        matches!(r, Err(Refused::NotLoopback(_))),
        "loopback jail must refuse non-loopback destinations, got {:?}",
        r
    );
    println!("LOOPBACK JAIL ok: non-loopback destination refused");
}

#[test]
fn wrong_argument_shapes_are_rejected_not_coerced() {
    // (Compile-time note: SafeMsg has no raw-address constructor, so this
    // test can only reach the wire through Param — that IS the whitelist.)
    let fake = FakeMixer::spawn(FakeConfig { animate: false, ..Default::default() }).unwrap();
    let sock = GuardedSocket::bind(true).unwrap();

    let fader = Param::ChMixFader(Ch::new(2).unwrap());
    let mute = Param::ChMixOn(Ch::new(2).unwrap());
    assert!(fader.set(&PVal::I(1)).is_err(), "int into float param");
    assert!(fader.set(&PVal::F(2.0)).is_err(), "float out of wire range");
    assert!(fader.set(&PVal::F(f32::NAN)).is_err(), "NaN");
    assert!(mute.set(&PVal::F(0.0)).is_err(), "float into int param");
    assert!(mute.set(&PVal::I(-1)).is_err(), "negative bool");
    assert!(
        Param::ChConfigName(Ch::new(2).unwrap())
            .set(&PVal::S("thirteen chars".into()))
            .is_err(),
        "over-long name"
    );

    wait_ms(100);
    assert_eq!(fake.received_count(), 0, "rejected values must never hit the wire");
    // and a correct one goes through once control is explicitly enabled
    sock.set_read_only(false);
    sock.send(fake.addr, &mute.set(&PVal::I(0)).unwrap()).unwrap();
    wait_ms(200);
    assert_eq!(fake.received_count(), 1);
    println!("SHAPE ok: mis-typed arguments rejected before the socket");
}

// ---------------------------------------------------------------------------
// Session behaviour against the fake.
// ---------------------------------------------------------------------------

#[test]
fn discovery_parses_both_xinfo_argument_orders() {
    for official in [false, true] {
        let fake = FakeMixer::spawn(FakeConfig {
            xinfo_official_order: official,
            animate: false,
        })
        .unwrap();
        let client = Client::start_jailed(Arc::new(|| {})).unwrap();
        client.cmd(Cmd::Scan { target: fake.addr.to_string() });
        let evts = drain_until(&client, Duration::from_secs(3), |e| {
            matches!(e, Evt::Found { .. })
        });
        let found = evts.iter().find_map(|e| match e {
            Evt::Found { info, from } => Some((info.clone(), *from)),
            _ => None,
        });
        let (info, from) = found.expect("no discovery reply");
        assert_eq!(from, fake.addr, "reply source is the connect address");
        assert_eq!(info.model.as_deref(), Some("XR18"), "official={}", official);
        assert_eq!(info.name.as_deref(), Some("FAKE18"));
        assert_eq!(info.raw.len(), 4, "raw args preserved for the user");
        println!(
            "XINFO ok (official_order={}): parsed {:?}",
            official,
            info.summary()
        );
    }
}

#[test]
fn connect_sweep_populates_model_and_external_changes_arrive() {
    let fake = FakeMixer::spawn(FakeConfig { animate: true, ..Default::default() }).unwrap();
    let client = Client::start_jailed(Arc::new(|| {})).unwrap();
    let mut model = MixerModel::default();

    client.cmd(Cmd::Connect { addr: fake.addr });

    // Drain until the model can lay out the surface and has the names.
    let start = Instant::now();
    let mut meters_seen = false;
    while start.elapsed() < Duration::from_secs(8) {
        match client.evt_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Evt::Param(p, v)) => {
                model.apply(p, v);
            }
            Ok(Evt::Meters { bank: 1, .. }) => meters_seen = true,
            Ok(_) => {}
            Err(_) => {}
        }
        if model.links_known()
            && model
                .get_s(Param::ChConfigName(Ch::new(1).unwrap()))
                .is_some()
            && model
                .get_s(Param::BusConfigName(crate::safety::BusN::new(1).unwrap()))
                .is_some()
            && model.get_s(Param::LrConfigName).is_some()
            && meters_seen
        {
            break;
        }
    }
    assert!(model.links_known(), "link state never arrived");
    assert!(meters_seen, "no meter blobs arrived");
    let strips = model.strips();
    assert_eq!(strips.len(), 13, "reference session strip count");
    assert_eq!(model.strip_name(strips[0]), "TV");
    assert_eq!(model.strip_name(strips[8]), "Speaker");
    assert_eq!(model.strip_name(strips[12]), "Headphones");

    // Our own SET echoes back (the console is the source of truth).
    // Sessions start view-only; flip the control gate first, as the user
    // does in the UI.
    client.cmd(Cmd::SetControl(true));
    let p = Param::ChMixFader(Ch::new(3).unwrap());
    client.cmd(Cmd::Set(p, PVal::F(0.6)));
    let evts = drain_until(&client, Duration::from_secs(3), |e| {
        matches!(e, Evt::Param(pp, PVal::F(f)) if *pp == p && (*f - 0.6).abs() < 1e-6)
    });
    assert!(
        evts.iter().any(
            |e| matches!(e, Evt::Param(pp, PVal::F(f)) if *pp == p && (*f - 0.6).abs() < 1e-6)
        ),
        "SET echo missing"
    );

    // A "second controller" (the fake's animator) moves ch 7 — we follow.
    let p7 = Param::ChMixFader(Ch::new(7).unwrap());
    let before = model.get_f(p7);
    let evts = drain_until(&client, Duration::from_secs(6), |e| {
        matches!(e, Evt::Param(pp, PVal::F(f)) if *pp == p7 && Some(*f) != before)
    });
    assert!(
        evts.iter()
            .any(|e| matches!(e, Evt::Param(pp, _) if *pp == p7)),
        "external fader move never pushed via /xremote"
    );

    // The entire exchange stayed clean.
    assert!(fake.danger.lock().unwrap().is_empty(), "danger tripwire fired");
    println!("SESSION ok: sweep, echo, external follow, no dangerous traffic");
}

#[test]
fn full_session_transmits_only_whitelisted_addresses() {
    // Run a complete session (connect + sweep + a few sets + renewals),
    // then audit every address the fake actually received.
    let fake = FakeMixer::spawn(FakeConfig { animate: false, ..Default::default() }).unwrap();
    let client = Client::start_jailed(Arc::new(|| {})).unwrap();
    client.cmd(Cmd::Connect { addr: fake.addr });
    wait_ms(1500); // sweep completes (~750 GETs at 40/25ms)
    client.cmd(Cmd::SetControl(true));
    client.cmd(Cmd::Set(Param::LrMixFader, PVal::F(0.7)));
    client.cmd(Cmd::Set(Param::ChMixOn(Ch::new(1).unwrap()), PVal::I(0)));
    wait_ms(300);

    let rx = fake.received.lock().unwrap();
    assert!(rx.len() > 700, "sweep should have queried the whole surface");
    for msg in rx.iter() {
        let ok = msg.addr == "/xinfo"
            || msg.addr == "/xremote"
            || msg.addr == "/meters"
            || Param::parse(&msg.addr).is_some();
        assert!(ok, "unexpected address on the wire: {}", msg.addr);
        assert!(
            crate::safety::deny_term(&msg.addr).is_none(),
            "deny-listed address on the wire: {}",
            msg.addr
        );
    }
    println!(
        "AUDIT ok: {} datagrams, every address whitelisted (/xinfo, /xremote, /meters, params)",
        rx.len()
    );
}

#[test]
fn meters_subscription_streams_decoded_banks() {
    let fake = FakeMixer::spawn(FakeConfig { animate: true, ..Default::default() }).unwrap();
    let client = Client::start_jailed(Arc::new(|| {})).unwrap();
    client.cmd(Cmd::Connect { addr: fake.addr });
    let evts = drain_until(&client, Duration::from_secs(4), |e| {
        matches!(e, Evt::Meters { bank: 1, vals } if vals.len() == 40)
    });
    let got = evts
        .iter()
        .find_map(|e| match e {
            Evt::Meters { bank: 1, vals } => Some(vals.clone()),
            _ => None,
        })
        .expect("no /meters/1 frame");
    assert_eq!(got.len(), 40);
    // Channel 1 has an open fader in the scene: its pre-fader level is live.
    assert!(got[0] > -90.0, "ch1 meter should carry signal, got {}", got[0]);
    // Channel 15's fader is closed but that does not silence a PRE meter in
    // general; our fake keys on fader, so it reads silent here.
    println!("METERS ok: 40-slot bank decodes, ch1 at {:.1} dB", got[0]);
}

#[test]
fn target_parsing() {
    assert!(parse_target("").is_err());
    assert!(parse_target("not an ip").is_err());
    assert_eq!(
        parse_target("192.0.2.7").unwrap(),
        "192.0.2.7:10024".parse().unwrap()
    );
    assert_eq!(
        parse_target("192.0.2.7:9999").unwrap(),
        "192.0.2.7:9999".parse().unwrap()
    );
}

#[test]
fn device_info_never_asserts_on_garbage() {
    // Hostile / malformed discovery replies must not panic the parser.
    let cases: Vec<Vec<OscArg>> = vec![
        vec![],
        vec![OscArg::I(48)],
        vec![OscArg::S("".into())],
        vec![OscArg::S("💥".into()), OscArg::F(1.0)],
        vec![OscArg::S("=".into()), OscArg::S("a=b=c".into())],
    ];
    for args in cases {
        let _ = DeviceInfo::from_args(&args);
    }
}

#[test]
fn subscription_constructors_are_the_only_specials() {
    // The four legal non-Param messages render exactly as documented.
    let m = SafeMsg::meters_subscribe(MeterBank::Channels);
    assert_eq!(m.addr(), "/meters");
    assert_eq!(SafeMsg::xremote().addr(), "/xremote");
    assert_eq!(SafeMsg::xinfo().addr(), "/xinfo");
    assert_eq!(SafeMsg::status().addr(), "/status");
}


// ---------------------------------------------------------------------------
// Stage-1 read-only live mode.
// ---------------------------------------------------------------------------

#[test]
fn read_only_gate_blocks_every_set_at_the_socket() {
    let fake = FakeMixer::spawn(FakeConfig { animate: false, ..Default::default() }).unwrap();
    let sock = GuardedSocket::bind(true).unwrap();
    assert!(sock.is_read_only(), "sockets must boot read-only");

    // Every whitelisted SET is refused while read-only…
    let set = Param::ChMixFader(Ch::new(1).unwrap()).set(&PVal::F(0.5)).unwrap();
    assert!(matches!(
        sock.send(fake.addr, &set),
        Err(Refused::ReadOnly { .. })
    ));
    let mute = Param::LrMixOn.set(&PVal::I(0)).unwrap();
    assert!(matches!(
        sock.send(fake.addr, &mute),
        Err(Refused::ReadOnly { .. })
    ));
    // …while the three stage-1 messages and bare GETs pass.
    sock.send(fake.addr, &SafeMsg::xinfo()).unwrap();
    sock.send(fake.addr, &SafeMsg::xremote()).unwrap();
    sock.send(fake.addr, &SafeMsg::meters_subscribe(MeterBank::Channels)).unwrap();
    sock.send(fake.addr, &Param::LrMixFader.get()).unwrap();
    wait_ms(200);
    let rx = fake.received.lock().unwrap();
    assert_eq!(rx.len(), 4, "only the read-only traffic arrived");
    for m in rx.iter() {
        assert!(
            m.args.is_empty() || m.addr == "/meters",
            "read-only session leaked a write: {} {:?}",
            m.addr,
            m.args
        );
    }
    drop(rx);
    // The explicit flip is what enables writes.
    sock.set_read_only(false);
    sock.send(fake.addr, &set).unwrap();
    wait_ms(200);
    assert_eq!(fake.received_count(), 5);
    println!("READ-ONLY GATE ok: sets refused at the socket until the explicit flip");
}

#[test]
fn stage1_session_wire_is_reads_and_subscribes_only() {
    // A full stage-1 session: connect, let the sweep run, then TRY to move
    // a fader without enabling control. Audit: nothing on the wire may
    // carry an argument except the /meters subscribe.
    let fake = FakeMixer::spawn(FakeConfig { animate: true, ..Default::default() }).unwrap();
    let client = Client::start_jailed(Arc::new(|| {})).unwrap();
    client.cmd(Cmd::Connect { addr: fake.addr });
    wait_ms(1200);
    client.cmd(Cmd::Set(Param::LrMixFader, PVal::F(0.9))); // must be dropped
    client.cmd(Cmd::Set(Param::ChMixOn(Ch::new(1).unwrap()), PVal::I(0)));
    wait_ms(400);
    let rx = fake.received.lock().unwrap();
    assert!(rx.len() > 700, "sweep ran");
    for m in rx.iter() {
        assert!(
            m.args.is_empty() || m.addr == "/meters",
            "stage-1 session transmitted a write: {} {:?}",
            m.addr,
            m.args
        );
    }
    println!(
        "STAGE-1 ok: {} datagrams, all argument-free reads + /meters subscribes",
        rx.len()
    );
}

#[test]
fn hostile_mixer_input_never_causes_a_transmission() {
    // A malicious or corrupted peer talks AT the client: dangerous
    // addresses, wrong types, truncated packets, lying blob counts. The
    // client must neither crash nor transmit anything in response.
    let fake = FakeMixer::spawn(FakeConfig { animate: false, ..Default::default() }).unwrap();
    let client = Client::start_jailed(Arc::new(|| {})).unwrap();
    client.cmd(Cmd::Connect { addr: fake.addr });
    // wait until the initial GET sweep has fully drained
    drain_until(&client, Duration::from_secs(10), |e| {
        matches!(e, Evt::Health { sweep_left: 0, .. })
    });
    wait_ms(200);
    let baseline = fake.received_count();

    let attacker = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let target = client.local_addr;
    let evil: Vec<Vec<u8>> = vec![
        OscMsg::with_args("/headamp/01/phantom", vec![OscArg::I(1)]).encode(),
        OscMsg::with_args("/-action/initall", vec![OscArg::I(1)]).encode(),
        OscMsg::with_args("/ch/01/mix/fader", vec![OscArg::S("lol".into())]).encode(),
        OscMsg::with_args("/ch/01/mix/on", vec![OscArg::F(0.5)]).encode(),
        // blob claiming 4096 int16s with a 6-byte payload
        OscMsg::with_args("/meters/1", vec![OscArg::B(vec![0, 16, 0, 0, 1, 2])]).encode(),
        // truncated garbage
        vec![0x2f, 0x63, 0x68],
        vec![0xff; 64],
        OscMsg::query("/xinfo").encode(), // spoofed reply-as-query shape
    ];
    for e in &evil {
        let _ = attacker.send_to(e, target);
    }
    wait_ms(1000);
    let rx = fake.received.lock().unwrap();
    for m in rx.iter().skip(baseline.saturating_sub(4)) {
        assert!(
            m.args.is_empty() || m.addr == "/meters",
            "hostile input provoked a write: {} {:?}",
            m.addr,
            m.args
        );
    }
    // only keep-alive renewals may have been added
    for m in rx.iter().skip(baseline) {
        assert!(
            m.addr == "/xremote" || m.addr == "/meters",
            "hostile input provoked traffic beyond keep-alives: {}",
            m.addr
        );
    }
    assert!(fake.danger.lock().unwrap().is_empty());
    println!("HOSTILE INPUT ok: client ignored it all; wire stayed read-only keep-alives");
}
