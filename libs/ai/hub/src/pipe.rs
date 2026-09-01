//! Pipes: the capability unit of the ai-hub fabric (aicore.md §2).
//!
//! A pipe is one published capability — `llm.qwen`, `stt.whisper`,
//! `image.flux` — and a node's config is just which pipes it publishes, and
//! to whom. In-process pipes wrap a local engine on its own worker thread;
//! fleet pipes wrap a remote node reached through [`crate::client`]. This
//! module holds the shared vocabulary; the concrete engines live beside it
//! (`local_llm`, the `*_backend` modules serving over the wire).
//!
//! Readiness is the `fleet.rs` affinity ladder given a name: publishing a
//! pipe means "I can execute this", not "I have the weights right now" —
//! conflating the two is how a scheduler sends a 1.6s job to a box that
//! spends 47s streaming weights (aicore.md §6).

/// A dotted capability name: `domain.engine`, e.g. `llm.qwen-local`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PipeId(pub String);

impl PipeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    /// The domain half of `domain.engine`, or the whole id when undotted.
    pub fn domain(&self) -> &str {
        self.0.split('.').next().unwrap_or(&self.0)
    }
}

impl std::fmt::Display for PipeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The readiness ladder, exactly `fleet.rs`'s affinity ranks with names.
/// Ordering: a higher variant is warmer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Readiness {
    /// Wrong hardware, or the registry has never heard of it.
    Cannot,
    /// The registry knows it; acquiring would come first.
    Capable,
    /// Acquiring right now.
    Downloading,
    /// Weights on disk, load on demand.
    Ready,
    /// Resident in VRAM/RAM, ready now.
    Loaded,
}

impl Readiness {
    /// The `fleet.rs` affinity score this readiness corresponds to.
    pub fn affinity_score(self) -> u32 {
        match self {
            Readiness::Loaded => 4,
            Readiness::Ready => 3,
            Readiness::Downloading => 2,
            Readiness::Capable => 1,
            Readiness::Cannot => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_is_the_dotted_prefix() {
        assert_eq!(PipeId::new("llm.qwen-local").domain(), "llm");
        assert_eq!(PipeId::new("undotted").domain(), "undotted");
    }

    #[test]
    fn readiness_orders_like_the_affinity_ladder() {
        assert!(Readiness::Loaded > Readiness::Ready);
        assert!(Readiness::Ready > Readiness::Downloading);
        assert!(Readiness::Downloading > Readiness::Capable);
        assert!(Readiness::Capable > Readiness::Cannot);
        assert_eq!(Readiness::Loaded.affinity_score(), 4);
        assert_eq!(Readiness::Cannot.affinity_score(), 0);
    }
}
