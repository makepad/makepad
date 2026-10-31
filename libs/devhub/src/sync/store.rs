//! The store applier: sync for a JIT host with no checkout (an Android phone, a Quest:
//! tools/run/android_host). The host embeds the node and gives sync its source store
//! ([`SourceStore`]): pushed files go into the JIT's own source store / virtual folders and are
//! handed to the JIT's live-patch entry (or its in-place restart). Nothing here needs git, a
//! filesystem checkout or a build: it compiles into any JIT host.
//!
//! Protocol, on the same `sync` service as the checkout applier:
//! - `sync status` answers `"apply":"store"`, the rev each repo's store holds and the path
//!   prefixes the host wants (empty: the whole repo);
//! - the `sync-files` channel takes one signed order `files <repo> <from rev|-> <to rev> <payload
//!   sha256> <bytes>`, then the payload ([`super::files`]). It applies only when the store holds
//!   `from` (the device's fast-forward rule: a stale or diverged device is refused, never
//!   overwritten), then writes and patches.
//!
//! The device's credential is FLEETID's `paired` role; orders are the master's, checked exactly
//! as on a desktop node ([`super::sign`]).

use super::files;
use super::sign::{self, Order, Seen};
use super::NodeCtx;
use crate::proto::{hub_record, Sev};
use makepad_network::digest::Sha256;
use makepad_remote::conn::{Event, CLOSE_REFUSED};
use makepad_remote::endpoint::Factory;
use makepad_remote::{json_str, lines, Endpoint};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The largest file-tree payload a device takes.
const MAX_PAYLOAD: u64 = 1 << 30;

/// What a JIT host gives the store applier.
pub trait SourceStore: Send + Sync {
    /// The rev (the master's commit id) the store holds for `repo`; `None`: nothing yet.
    fn rev(&self, repo: &str) -> Option<String>;
    /// The path prefixes of `repo` the host runs from (empty: the whole repo).
    fn prefixes(&self, repo: &str) -> Vec<String>;
    /// Writes (`Some`) or deletes (`None`) the files (paths relative to the repo root) and then
    /// records `rev`, as one step: on `Err` the store still holds its old rev.
    fn write(&self, repo: &str, rev: &str, changes: &[files::Change]) -> Result<(), String>;
    /// Hands the changed paths to the JIT's live patch (or an in-place restart when the JIT asks
    /// for one); answers what it did, one line.
    fn patch(&self, repo: &str, paths: &[String]) -> Result<String, String>;
}

static STORE: OnceLock<Arc<dyn SourceStore>> = OnceLock::new();

/// A JIT host with no checkout installs its source store before it starts the embedded node;
/// the node then serves the store applier instead of the checkout one.
pub fn set_source_store(store: Arc<dyn SourceStore>) {
    let _ = STORE.set(store);
}

pub fn source_store() -> Option<Arc<dyn SourceStore>> {
    STORE.get().cloned()
}

pub struct StoreSync {
    ctx: Arc<NodeCtx>,
    store: Arc<dyn SourceStore>,
    seen: Seen,
    /// a test's fleet (else this process's)
    fleet: Option<makepad_network::fleet::id::Fleet>,
    busy: Mutex<()>,
}

fn err_line(e: &str) -> String {
    format!("{{\"err\":{}}}", json_str(e))
}

impl StoreSync {
    pub fn new(ctx: Arc<NodeCtx>, store: Arc<dyn SourceStore>, fleet: Option<makepad_network::fleet::id::Fleet>) -> Arc<StoreSync> {
        Arc::new(StoreSync { ctx, store, seen: Seen::default(), fleet, busy: Mutex::new(()) })
    }

    fn event(&self, sev: Sev, line: String) {
        (self.ctx.publish)(hub_record(&self.ctx.machine, "", 0, "sync", sev, line, String::new()));
    }

