//! One participant's call with everyone else in it, as a mesh: a [`Peer`] per other
//! participant, each pair under its own secrets. What this side captures is sealed once per
//! receiver; what arrives is routed to the peer it came from.
//!
//! One encoder serves the peers taking video. Each link controls whether it carries voice.

use crate::session::{Event, Peer, ReceiveOutcome, START_BPS};
use crate::wire::{stream, Voice};
use std::io;

/// How this sender includes a peer in its shared video encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaState {
    /// Waiting to try video.
    Waiting,
    /// Its path is being probed for video capacity.
    Probing,
    /// Receives video and contributes to the guard's inputs.
    Taking,
}

struct Link<P> {
    path: P,
    peer: Peer,
    voice: bool,
    state: MediaState,
}

/// This side of a call; `P` names a participant's path (an address, a link id).
pub struct Mesh<P> {
    call_id: Vec<u8>,
    my_pub: Vec<u8>,
    peers: Vec<Link<P>>,
    /// Peers present at call start share the first encoding. Only joins into running video must wait.
    video_sent: bool,
}

impl<P: Clone + PartialEq> Mesh<P> {
    pub fn new(call_id: &[u8], my_pub: &[u8]) -> Self {
        Mesh { call_id: call_id.to_vec(), my_pub: my_pub.to_vec(), peers: Vec::new(), video_sent: false }
    }

    /// A participant joins: its peer, to install secrets on.
    pub fn join(&mut self, path: P, their_pub: &[u8]) -> &mut Peer {
        self.join_link(path, Peer::new(&self.call_id, &self.my_pub, their_pub), true)
    }

    /// Joins a link under its negotiated call id, optionally carrying microphone audio.
    pub fn join_with_call_id(&mut self, path: P, call_id: &[u8], their_pub: &[u8], voice: bool) -> &mut Peer {
        self.join_link(path, Peer::new(call_id, &self.my_pub, their_pub), voice)
    }

    fn join_link(&mut self, path: P, peer: Peer, voice: bool) -> &mut Peer {
        self.leave(&path);
        let state = if self.video_sent && self.peers.iter().any(|link| link.state == MediaState::Taking) {
            MediaState::Waiting
        } else {
            MediaState::Taking
        };
        self.peers.push(Link { path, peer, voice, state });
        &mut self.peers.last_mut().unwrap().peer
    }

    pub fn set_voice(&mut self, path: &P, voice: bool) {
        if let Some(link) = self.peers.iter_mut().find(|link| &link.path == path) {
            link.voice = voice;
        }
    }

    pub fn leave(&mut self, path: &P) {
        self.peers.retain(|link| &link.path != path);
    }

    pub fn peer(&mut self, path: &P) -> Option<&mut Peer> {
        self.peers.iter_mut().find(|link| &link.path == path).map(|link| &mut link.peer)
    }

    pub fn peers(&self) -> impl Iterator<Item = (&P, &Peer)> {
        self.peers.iter().map(|link| (&link.path, &link.peer))
    }

    /// This sender's video state for a participant on the given path.
    pub fn media_state(&self, path: &P) -> Option<MediaState> {
        self.peers.iter().find(|link| &link.path == path).map(|link| link.state)
    }

    /// Lowest estimate, highest pacer age and highest loss reading among peers taking video.
    /// The reading is unknown until every such peer has one.
    pub fn media(&self, now: u64) -> (u64, u64, Option<f64>) {
        self.peers.iter().filter(|link| link.state == MediaState::Taking)
            .map(|link| (link.peer.estimate_bps(), link.peer.pacer_delay_us(now), link.peer.video_loss_level()))
            .reduce(|(min_rate, max_age, highest), (rate, age, level)| {
                (min_rate.min(rate), max_age.max(age), highest.zip(level).map(|(a, b)| a.max(b)))
            })
            .unwrap_or((START_BPS, 0, None))
    }

