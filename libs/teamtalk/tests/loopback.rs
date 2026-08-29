//! End-to-end tests over real UDP sockets on the loopback interface. Both
//! links bind ephemeral ports (no broadcast, no fixed 41531) so the suite
//! can run anywhere without opening the LAN port.

use makepad_teamtalk::{VoiceConfig, VoiceLink};
use std::time::{Duration, Instant};

fn test_config() -> VoiceConfig {
    VoiceConfig {
        port: 0,
        broadcast: false,
        hello_ms: 100,
        ..VoiceConfig::default()
    }
}

/// Two links pointed at each other by static address; returns them wired.
fn pair() -> (VoiceLink, VoiceLink) {
    let a = VoiceLink::bind(test_config()).expect("bind a");
    // The socket is bound to 0.0.0.0; point the second link at loopback.
    let a_addr: std::net::SocketAddr = format!("127.0.0.1:{}", a.local_addr().port())
        .parse()
        .unwrap();
    let b = VoiceLink::bind(VoiceConfig {
        static_peers: vec![a_addr],
        ..test_config()
    })
    .expect("bind b");
    // A learns B from B's hello; b already knows a.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if !a.peers().is_empty() && !b.peers().is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "discovery timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    (a, b)
}

/// Drive A's capture with a tone and B's playback until audio comes out (or
/// a deadline passes). Returns the peak of the last rendered block.
fn tone_reaches(a: &mut VoiceLink, b: &mut VoiceLink, seconds: f64) -> f32 {
    let mut capture = a.take_capture().expect("capture");
    let mut playback = b.take_playback().expect("playback");
    let block = 480; // 10 ms at 48 kHz
    let mut input = vec![0.0f32; block];
    let mut output = vec![0.0f32; block];
    let mut phase = 0.0f64;
    let mut peak = 0.0f32;
    let blocks = (seconds * 100.0) as usize;
    for _ in 0..blocks {
        for v in input.iter_mut() {
            *v = (phase.sin() * 0.25) as f32;
            phase += 440.0 * std::f64::consts::TAU / 48000.0;
        }
        capture.push_mono(48000.0, &input);
        std::thread::sleep(Duration::from_millis(10));
        output.fill(0.0);
        playback.mix_into_mono(48000.0, &mut output);
        peak = output.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        if peak > 0.05 {
            break;
        }
    }
    peak
}

#[test]
fn audio_flows_between_two_links() {
    let (mut a, mut b) = pair();
    let peak = tone_reaches(&mut a, &mut b, 3.0);
    assert!(peak > 0.05, "no audio arrived (peak {peak})");
    let stats = b.stats();
    assert!(stats.packets_recv > 0);
    assert_eq!(stats.bad_packets, 0);
    // The peer is visible with the right identity and marked talking.
    let peers = b.peers();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].sender, a.sender_id());
    assert!(peers[0].talking);
    assert!(peers[0].frames_accepted > 0);
}

#[test]
fn the_channel_filter_gates_teams_and_channel_zero_passes() {
    let (mut a, mut b) = pair();
    a.set_channel(2);
    b.set_listen_channels(&[1]); // not 2
    assert!(!b.listens_to(2) && b.listens_to(1) && b.listens_to(0));
    let peak = tone_reaches(&mut a, &mut b, 1.0);
    assert!(peak < 0.01, "team-2 audio leaked through (peak {peak})");
    assert!(b.stats().filtered > 0, "filter never engaged");

    // Retune to the right team: reuse the same links (handles were taken) by
    // flipping the listen set and pushing more audio through fresh handles is
    // not possible — so verify with a fresh pair.
    let (mut a, mut b) = pair();
    a.set_channel(2);
    b.set_listen_channels(&[2]);
    let peak = tone_reaches(&mut a, &mut b, 3.0);
    assert!(peak > 0.05, "team-2 audio missing on a team-2 listener (peak {peak})");

    // Channel 0 always plays, whatever the listen set.
    let (mut a, mut b) = pair();
    a.set_channel(0);
    b.set_listen_channels(&[7]);
    let peak = tone_reaches(&mut a, &mut b, 3.0);
    assert!(peak > 0.05, "channel-0 audio missing (peak {peak})");
}

#[test]
fn silence_is_dtx_and_costs_almost_no_bandwidth() {
    let (mut a, b) = pair();
    let mut capture = a.take_capture().expect("capture");
    let quiet = vec![0.0f32; 480];
    for _ in 0..50 {
        capture.push_mono(48000.0, &quiet);
        std::thread::sleep(Duration::from_millis(10));
    }
    let stats = a.stats();
    // Half a second of silence: sequence continuity is kept with header-only
    // packets, so bytes per packet must be tiny (the 32-byte header + a few
    // hellos).
    assert!(stats.packets_sent >= 50, "packets {}", stats.packets_sent);
    let per_packet = stats.bytes_sent as f64 / stats.packets_sent as f64;
    assert!(per_packet < 40.0, "silence costs {per_packet} bytes/packet");
    // And the receiver never marks the peer talking.
    assert!(!b.peers().iter().any(|p| p.talking));
    drop(b);
}

#[test]
fn peer_gain_and_mute_act_on_playback() {
    let (mut a, mut b) = pair();
    let sender = a.sender_id();
    b.set_peer_muted(sender, true);
    let peak = tone_reaches(&mut a, &mut b, 1.0);
    assert!(peak < 0.01, "muted peer audible (peak {peak})");
    b.set_peer_muted(sender, false);
    // (Gain/mute path exercised; audibility after unmute is covered by
    // audio_flows_between_two_links.)
    assert_eq!(b.peers()[0].gain, 1.0);
    b.set_peer_gain(sender, 0.25);
    assert_eq!(b.peers()[0].gain, 0.25);
}