    fn check(&self, order: &str) -> Result<Order, String> {
        match &self.fleet {
            Some(f) => self.seen.check_with(f, order, makepad_network::fleet::id::now_secs()),
            None => self.seen.check(order),
        }
    }

    pub fn register(self: &Arc<Self>, ep: &Endpoint) {
        let s = self.clone();
        ep.register("sync", 1, lines::service("sync", Arc::new(move |line, out| s.sync_line(line, out))));
        let s = self.clone();
        ep.register("sync-files", 1, files_factory(s));
    }

    fn sync_line(&self, line: &str, out: &mut Vec<String>) {
        match line.split_whitespace().collect::<Vec<_>>().as_slice() {
            ["status"] => {
                let repos: Vec<String> = ["makepad", "commercial"]
                    .iter()
                    .map(|r| {
                        let p: Vec<String> = self.store.prefixes(r).iter().map(|p| json_str(p)).collect();
                        format!("{}:{{\"checkout\":{},\"refs\":[],\"prefixes\":[{}]}}", json_str(r), self.store.rev(r).map_or("null".into(), |s| json_str(&s)), p.join(","))
                    })
                    .collect();
                out.push(format!("{{\"machine\":{},\"apply\":\"store\",\"busy\":{},\"repos\":{{{}}}}}", json_str(&self.ctx.machine), self.busy.try_lock().is_err(), repos.join(",")));
            }
            _ => out.push(err_line("sync status (this node applies pushed files to its JIT's source store: `sync push` sends them)")),
        }
    }

    /// One store push: the signed order, then its payload. Returns the answer line.
    pub fn apply(&self, order: &str, payload: &[u8]) -> Result<String, String> {
        let o = self.check(order)?;
        let w: Vec<&str> = o.words.iter().map(String::as_str).collect();
        let ["files", repo, from, to, hash, size] = w.as_slice() else { return Err("not a files order".into()) };
        if !matches!(*repo, "makepad" | "commercial") || to.len() != 40 || hash.len() != 64 {
            return Err("malformed files order".into());
        }
        if size.parse::<u64>().ok() != Some(payload.len() as u64) {
            return Err("the payload's size is not the one the master signed".into());
        }
        let mut h = Sha256::new();
        h.update(payload);
        if sign::hex(&h.finalise()) != *hash {
            return Err("the payload's hash is not the one the master signed".into());
        }
        let Ok(_busy) = self.busy.try_lock() else { return Err("a push is being applied".into()) };
        let have = self.store.rev(repo);
        let from = (*from != "-").then_some(*from);
        if have.as_deref() != from {
            return Err(format!("the store holds {} of {repo}, the push starts from {}: not a fast-forward of this device", have.as_deref().unwrap_or("nothing"), from.unwrap_or("nothing")));
        }
        let changes = files::decode(payload)?;
        self.store.write(repo, to, &changes)?;
        let paths: Vec<String> = changes.iter().map(|c| c.0.clone()).collect();
        let patched = self.store.patch(repo, &paths).unwrap_or_else(|e| format!("patch failed: {e}"));
        self.event(Sev::Info, format!("sync: {repo} {} -> {} ({} files) into the JIT store by {}: {patched}", from.map_or("(new)", |f| &f[..10]), &to[..10], paths.len(), o.signer));
        Ok(format!("{{\"stored\":{},\"repo\":{},\"files\":{},\"patch\":{}}}", json_str(to), json_str(repo), paths.len(), json_str(&patched)))
    }
}