    /// Sends microphone audio to voice participants and application audio to every keyed link.
    pub fn send_voice(&mut self, now: u64, stream_id: u8, voice: &Voice, talking: bool, out: &mut Vec<(P, Vec<u8>)>) -> io::Result<()> {
        if !stream::is_audio(stream_id) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an audio stream"));
        }
        for link in &mut self.peers {
            if stream_id == stream::VOICE && !link.voice { continue; }
            if let Ok(d) = link.peer.send_voice(now, stream_id, voice, talking) {
                out.push((link.path.clone(), d));
            }
        }
        Ok(())
    }

    /// True when the next picture of `stream` should be a key frame: someone asked.
    pub fn key_frame_due(&mut self, stream: u8, now: u64) -> bool {
        let mut due = false;
        for link in self.peers.iter_mut().filter(|link| link.state == MediaState::Taking) {
            due |= link.peer.key_frame_due(stream, now);
        }
        due
    }

    /// One encoded access unit to every peer taking video.
    pub fn send_video(&mut self, now: u64, stream: u8, au: &[u8], key: bool, capture_us: u64, out: &mut Vec<(P, Vec<u8>)>) -> io::Result<()> {
        let mut datagrams = Vec::new();
        for link in self.peers.iter_mut().filter(|link| link.state == MediaState::Taking) {
            datagrams.clear();
            match link.peer.send_video(now, stream, au, key, capture_us, &mut datagrams) {
                Ok(()) => {
                    self.video_sent |= !datagrams.is_empty();
                    out.extend(datagrams.drain(..).map(|d| (link.path.clone(), d)));
                }
                Err(e) if e.kind() == io::ErrorKind::NotConnected => {} // no secret yet
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// A datagram from `path`; one from a path not in the call is dropped.
    pub fn receive(&mut self, now: u64, path: &P, datagram: &[u8], events: &mut Vec<(P, Event)>) {
        let Some(link) = self.peers.iter_mut().find(|link| &link.path == path) else { return };
        let mut got = Vec::new();
        link.peer.receive(now, datagram, &mut got);
        events.extend(got.into_iter().map(|e| (link.path.clone(), e)));
    }

    pub fn try_receive(&mut self, now: u64, path: &P, datagram: &[u8], events: &mut Vec<Event>) -> ReceiveOutcome {
        self.peer(path).map_or(ReceiveOutcome::Refused, |peer| peer.try_receive(now, datagram, events))
    }

    pub fn note_bad_tag(&mut self, path: &P) {
        if let Some(peer) = self.peer(path) { peer.note_bad_tag(); }
    }

    pub fn note_refused(&mut self, path: &P) {
        if let Some(peer) = self.peer(path) { peer.note_refused(); }
    }

    /// Resends and feedback for every peer.
    pub fn poll(&mut self, now: u64, out: &mut Vec<(P, Vec<u8>)>) {
        let mut datagrams = Vec::new();
        for link in &mut self.peers {
            link.peer.poll(now, &mut datagrams);
            out.extend(datagrams.drain(..).map(|d| (link.path.clone(), d)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::{encoder_kbps, video_ask, Guard, Rung, Tick};
    use crate::session::{FEEDBACK_US, IDLE_FEEDBACK_US};
    use crate::sim::{Path, PathConfig};
    use std::collections::BTreeMap;

    const N: usize = 3;

    fn device(n: u8) -> [u8; 65] {
        let mut k = [4u8; 65];
        k[1] = n;
        k
    }

    /// The secret `from` seals under for `to`.
    fn secret(from: usize, to: usize) -> [u8; 32] {
        let mut s = [0u8; 32];
        s[0] = from as u8;
        s[1] = to as u8;
        s
    }

    #[test]
    fn per_peer_call_id_and_voice_gate() {
        let mut mesh = Mesh::new(b"default", &device(0));
        let mut receivers = BTreeMap::new();
        for id in [1u8, 2] {
            let cid = [id; 16];
            let key = secret(0, id as usize);
            mesh.join_with_call_id(id, &cid, &device(id), id == 1).install_send_secret(0, &key).unwrap();
            let mut receiver = Peer::new(&cid, &device(id), &device(0));
            receiver.install_recv_secret(0, &key).unwrap();
            receivers.insert(id, receiver);
        }
        let voice = Voice { flags: 0, seq: 0, capture_us: 0, payload: b"audio" };
        let mut packets = Vec::new();
        let mut events = Vec::new();
        for (seq, (stream, enabled, expected)) in [
            (stream::VOICE, None, &[1][..]), (stream::SHARE, None, &[1, 2]),
            (stream::APP_AUDIO, None, &[1, 2]), (stream::VOICE, Some(true), &[1, 2]),
            (stream::VOICE, Some(false), &[1]),
        ].into_iter().enumerate() {
            if let Some(on) = enabled { mesh.set_voice(&2, on); }
            let now = seq as u64 * 20_000;
            if stream == stream::SHARE {
                mesh.send_video(now, stream, b"picture", true, now, &mut packets).unwrap();
            } else {
                mesh.send_voice(now, stream, &voice, true, &mut packets).unwrap();
            }
            assert!(packets.iter().map(|(id, _)| id).eq(expected), "wrong recipients for stream {stream} at step {seq}");
            for (id, packet) in packets.drain(..) {
                events.clear();
                let outcome = receivers.get_mut(&id).unwrap().try_receive(now, &packet, &mut events);
                assert_eq!(outcome, ReceiveOutcome::Opened, "peer {id} must open stream {stream} with its pairwise call id");
                assert_eq!(events.len(), 1, "peer {id} must receive one frame on stream {stream}");
            }
        }
    }

    struct ThreePeople {
        meshes: Vec<Mesh<usize>>,
        paths: BTreeMap<(usize, usize), Path>,
    }

    impl ThreePeople {
        fn disconnected(config: PathConfig) -> Self {
            let meshes = (0..N).map(|i| Mesh::new(b"three of us", &device(i as u8))).collect();
            let mut paths = BTreeMap::new();
            for i in 0..N {
                for j in (0..N).filter(|&j| j != i) {
                    paths.insert((i, j), Path::new(config, (i * 7 + j) as u64 + 1));
                }
            }
            Self { meshes, paths }
        }

        fn join_pair(&mut self, a: usize, b: usize) {
            for (from, to) in [(a, b), (b, a)] {
                let peer = self.meshes[from].join(to, &device(to as u8));
                peer.install_send_secret(0, &secret(from, to)).unwrap();
                peer.install_recv_secret(0, &secret(to, from)).unwrap();
            }
        }

        fn new(config: PathConfig) -> Self {
            let mut call = Self::disconnected(config);
            for a in 0..N {
                for b in a + 1..N {
                    call.join_pair(a, b);
                }
            }
            call
        }

        fn step(
            &mut self, now: u64,
            mut capture: impl FnMut(usize, &mut Mesh<usize>, &mut Vec<(usize, Vec<u8>)>),
            mut receive: impl FnMut(usize, usize, Event),
        ) {
            let mut out = Vec::new();
            for (from, mesh) in self.meshes.iter_mut().enumerate() {
                capture(from, mesh, &mut out);
                mesh.poll(now, &mut out);
                for (to, d) in out.drain(..) {
                    self.paths.get_mut(&(from, to)).unwrap().send(now, d);
                }
            }
            let mut arrived = Vec::new();
            let mut events = Vec::new();
            for (&(from, to), path) in &mut self.paths {
                path.poll(now, &mut arrived);
                for d in arrived.drain(..) {
                    self.meshes[to].receive(now, &from, &d, &mut events);
                }
                for (p, e) in events.drain(..) {
                    assert_eq!(p, from, "events name the peer they came from");
                    receive(from, to, e);
                }
            }
        }
    }

    #[test]
    fn a_waiting_joiner_preserves_the_existing_video() {
        #[derive(Clone, Copy)]
        struct Capture {
            at: u64,
            guard_ask: u64,
            encoder_ask: u64,
            rung: Rung,
            encoded: bool,
        }

        fn run(join_c: bool) -> ([Vec<Capture>; 2], [u32; 2], [u32; 2], u64) {
            let mut call = ThreePeople::disconnected(PathConfig {
                latency_us: 20_000, rate_bps: 3_000_000, queue_us: 12_000, ..Default::default()
            });
            call.join_pair(0, 1);
            let mut guards: [Guard; N] = std::array::from_fn(|_| Guard::new(Guard::rung_for(video_ask(START_BPS))));
            let mut captures: [Vec<Capture>; 2] = Default::default();
            let mut need_key = [true; N];
            let mut pictures = [0; 2];
            let mut voice = [0; 2];
            let mut first_picture = [None; 2];
            let mut qualified = None;
            let mut join = None;
            for now in (1_000..=35_000_000u64).step_by(1_000) {
                call.step(now, |from, mesh, out| {
                    if from == 2 && (!join_c || join.is_none()) {
                        return;
                    }
                    if now % 20_000 == 0 {
                        mesh.send_voice(now, stream::VOICE, &Voice { flags: 0, seq: (now / 20_000) as u32,
                            capture_us: now, payload: &[from as u8; 100] }, true, out).unwrap();
                    }
                    if now % 33_000 != 0 {
                        return;
                    }
                    let (estimate, delay, level) = mesh.media(now);
                    let g = &mut guards[from];
                    let encoded = g.tick(now, estimate, delay, level) == Tick::Encode;
                    need_key[from] |= g.take_step().is_some();
                    let rung = g.rung();
                    let guard_ask = g.ask_bps();
                    let encoder_ask = u64::from(encoder_kbps(guard_ask, rung)) * 1_000;
                    if let Some(samples) = captures.get_mut(from) {
                        samples.push(Capture { at: now, guard_ask, encoder_ask, rung, encoded });
                    }
                    if encoded {
                        let key = mesh.key_frame_due(stream::CAMERA_HIGH, now) | std::mem::take(&mut need_key[from]);
                        let bytes = (encoder_ask / u64::from(rung.fps) / 8) as usize * if key { 3 } else { 1 };
                        g.encoded(now, bytes, key);
                        mesh.send_video(now, stream::CAMERA_HIGH, &vec![from as u8; bytes], key, now, out).unwrap();
                    }
                }, |from, to, event| {
                    if to == 2 {
                        match event {
                            Event::Video { .. } => {
                                pictures[from] += 1;
                                first_picture[from].get_or_insert(now);
                            }
                            Event::Voice { .. } => voice[from] += 1,
                        }
                    }
                });
                if let Some(at) = join {
                    if now == at + 20_000_000 { break; }
                } else {
                    if captures.iter().all(|v| v.last().is_some_and(|c| c.encoder_ask >= 900_000)) {
                        if now - *qualified.get_or_insert(now) >= 3_000_000 {
                            join = Some(now);
                            if join_c {
                                for i in 0..2 {
                                    let before = call.meshes[i].media(now);
                                    call.join_pair(i, 2);
                                    assert_eq!(call.meshes[i].media(now), before,
                                        "adding C to sender {i} at {now} µs changed the guard's inputs");
                                }
                            }
                        }
                    } else {
                        qualified = None;
                    }
                    assert!(now < 15_000_000 || join.is_some(), "A and B did not hold 900 kbit/s for 3 s by {now} µs");
                }
            }

            let join = join.unwrap();
            for (i, samples) in captures.iter().enumerate() {
                let cut = samples.partition_point(|c| c.at <= join);
                let before = samples[cut - 1];
                let after = &samples[cut..];
                let min = after.iter().map(|c| c.encoder_ask).min().unwrap();
                let max = after.iter().map(|c| c.encoder_ask).max().unwrap();
                let first = &after[..after.partition_point(|c| c.at <= join + 250_000)];
                let encoded = after.iter().filter(|c| c.encoded).count();
                let steps: Vec<_> = samples[cut - 1..].windows(2).filter(|p| p[0].rung != p[1].rung)
                    .map(|p| (p[1].at - join, p[1].rung.to_string())).collect();
                eprintln!("join {join} µs (join_c={join_c}), sender {i}: pre {} bit/s (guard {}) {}, post {min}..{max} bit/s, steps {steps:?}, encoded {encoded}/{}, first 250 ms {}/{}; C received {} pictures, first at +{:?} µs, {} voice frames",
                    before.encoder_ask, before.guard_ask, before.rung, after.len(), first.iter().filter(|c| c.encoded).count(), first.len(),
                    pictures[i], first_picture[i].map(|at| at - join), voice[i]);
            }
            (captures, pictures, voice, join)
        }

        let (joined, pictures, voice, join) = run(true);
        let (control, _, _, control_join) = run(false);
        assert_eq!(join, control_join, "C's join and the control must qualify at the same time");
        for i in 0..2 {
            assert_eq!(pictures[i], 0, "waiting C received {} pictures from sender {i}", pictures[i]);
            assert!(voice[i] > 0, "waiting C heard no voice from sender {i} in 20 s");
            assert_eq!(joined[i].len(), control[i].len(), "sender {i}, join +20 s: capture record count");
            for (c, reference) in joined[i].iter().zip(&control[i]) {
                if c.at <= join && reference.at <= join {
                    continue;
                }
                let elapsed = c.at.max(reference.at) - join;
                assert_eq!(c.at, reference.at, "sender {i}, join +{elapsed} µs: capture timestamp");
                assert_eq!(c.guard_ask, reference.guard_ask, "sender {i}, join +{elapsed} µs: guard ask");
                assert_eq!(c.encoder_ask, reference.encoder_ask, "sender {i}, join +{elapsed} µs: encoder ask");
                assert_eq!(c.rung, reference.rung, "sender {i}, join +{elapsed} µs: rung");
                assert_eq!(c.encoded, reference.encoded, "sender {i}, join +{elapsed} µs: encode decision");
            }
        }
    }

    #[test]
    fn three_people_each_hear_and_see_the_other_two() {
        let mut call = ThreePeople::new(PathConfig { latency_us: 15_000, ..Default::default() });
        let mut voices = [[0u32; N]; N]; // voices[to][from]
        let mut pictures = [[0u32; N]; N];
        let mut sent = 0u32;
        for ms in 1..=1_000u64 {
            let now = ms * 1_000;
            call.step(now, |i, mesh, out| {
                if now % 20_000 == 0 {
                    let v = Voice { flags: 0, seq: (now / 20_000) as u32, capture_us: now, payload: &[i as u8; 80] };
                    mesh.send_voice(now, stream::VOICE, &v, true, out).unwrap();
                }
                if now % 50_000 == 0 {
                    let key = mesh.key_frame_due(stream::CAMERA_HIGH, now) || now == 50_000;
                    let au = vec![i as u8; if key { 9_000 } else { 2_500 }];
                    mesh.send_video(now, stream::CAMERA_HIGH, &au, key, now, out).unwrap();
                    sent += (i == 0) as u32;
                }
            }, |from, to, e| match e {
                Event::Voice { payload, .. } => {
                    assert_eq!(payload, [from as u8; 80], "{to} hears {from}'s voice, not anyone else's");
                    voices[to][from] += 1;
                }
                Event::Video { frame, .. } => {
                    assert!(frame.data.iter().all(|&b| b == from as u8), "{to} sees {from}'s picture");
                    pictures[to][from] += 1;
                }
            });
        }
        for to in 0..N {
            for from in (0..N).filter(|&f| f != to) {
                assert!(voices[to][from] >= 48, "{to} heard {} voice frames from {from}", voices[to][from]);
                assert!(pictures[to][from] >= sent - 1, "{to} saw {} of {sent} pictures from {from}", pictures[to][from]);
            }
        }

        // a datagram sealed for 1 doesn't open at 2, even arriving on 0's path
        let v = Voice { flags: 0, seq: 999, capture_us: 0, payload: b"x" };
        let mut events = Vec::new();
        let for_one = call.meshes[0].peer(&1).unwrap().send_voice(0, stream::VOICE, &v, true).unwrap();
        call.meshes[2].receive(0, &0, &for_one, &mut events);
        assert!(events.is_empty(), "pairwise secrets: what 0 sealed for 1 is noise to 2");
        assert_eq!(call.meshes[2].peer(&0).unwrap().stats.bad_tag, 1, "and is counted as a bad tag");
    }

    /// Resend a lost key only on the affected link. The shared encoder makes at most one
    /// requested key per interval, so restoring B's link must not reset C's picture.
    #[test]
    fn a_lost_key_is_repaired_on_one_link_without_resetting_the_other_picture() {
        let (a, b, c) = (0, 1, 2);
        let path = PathConfig { latency_us: 10_000, ..Default::default() };
        let mut call = ThreePeople::new(path);
        let returned = 2_500_000;
        let end_capture = returned + 1_000_000;
        let bound = IDLE_FEEDBACK_US + FEEDBACK_US + 4 * path.latency_us;
        let mut encoded = 0u32;
        let mut keys_made = Vec::new();
        let mut first_at_b = None;
        let mut pictures_at_c = Vec::new();
        let mut keys_at_c = Vec::new();

        // Use the outage session's captures and allow its last pictures to leave the pacers.
        for now in (1_000..=end_capture + crate::video::CACHE_US).step_by(1_000) {
            call.paths.get_mut(&(a, b)).unwrap().config.loss_ppm = if now > 1_000_000 && now <= returned { 1_000_000 } else { 0 };
            call.step(now, |from, mesh, out| {
                if from != a || now > end_capture {
                    return;
                }
                if now % 20_000 == 0 {
                    let seq = (now / 20_000 - 1) as u32;
                    let v = Voice { flags: 0, seq, capture_us: now, payload: &[seq as u8; 100] };
                    mesh.send_voice(now, stream::VOICE, &v, true, out).unwrap();
                }
                if now % 33_000 == 0 {
                    let key = encoded == 0 || mesh.key_frame_due(stream::CAMERA_HIGH, now);
                    let size = if key { 20_000 } else { 3_000 + (encoded as usize % 7) * 100 };
                    mesh.send_video(now, stream::CAMERA_HIGH, &vec![encoded as u8; size], key, now, out).unwrap();
                    if key {
                        keys_made.push((encoded, now));
                    }
                    encoded += 1;
                }
            }, |from, to, event| {
                let Event::Video { frame, .. } = event else { return };
                if from != a {
                    return;
                }
                if to == b && now > returned {
                    first_at_b.get_or_insert((frame.frame, now));
                } else if to == c {
                    pictures_at_c.push(frame.frame);
                    if frame.key {
                        keys_at_c.push((frame.frame, now));
                    }
                }
            });
        }
        let delay = first_at_b.map(|(_, at)| at - returned);
        eprintln!("M: A encoded {encoded} pictures, keys (frame, µs) {keys_made:?}; B first after return {first_at_b:?}, delay {delay:?} µs, bound {bound}; C showed {} pictures, keys {keys_at_c:?}", pictures_at_c.len());
        assert_eq!(keys_made.len(), 2, "M: expected two shared keys, A made {keys_made:?}");
        assert!(pictures_at_c.iter().copied().eq(0..encoded), "M: C should show frames 0..{encoded} in order, got {pictures_at_c:?}");
        assert!(keys_at_c.iter().all(|&(_, at)| at <= returned), "M: C saw a key after return at {returned} µs: {keys_at_c:?}");
        assert!(delay.is_some_and(|us| us <= bound), "M: B's first picture after return at {returned} µs was {first_at_b:?}, bound {bound} µs");
    }
}
