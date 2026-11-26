//! One participant's call with everyone else in it, as a mesh: a [`Peer`] per other
//! participant, each pair under its own secrets. What this side captures is sealed once per
//! receiver; what arrives is routed to the peer it came from.
//!
//! One encoder serves every receiver (no layers yet), so a key frame any receiver asks for goes
//! to all of them.

use crate::session::{Event, Peer};
use crate::wire::Voice;
use std::io;

/// This side of a call; `P` names a participant's path (an address, a link id).
pub struct Mesh<P> {
    call_id: Vec<u8>,
    my_pub: Vec<u8>,
    peers: Vec<(P, Peer)>,
}

impl<P: Clone + PartialEq> Mesh<P> {
    pub fn new(call_id: &[u8], my_pub: &[u8]) -> Self {
        Mesh { call_id: call_id.to_vec(), my_pub: my_pub.to_vec(), peers: Vec::new() }
    }

    /// A participant joins: its peer, to install secrets on.
    pub fn join(&mut self, path: P, their_pub: &[u8]) -> &mut Peer {
        self.leave(&path);
        self.peers.push((path, Peer::new(&self.call_id, &self.my_pub, their_pub)));
        &mut self.peers.last_mut().unwrap().1
    }

    pub fn leave(&mut self, path: &P) {
        self.peers.retain(|(p, _)| p != path);
    }

    pub fn peer(&mut self, path: &P) -> Option<&mut Peer> {
        self.peers.iter_mut().find(|(p, _)| p == path).map(|(_, peer)| peer)
    }

    pub fn peers(&self) -> impl Iterator<Item = (&P, &Peer)> {
        self.peers.iter().map(|(p, peer)| (p, peer))
    }

    /// One voice frame to everyone. A peer whose secret isn't in yet is skipped.
    pub fn send_voice(&mut self, now: u64, voice: &Voice, talking: bool, out: &mut Vec<(P, Vec<u8>)>) {
        for (path, peer) in &mut self.peers {
            if let Ok(d) = peer.send_voice(now, voice, talking) {
                out.push((path.clone(), d));
            }
        }
    }

    /// True when the next picture of `stream` should be a key frame: someone asked.
    pub fn key_frame_due(&mut self, stream: u8, now: u64) -> bool {
        let mut due = false;
        for (_, peer) in &mut self.peers {
            due |= peer.key_frame_due(stream, now);
        }
        due
    }

    /// One encoded access unit to everyone.
    pub fn send_video(&mut self, now: u64, stream: u8, au: &[u8], key: bool, capture_us: u64, out: &mut Vec<(P, Vec<u8>)>) -> io::Result<()> {
        let mut datagrams = Vec::new();
        for (path, peer) in &mut self.peers {
            datagrams.clear();
            match peer.send_video(now, stream, au, key, capture_us, &mut datagrams) {
                Ok(()) => out.extend(datagrams.drain(..).map(|d| (path.clone(), d))),
                Err(e) if e.kind() == io::ErrorKind::NotConnected => {} // no secret yet
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// A datagram from `path`; one from a path not in the call is dropped.
    pub fn receive(&mut self, now: u64, path: &P, datagram: &[u8], events: &mut Vec<(P, Event)>) {
        let Some((p, peer)) = self.peers.iter_mut().find(|(p, _)| p == path) else { return };
        let mut got = Vec::new();
        peer.receive(now, datagram, &mut got);
        events.extend(got.into_iter().map(|e| (p.clone(), e)));
    }

    /// Resends and feedback for every peer.
    pub fn poll(&mut self, now: u64, out: &mut Vec<(P, Vec<u8>)>) {
        let mut datagrams = Vec::new();
        for (path, peer) in &mut self.peers {
            peer.poll(now, &mut datagrams);
            out.extend(datagrams.drain(..).map(|d| (path.clone(), d)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{FEEDBACK_US, IDLE_FEEDBACK_US};
    use crate::sim::{Path, PathConfig};
    use crate::wire::stream;
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

    struct ThreePeople {
        meshes: Vec<Mesh<usize>>,
        paths: BTreeMap<(usize, usize), Path>,
    }

    impl ThreePeople {
        fn new(config: PathConfig) -> Self {
            let mut meshes: Vec<_> = (0..N).map(|i| Mesh::new(b"three of us", &device(i as u8))).collect();
            let mut paths = BTreeMap::new();
            for (i, mesh) in meshes.iter_mut().enumerate() {
                for j in (0..N).filter(|&j| j != i) {
                    let peer = mesh.join(j, &device(j as u8));
                    peer.install_send_secret(0, &secret(i, j)).unwrap();
                    peer.install_recv_secret(0, &secret(j, i)).unwrap();
                    paths.insert((i, j), Path::new(config, (i * 7 + j) as u64 + 1));
                }
            }
            Self { meshes, paths }
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
                    mesh.send_voice(now, &v, true, out);
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
        let for_one = call.meshes[0].peer(&1).unwrap().send_voice(0, &v, true).unwrap();
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
                    mesh.send_voice(now, &v, true, out);
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