fn files_factory(sync: Arc<StoreSync>) -> Factory {
    Arc::new(move |conn, open| {
        let hooks = conn.hooks();
        let sync = sync.clone();
        std::thread::Builder::new()
            .name("devhub-sync-files".into())
            .spawn(move || {
                (hooks.enter)();
                let ch = open.channel;
                let r = (|| -> Result<String, String> {
                    let Ok(Event::Data(first)) = ch.recv_timeout(Duration::from_secs(120)) else { return Err("no files order".into()) };
                    let order = String::from_utf8(first).map_err(|_| "the order is not text")?;
                    let size: u64 = order_size(&sync, &order)?;
                    if size > MAX_PAYLOAD {
                        return Err("the payload is too large".into());
                    }
                    let mut payload = Vec::with_capacity(size as usize);
                    while (payload.len() as u64) < size {
                        match ch.recv_timeout(Duration::from_secs(120)) {
                            Ok(Event::Data(d)) => payload.extend_from_slice(&d),
                            Ok(Event::Opened(_)) => {}
                            _ => return Err(format!("the push ended after {} of {size} bytes", payload.len())),
                        }
                    }
                    sync.apply(order.trim(), &payload)
                })();
                let line = r.unwrap_or_else(|e| {
                    sync.event(Sev::Warn, format!("sync: store push refused: {e}"));
                    err_line(&e)
                });
                let _ = ch.send(format!("{line}\n{{\"end\":\"files\",\"ms\":0}}\n").into_bytes());
                (hooks.exit)();
            })
            .map(|_| ())
            .map_err(|e| (CLOSE_REFUSED, format!("cannot start the files thread: {e}")))
    })
}

