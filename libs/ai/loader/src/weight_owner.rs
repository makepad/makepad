//! Device-independent identities and deferred retirement of immutable weights.
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug)]
pub struct WeightOwner {
    id: u64,
    namespace: String,
}

/// A non-owning residency record. It outlives the owner and cannot key the cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeightOwnerTrace { namespace: String }

impl WeightOwnerTrace {
    pub fn namespace(&self) -> &str { &self.namespace }
}

/// A qualified cache key made only from an owner, for qualified-key entry points.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WeightKey(String);

impl WeightKey {
    /// Append a qualified sub-key, preserving the cache key's existing bytes.
    pub fn sub(&self, suffix: &str) -> WeightKey {
        WeightKey(format!("{}::{suffix}", self.0))
    }

    pub fn as_str(&self) -> &str { &self.0 }
    pub fn belongs_to(&self, owner: &WeightOwner) -> bool {
        self.0.starts_with(owner.namespace())
    }
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .expect("weight identity space exhausted")
}

impl WeightOwner {
    /// `label` names the weights in logs; a `:` in it becomes `_`, so the namespace's first
    /// `"::"` is always the one that ends it (a store finds the owner of a suffixed key by it).
    pub fn new(label: &str) -> Self {
        let id = next_id();
        Self { id, namespace: format!("\u{1f}{}#{id}::", label.replace(':', "_")) }
    }

    pub fn trace(&self) -> WeightOwnerTrace {
        WeightOwnerTrace { namespace: self.namespace.clone() }
    }

    pub fn key(&self, name: &str) -> WeightKey {
        WeightKey(format!("{}::{name}", self.namespace))
    }

    pub fn id(&self) -> u64 { self.id }
    pub fn namespace(&self) -> &str { &self.namespace }
}

#[derive(Default)]
struct Registry {
    inboxes: HashMap<u64, Arc<Mail>>,
    holders: HashMap<String, HashSet<u64>>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

#[derive(Default)]
struct Mail {
    has_mail: AtomicBool,
    namespaces: Mutex<Vec<String>>,
}

impl Drop for WeightOwner {
    fn drop(&mut self) {
        let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(holders) = registry.holders.remove(&self.namespace) {
            for id in holders {
                if let Some(inbox) = registry.inboxes.get(&id) {
                    let mut mail = inbox.namespaces.lock().unwrap_or_else(|e| e.into_inner());
                    mail.push(self.namespace.clone());
                    inbox.has_mail.store(true, Ordering::Release);
                }
            }
        }
    }
}

/// One consumer's retirement mailbox. Only bookkeeping locks are taken here;
/// callbacks run after unlocking, on the consumer's own device thread.
pub struct DeadOwnerInbox {
    id: u64,
    mail: Arc<Mail>,
    held: HashSet<String>,
}

impl DeadOwnerInbox {
    pub fn register() -> Self {
        let id = next_id();
        let mail = Arc::new(Mail::default());
        registry().lock().unwrap_or_else(|e| e.into_inner()).inboxes.insert(id, mail.clone());
        Self { id, mail, held: HashSet::new() }
    }

    /// Register once per owner, ignoring legacy namespaces and representation suffixes.
    pub fn holds(&mut self, namespace: &str) {
        if !namespace.starts_with('\u{1f}') { return; }
        let Some(end) = namespace.find("::") else { return; };
        let owner = &namespace[..end + 2];
        if self.held.contains(owner) { return; }
        registry().lock().unwrap_or_else(|e| e.into_inner()).holders
            .entry(owner.to_owned()).or_default().insert(self.id);
        self.held.insert(owner.to_owned());
    }

    pub fn take(&mut self, mut f: impl FnMut(&str)) {
        if !self.mail.has_mail.load(Ordering::Acquire) { return; }
        let mail = {
            let mut mail = self.mail.namespaces.lock().unwrap_or_else(|e| e.into_inner());
            self.mail.has_mail.store(false, Ordering::Release);
            std::mem::take(&mut *mail)
        };
        for namespace in mail {
            self.held.remove(&namespace);
            f(&namespace);
        }
    }
}

impl Drop for DeadOwnerInbox {
    fn drop(&mut self) {
        let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());
        registry.inboxes.remove(&self.id);
        for owner in &self.held {
            if let Some(holders) = registry.holders.get_mut(owner) {
                holders.remove(&self.id);
                if holders.is_empty() { registry.holders.remove(owner); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts() -> (usize, usize, usize) {
        let registry = registry().lock().unwrap();
        (registry.inboxes.len(), registry.holders.len(),
            registry.holders.values().map(HashSet::len).sum())
    }

    #[test]
    fn retirement_reaches_only_holders_and_leaves_no_history() {
        let mut holder = DeadOwnerInbox::register();
        let mut empty = DeadOwnerInbox::register();
        let a = WeightOwner::new("same::label");
        let b = WeightOwner::new("same::label");
        assert_ne!(a.id(), b.id(), "owner ids are unique");
        assert_eq!(a.namespace().find("::"), Some(a.namespace().len() - 2), "a label's colons never end the namespace early");
        holder.holds(&format!("{}f16::", a.namespace()));
        holder.holds(a.namespace());
        empty.holds("legacy::weight");
        assert_eq!(counts(), (2, 1, 1), "only one holder registered, without duplicate or legacy entries");
        let namespace = a.namespace().to_owned();
        drop(a);
        let mut delivered = Vec::new();
        empty.take(|_| panic!("non-holder received mail"));
        holder.take(|ns| delivered.push(ns.to_owned()));
        holder.take(|_| panic!("owner delivered twice"));
        assert_eq!(delivered, [namespace], "representation suffix resolves to owner");
        assert!(holder.held.is_empty(), "take forgets consumer registration");
        assert!(holder.mail.namespaces.lock().unwrap().is_empty(), "take empties mail");
        drop(b);
        assert_eq!(counts(), (2, 0, 0), "unused owner leaves no registry history");
        let c = WeightOwner::new("closed");
        holder.holds(c.namespace());
        drop(holder);
        assert_eq!(counts(), (1, 0, 0), "closed inbox removes all holder entries");
        drop(c);
        empty.take(|_| panic!("closed holder forwarded mail"));
        drop(empty);
        assert_eq!(counts(), (0, 0, 0), "all bookkeeping is gone after consumers and owners drop");
    }
}
