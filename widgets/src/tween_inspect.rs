//! The tween inspector's registry: what every live [`TweenHost`] is playing,
//! published while the inspector is open, and the playback commands the
//! inspector sends back (scrub, pause, resume, restart, time scale).
//!
//! Closed, it costs one relaxed atomic load per host event and nothing else.
//! Open, a host republishes at most ~30 times a second (and at once after a
//! command it applied or a frame that changed something) into a `Cx` global,
//! reusing the entry's buffers. Everything runs on the UI thread: the
//! registry is a plain global, no locks.
//!
//! [`TweenHost`]: crate::tween::TweenHost

use crate::makepad_draw::*;
use crate::tween::{InspectKind, InspectNode, Tag, TweenId};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Whether the inspector is open (hosts publish only then).
pub(crate) static INSPECT_OPEN: AtomicBool = AtomicBool::new(false);
static NEXT_HOST: AtomicU64 = AtomicU64::new(1);

/// How long a host that stopped publishing stays listed (seconds).
const STALE_SECS: f64 = 5.0;
/// The shortest time between two publishes of an unchanged host.
pub(crate) const REPUBLISH_SECS: f64 = 1.0 / 30.0;

/// Opens or closes the inspector. Closing forgets every snapshot.
pub fn set_tween_inspect(cx: &mut Cx, on: bool) {
    INSPECT_OPEN.store(on, Ordering::Relaxed);
    if !on {
        cx.global::<TweenInspectRegistry>().hosts.clear();
    }
    cx.redraw_all();
}

/// Whether the inspector is open.
pub fn tween_inspect_on() -> bool {
    INSPECT_OPEN.load(Ordering::Relaxed)
}

/// A fresh host identity for the registry.
pub(crate) fn next_host_id() -> u64 {
    NEXT_HOST.fetch_add(1, Ordering::Relaxed)
}

/// A playback command for one animation of one host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InspectCommand {
    /// Pause and put the playhead at this total time, events suppressed.
    Scrub { id: TweenId, time: f64 },
    Pause(TweenId),
    Resume(TweenId),
    /// Back to the start and play (delay included), events suppressed.
    Restart(TweenId),
    TimeScale { id: TweenId, scale: f64 },
}

/// One host as last published.
#[derive(Debug, Default)]
pub struct InspectHost {
    pub id: u64,
    pub name: String,
    /// When it last published (app seconds).
    pub at: f64,
    /// Every linked animation, parent first (see [`crate::tween::TweenEngine::inspect`]).
    pub nodes: Vec<InspectNode>,
    /// The labels of every timeline in `nodes`: (timeline, label, time).
    pub labels: Vec<(TweenId, Tag, f64)>,
    /// Commands waiting for the host's next event.
    pub commands: Vec<InspectCommand>,
}

impl InspectHost {
    /// The top-level animations (children of the root).
    pub fn roots(&self) -> impl Iterator<Item = &InspectNode> {
        self.nodes.iter().filter(|n| n.depth == 0)
    }

    /// `root` and everything under it, parent first.
    pub fn subtree(&self, root: TweenId) -> &[InspectNode] {
        let Some(start) = self.nodes.iter().position(|n| n.id == root) else {
            return &[];
        };
        let depth = self.nodes[start].depth;
        let end = self.nodes[start + 1..]
            .iter()
            .position(|n| n.depth <= depth)
            .map_or(self.nodes.len(), |i| start + 1 + i);
        &self.nodes[start..end]
    }
}

/// Every host that published while the inspector is open.
#[derive(Debug, Default)]
pub struct TweenInspectRegistry {
    pub hosts: Vec<InspectHost>,
    /// Scratch for a timeline's labels while publishing.
    label_scratch: Vec<(Tag, f64)>,
}

impl TweenInspectRegistry {
    /// Queues `cmd` for host `host`. False when that host is not listed.
    pub fn command(&mut self, host: u64, cmd: InspectCommand) -> bool {
        match self.hosts.iter_mut().find(|h| h.id == host) {
            Some(h) => {
                h.commands.push(cmd);
                true
            }
            None => false,
        }
    }

    /// Drops hosts that have not published for a while (dropped widgets,
    /// pages no longer on screen).
    pub fn prune(&mut self, now: f64) {
        self.hosts.retain(|h| now - h.at < STALE_SECS);
    }

    /// The entry of host `id`, created on first publish.
    pub(crate) fn entry(&mut self, id: u64, name: &str) -> &mut InspectHost {
        let ix = match self.hosts.iter().position(|h| h.id == id) {
            Some(ix) => ix,
            None => {
                self.hosts.push(InspectHost { id, ..Default::default() });
                self.hosts.len() - 1
            }
        };
        let h = &mut self.hosts[ix];
        if h.name != name {
            h.name.clear();
            h.name.push_str(name);
        }
        h
    }

    /// Publishes `engine`'s animations and labels into host `id`'s entry.
    pub(crate) fn publish(&mut self, id: u64, name: &str, now: f64, engine: &crate::tween::TweenEngine) {
        let mut scratch = std::mem::take(&mut self.label_scratch);
        let h = self.entry(id, name);
        h.at = now;
        engine.inspect(&mut h.nodes);
        h.labels.clear();
        for n in &h.nodes {
            if matches!(n.kind, InspectKind::Timeline) {
                engine.inspect_labels(n.id, &mut scratch);
                h.labels.extend(scratch.iter().map(|(tag, t)| (n.id, *tag, *t)));
            }
        }
        self.label_scratch = scratch;
    }
}