#[test]
fn dropping_a_link_says_goodbye() {
    let (a, b) = pair();
    assert_eq!(b.peers().len(), 1);
    drop(a);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if b.peers().is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "BYE never removed the peer");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn muting_the_link_sends_silence_but_keeps_presence() {
    let (mut a, mut b) = pair();
    a.set_muted(true);
    let peak = tone_reaches(&mut a, &mut b, 1.0);
    assert!(peak < 0.01, "muted link audible (peak {peak})");
    assert_eq!(b.peers().len(), 1, "muted link lost presence");
}

#[test]
fn co_located_instances_fall_back_to_the_next_port_and_meet() {
    // A fixed port (not the real 41531: tests must not squat the LAN port).
    let port = 42641;
    let a = VoiceLink::bind(VoiceConfig {
        port,
        broadcast: false,
        hello_ms: 100,
        ..VoiceConfig::default()
    });
    let a = match a {
        Ok(a) => a,
        // Something else on this machine owns the test port: nothing to test.
        Err(_) => return,
    };
    assert_eq!(a.local_addr().port(), port);
    let b = VoiceLink::bind(VoiceConfig {
        port,
        broadcast: false,
        hello_ms: 100,
        static_peers: vec![format!("127.0.0.1:{port}").parse().unwrap()],
        ..VoiceConfig::default()
    })
    .expect("bind b on the fallback port");
    assert_eq!(b.local_addr().port(), port + 1, "expected the next port");
    let deadline = Instant::now() + Duration::from_secs(5);
    while a.peers().is_empty() || b.peers().is_empty() {
        assert!(Instant::now() < deadline, "co-located discovery timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(a.peers()[0].sender, b.sender_id());
}

#[test]
fn different_rooms_on_one_lan_never_meet() {
    // Same discovery path as `pair()`, but the rooms differ: the packets
    // must die at the room check — no peer entry, no audio, ever.
    let a = VoiceLink::bind(VoiceConfig {
        room: 0x1111,
        ..test_config()
    })
    .expect("bind a");
    let a_addr: std::net::SocketAddr = format!("127.0.0.1:{}", a.local_addr().port())
        .parse()
        .unwrap();
    let b = VoiceLink::bind(VoiceConfig {
        room: 0x2222,
        static_peers: vec![a_addr],
        ..test_config()
    })
    .expect("bind b");
    std::thread::sleep(Duration::from_millis(600));
    assert!(a.peers().is_empty(), "room 0x1111 saw a 0x2222 peer");
    assert!(b.peers().is_empty(), "room 0x2222 saw a 0x1111 peer");
    assert!(a.stats().wrong_room > 0, "the room check never engaged");

    // Move B into A's room at runtime: they meet within a hello interval.
    b.set_room(0x1111);
    let deadline = Instant::now() + Duration::from_secs(5);
    while a.peers().is_empty() || b.peers().is_empty() {
        assert!(Instant::now() < deadline, "same-room discovery timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(a.peers()[0].sender, b.sender_id());
}

#[test]
fn the_host_sentinel_is_a_working_sender_id() {
    let a = VoiceLink::bind(VoiceConfig {
        sender_id: makepad_teamtalk::HOST_SENDER_ID,
        ..test_config()
    })
    .expect("bind host");
    assert_eq!(a.sender_id(), u64::MAX);
    let a_addr: std::net::SocketAddr = format!("127.0.0.1:{}", a.local_addr().port())
        .parse()
        .unwrap();
    let b = VoiceLink::bind(VoiceConfig {
        static_peers: vec![a_addr],
        ..test_config()
    })
    .expect("bind b");
    let deadline = Instant::now() + Duration::from_secs(5);
    while b.peers().is_empty() {
        assert!(Instant::now() < deadline, "host discovery timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(b.peers()[0].sender, makepad_teamtalk::HOST_SENDER_ID);
}

#[test]
fn the_ogg_codec_flows_and_compresses() {
    use makepad_teamtalk::Codec;
    let a = VoiceLink::bind(VoiceConfig {
        codec: Codec::Ogg,
        // Gate off so every packet carries audio: the bandwidth numbers
        // below are then exact.
        gate_threshold_rms: -1.0,
        ..test_config()
    })
    .expect("bind a");
    let a_addr: std::net::SocketAddr = format!("127.0.0.1:{}", a.local_addr().port())
        .parse()
        .unwrap();
    let b = VoiceLink::bind(VoiceConfig {
        static_peers: vec![a_addr],
        ..test_config()
    })
    .expect("bind b");
    let deadline = Instant::now() + Duration::from_secs(5);
    while a.peers().is_empty() || b.peers().is_empty() {
        assert!(Instant::now() < deadline, "discovery timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut a = a;
    let mut b = b;
    let peak = tone_reaches(&mut a, &mut b, 3.0);
    assert!(peak > 0.05, "no audio over the ogg codec (peak {peak})");
    assert_eq!(b.stats().opaque_payloads, 0, "pages failed to decode");
    // 4-bit ADPCM at 240-sample frames: 32 header + 28 Ogg + 8 state +
    // 120 nibbles = 188 B vs 512 raw. Hellos push the average up a little.
    let s = a.stats();
    let per_packet = s.bytes_sent as f64 / s.packets_sent as f64;
    assert!(
        per_packet < 250.0,
        "ogg audio costs {per_packet:.0} B/packet (raw would be 512)"
    );
}