/// The payload size an order names, read before the order is checked (it is checked, with its
/// nonce, when the payload is in).
fn order_size(_sync: &StoreSync, order: &str) -> Result<u64, String> {
    let bytes = sign::unhex(order.trim()).ok_or("the order is not hex")?;
    let text = String::from_utf8_lossy(&bytes);
    let at = text.find("files ").ok_or("not a files order")?;
    text[at..].split_whitespace().nth(5).and_then(|s| s.parse().ok()).ok_or_else(|| "not a files order".to_string())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use makepad_network::fleet::id::{self, Role};
    use std::collections::BTreeMap;

    /// An in-process JIT host: its source store is a map, its live patch records what it got.
    #[derive(Default)]
    pub struct FakeHost {
        pub files: Mutex<BTreeMap<String, Vec<u8>>>,
        pub revs: Mutex<BTreeMap<String, String>>,
        pub patched: Mutex<Vec<String>>,
    }

    impl SourceStore for FakeHost {
        fn rev(&self, repo: &str) -> Option<String> {
            self.revs.lock().unwrap().get(repo).cloned()
        }
        fn prefixes(&self, _repo: &str) -> Vec<String> {
            vec!["apps/game".into()]
        }
        fn write(&self, repo: &str, rev: &str, changes: &[files::Change]) -> Result<(), String> {
            let mut f = self.files.lock().unwrap();
            for (p, d) in changes {
                match d {
                    Some(d) => f.insert(format!("{repo}/{p}"), d.clone()),
                    None => f.remove(&format!("{repo}/{p}")),
                };
            }
            self.revs.lock().unwrap().insert(repo.into(), rev.into());
            Ok(())
        }
        fn patch(&self, _repo: &str, paths: &[String]) -> Result<String, String> {
            self.patched.lock().unwrap().extend(paths.iter().cloned());
            Ok(format!("patched {} files", paths.len()))
        }
    }

    pub fn test_fleet() -> (id::Fleet, id::Me, id::Me) {
        let root = id::Root::generate().unwrap();
        let fleet = id::Fleet::new(root.point(), Default::default());
        let now = id::now_secs();
        let mk = |name: &str, role: Role, from: Option<std::net::IpAddr>| {
            let key = id::generate_key().unwrap();
            id::Me::new(key, root.issue(name, role, now + 3600, from, &id::point_of(&key)).unwrap()).unwrap()
        };
        (fleet, mk("mac", Role::Device, Some([10, 0, 0, 222].into())), mk("quest", Role::Paired, None))
    }

    fn order(me: &id::Me, words: &str) -> String {
        sign::hex(&me.announce(sign::SERVICE, format!("{words} nonce={}", id::now_secs() as f64 + rand_ish()).as_bytes()).unwrap())
    }

    fn rand_ish() -> f64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed) as f64 / 1000.0
    }

    fn hash(b: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(b);
        sign::hex(&h.finalise())
    }

    /// The whole device path in process: the master's payload and signed order, over a real
    /// endpoint channel (Unix socket), into a fake JIT host's store and live patch.
    #[test]
    fn device_push_over_a_channel_into_a_fake_host() {
        let (fleet, master, device) = test_fleet();
        let host = Arc::new(FakeHost::default());
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let ctx = Arc::new(NodeCtx { machine: "quest".into(), publish: Arc::new(move |r| ev.lock().unwrap().push(r.line)) });
        let sync = StoreSync::new(ctx, host.clone(), Some(fleet));
        let ep = Endpoint::new("devhub quest");
        sync.register(&ep);
        let sock = std::env::temp_dir().join(format!("devhub-store-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock);
        makepad_remote::carrier::unix::listen_at(&ep, &sock).unwrap();
        let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(sock.clone()), "test");

        let send = |order: String, payload: &[u8]| -> String {
            let ch = c.open("sync-files", "").unwrap();
            ch.send(order.into_bytes()).unwrap();
            for chunk in payload.chunks(1000) {
                ch.send(chunk.to_vec()).unwrap();
            }
            let mut got = String::new();
            while let Some(Event::Data(d)) = ch.recv() {
                got.push_str(&String::from_utf8_lossy(&d));
                if got.contains("{\"end\":") {
                    break;
                }
            }
            got.lines().next().unwrap_or("").to_string()
        };
        let r1 = "1111111111111111111111111111111111111111";
        let r2 = "2222222222222222222222222222222222222222";
        // first push: the whole tree under the host's prefixes
        let p1 = files::encode(&[("apps/game/src/lib.rs".into(), Some(b"fn speed() -> f32 { 1.0 }".to_vec())), ("apps/game/src/old.rs".into(), Some(b"x".to_vec()))]).unwrap();
        let a = send(order(&master, &format!("files makepad - {r1} {} {}", hash(&p1), p1.len())), &p1);
        assert!(a.contains("\"stored\""), "{a}");
        // the next push: a diff from r1
        let p2 = files::encode(&[("apps/game/src/lib.rs".into(), Some(b"fn speed() -> f32 { 2.0 }".to_vec())), ("apps/game/src/old.rs".into(), None)]).unwrap();
        let a = send(order(&master, &format!("files makepad {r1} {r2} {} {}", hash(&p2), p2.len())), &p2);
        assert!(a.contains("patched 2 files"), "{a}");
        assert_eq!(host.files.lock().unwrap().get("makepad/apps/game/src/lib.rs").map(|v| v.as_slice()), Some(&b"fn speed() -> f32 { 2.0 }"[..]));
        assert!(!host.files.lock().unwrap().contains_key("makepad/apps/game/src/old.rs"));
        assert_eq!(host.rev("makepad").as_deref(), Some(r2));
        // refused: a stale base, a payload that is not the signed one, an order by the device itself
        let a = send(order(&master, &format!("files makepad {r1} {r2} {} {}", hash(&p2), p2.len())), &p2);
        assert!(a.contains("not a fast-forward"), "{a}");
        let mut forged = p2.clone();
        *forged.last_mut().unwrap() ^= 1;
        let a = send(order(&master, &format!("files makepad {r2} {r1} {} {}", hash(&p2), p2.len())), &forged);
        assert!(a.contains("hash"), "{a}");
        let a = send(order(&device, &format!("files makepad {r2} {r1} {} {}", hash(&p2), p2.len())), &p2);
        assert!(a.contains("may not sign"), "{a}");
        assert_eq!(host.rev("makepad").as_deref(), Some(r2));
        assert_eq!(host.patched.lock().unwrap().len(), 4);
        assert!(events.lock().unwrap().iter().any(|l| l.contains("into the JIT store")));
        let _ = std::fs::remove_file(&sock);
    }
}
