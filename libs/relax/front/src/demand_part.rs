//! Crate-local planning records carried by the existing graph image. All binding
//! identities are payload-local IDs over spellings, never process binding IDs.
use super::*;
use crate::crate_image::{fields, Codec, Decode, DemandSlice, DemandWriter};
use crate::arena::Arena;
use crate::crate_image::strings::{self, Pack, Strings};
strings::fields!(FileIndex, ci, path, module, names, glob_names, impls, blankets, force);

pub(super) type Scan = (String, String, Option<u128>, Option<Arc<Scanned>>, BTreeSet<String>, Vec<(String, usize)>);

#[derive(Clone, Default)]
pub struct DemandPart {
    pub key: u128,
    pub inputs: Vec<(String, u128)>,
    pub probes: Vec<(String, bool)>,
    pub late_inputs: Vec<(String, u8, String, u128)>,
    pub(super) scans: Vec<Scan>,
    pub(super) aliases: Vec<(String, String)>,
    pub(super) dependencies: Vec<u128>,
    pub(super) namespace: Vec<u8>,
    pub(super) namespace_id: u128,
    // Identity of what preparing this crate's namespace consumed (Names::snapshot).
    pub(super) input_id: u128,
    pub(super) surface_id: u128,
    pub(super) macros: MacroPart,
    pub(super) files: Vec<FileIndex>,
    pub(super) unindexed: Vec<bool>,
    pub(super) items: BTreeMap<(String, String), Arc<Vec<items::Item>>>,
    pub(super) eager: BTreeMap<(String, String), BTreeSet<u32>>,
    pub(super) outlined: BTreeSet<(String, String)>,
    // Scopes whose scans carry a certified expanded external declaration index.
    pub(super) failures: BTreeMap<(String, String), bool>,
    pub(super) boundary: Boundary,
    pub(super) roots: Option<(BTreeSet<String>, BTreeSet<String>, usize)>,
    pub(super) closure_delta: Option<Arc<demand::ClosureDelta>>,
    pub(super) reached: BTreeSet<String>,
    pub(super) counts: (usize, usize),
}
// Graph certificates decode only headers. The large outline payload is a
// content-addressed row in that same graph store, opened on a planning worker.
impl Codec for DemandPart {
    fn put(&self, w: &mut Vec<u8>) {
        self.key.put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Self { key: Codec::get(r)?, ..Self::default() })
    }
}
const PART_VERSION: u32 = 8;
const PART_DOMAIN: &str = "demand-part-v8";

type Resolve<'a> = dyn FnMut(&str, u128) -> Option<&'static [u8]> + 'a;

fn put_defs<'a>(defs: impl ExactSizeIterator<Item = &'a crate::macros::MacroDef>, w: &mut Vec<u8>, images: &mut dyn DemandWriter) -> Option<usize> {
    defs.len().put(w);
    let mut saved = 0;
    for def in defs { let (key, bytes) = images.definition(def)?; key.put(w); saved += bytes - 16; }
    Some(saved)
}
fn get_defs(r: &mut Decode<'_>, store: &str, images: &mut Resolve<'_>, definitions: &mut HashMap<u128, Arc<crate::macros::MacroDef>>) -> Option<Vec<Arc<crate::macros::MacroDef>>> {
    let keys = Vec::<u128>::get(r)?;
    keys.into_iter().map(|key| {
        if let Some(def) = definitions.get(&key) { return Some(def.clone()); }
        let image = crate::image::Reader::new(images(store, key)?)?;
        let bytes = image.slice::<u8>(0, 3)?;
        if crate::crate_image::file_hash("demand-front-definition-v1", bytes) != key { return None; }
        let def = Arc::new(crate::crate_image::decode::<crate::macros::MacroDef>(bytes)?);
        definitions.insert(key, def.clone());
        Some(def)
    }).collect()
}
fn put_surface(path: &str, hash: u128, src: &[u8], toks: &[Tok], w: &mut Vec<u8>, images: &mut dyn DemandWriter) -> Option<()> {
    // Identity is explicit, including for unselected files without a parsed image.
    path.to_owned().put(w);
    hash.put(w);
    images.surface(path, hash, src, toks)?.put(w);
    Some(())
}
fn get_surface(r: &mut Decode<'_>, store: &str, images: &mut Resolve<'_>) -> Option<(String, u128, ScanBytes, Arena<Tok>)> {
    let path = String::get(r)?;
    let hash = u128::get(r)?;
    let (src, toks) = <(DemandSlice, DemandSlice)>::get(r)?;
    let source = crate::image::Reader::new(images(store, src.image)?)?;
    let tokens = crate::image::Reader::new(images(store, toks.image)?)?;
    let source_hash = src.hash;
    let token_hash = toks.hash;
    let src = source.slice::<u8>(0, 1)?.get(src.start..src.start.checked_add(src.len)?)?;
    let toks = tokens.slice::<Tok>(1, 2)?.get(toks.start..toks.start.checked_add(toks.len)?)?;
    if crate::crate_image::file_hash("", src) != source_hash || crate::crate_image::demand_token_hash(toks) != token_hash { return None; }
    if toks.iter().any(|t| t.lo > t.hi || t.hi as usize > src.len()) { return None; }
    Some((path, hash, ScanBytes::Mapped(src), Arena::Mapped(toks)))
}
impl DemandPart {
    pub fn encode_body(&self, images: &mut dyn DemandWriter, profile: bool, name: &str) -> Option<(u128, Vec<u8>)> {
        let _mem = crate::memstat::tag(crate::memstat::Tag::Planner);
        let mut w = Vec::new();
        PART_VERSION.put(&mut w);
        self.inputs.put(&mut w);
        self.probes.put(&mut w);
        self.late_inputs.put(&mut w);
        images.store_name().to_owned().put(&mut w);
        let mut sizes = BTreeMap::<&str, (usize, usize)>::new();
        sizes.insert("header", (w.len(), w.len()));
        let start = w.len();
        self.scans.len().put(&mut w);
        for (path, module, hash, scan, parameters, positions) in &self.scans {
            path.put(&mut w); module.put(&mut w); hash.put(&mut w);
            scan.is_some().put(&mut w);
            if let Some(scan) = scan {
                scan.src_len.put(&mut w); scan.probes.put(&mut w);
                put_surface(path, (*hash)?, &scan.src, &scan.toks, &mut w, images)?;
                put_defs(scan.defs.iter(), &mut w, images)?;
                scan.children.put(&mut w); scan.aliases.put(&mut w); scan.macro_use.put(&mut w);
                scan.uses.put(&mut w); scan.mods.put(&mut w);
            }
            parameters.put(&mut w); positions.put(&mut w);
        }
        sizes.insert("scans", (w.len() - start, w.len() - start));
        let mut prefix = w;
        let mut w = Vec::new();
        let mut strings = Strings::default();
        let mut macro_records = HashMap::default();
        macro_rules! packed {
            ($field:ident) => {{
                let start = w.len();
                self.$field.pack(&mut w, &mut strings);
                sizes.insert(stringify!($field), (if profile { self.$field.plain_size() } else { 0 }, w.len() - start));
            }};
        }
        packed!(aliases);
        packed!(dependencies);
        let start = w.len();
        // the same bytes as Vec<u8>::put (length, then the bytes), in one copy
        self.namespace.len().put(&mut w);
        w.extend_from_slice(&self.namespace);
        sizes.insert("namespace_data", (4 + if profile { demand::namespace_legacy_size(&self.namespace)? } else { self.namespace.len() }, w.len() - start));
        let start = w.len();
        self.namespace_id.put(&mut w);
        self.surface_id.put(&mut w);
        self.input_id.put(&mut w);
        sizes.insert("identities", (w.len() - start, w.len() - start));
        let start = w.len();
        self.macros.put_referenced(&mut w, images, &mut strings, &mut macro_records)?;
        sizes.insert("macros", (if profile { self.macros.referenced_size() } else { 0 }, w.len() - start));
        packed!(files);
        packed!(unindexed);
        packed!(items);
        packed!(eager);
        packed!(outlined);
        packed!(failures);
        packed!(boundary);
        packed!(roots);
        let start = w.len();
        self.closure_delta.put(&mut w);
        let old_delta = if profile { 1 + self.closure_delta.as_ref().map_or(0, |delta| delta.legacy_size()) } else { 0 };
        sizes.insert("closure_delta", (old_delta, w.len() - start));
        packed!(reached);
        packed!(counts);
        let before = prefix.len();
        strings.write_header(&mut prefix);
        sizes.insert("strings", (0, prefix.len() - before));
        prefix.extend_from_slice(&w);
        drop(w);
        let w = prefix;
        if profile {
            for (old, label, version) in [(true, "rust-demand-part-bytes-before", 5), (false, "rust-demand-part-bytes", PART_VERSION)] {
                let size = |key: &str| { let pair = sizes[key]; if old { pair.0 } else { pair.1 } };
                let sum = |keys: &[&str]| keys.iter().map(|key| size(key)).sum::<usize>();
                let items = sum(&["items", "eager", "outlined", "failures"]);
                let namespace = sum(&["aliases", "dependencies", "namespace_data", "identities"]);
                let other = sum(&["header", "macros", "files", "unindexed", "roots", "closure_delta", "reached", "counts", "strings"]);
                let total = size("scans") + items + namespace + size("boundary") + other;
                let fields = sizes.iter().filter(|(key, _)| !matches!(**key, "scans" | "items"))
                    .map(|(key, _)| format!(" {key}={}", size(key))).collect::<String>();
                eprintln!("{label}: crate={name} version={version} scans={} scans_src=0 scans_toks=0 items={items} namespace={namespace} contributions={} other={other} total={total} items_data={}{}",
                    size("scans"), size("boundary"), size("items"), fields);
                debug_assert!(old || total == w.len());
            }
        }
        Some((crate::crate_image::file_hash(PART_DOMAIN, &w), w))
    }
    pub fn header(&self, key: u128) -> Self {
        Self { key, inputs: self.inputs.clone(), probes: self.probes.clone(),
            late_inputs: self.late_inputs.clone(), ..Self::default() }
    }
    pub fn read_header(key: u128, bytes: &[u8]) -> Option<Self> {
        let mut r = Decode::new(bytes);
        if u32::get(&mut r)? != PART_VERSION { return None; }
        Some(Self { key, inputs: Codec::get(&mut r)?, probes: Codec::get(&mut r)?,
            late_inputs: Codec::get(&mut r)?, ..Self::default() })
    }
    pub fn read(key: u128, bytes: &[u8], images: &mut Resolve<'_>) -> Option<Self> {
        if crate::crate_image::file_hash(PART_DOMAIN, bytes) != key { return None; }
        let mut r = Decode::new(bytes);
        if u32::get(&mut r)? != PART_VERSION { return None; }
        let mut part = Self { key, inputs: Codec::get(&mut r)?, probes: Codec::get(&mut r)?,
            late_inputs: Codec::get(&mut r)?, ..Self::default() };
        let store = String::get(&mut r)?;
        let mut definitions = HashMap::default();
        let n = usize::get(&mut r)?;
        if n > r.bytes.len() { return None; }
        for _ in 0..n {
            let path = String::get(&mut r)?;
            let module = String::get(&mut r)?;
            let hash = Option::<u128>::get(&mut r)?;
            let scan = if bool::get(&mut r)? {
                let src_len = usize::get(&mut r)?;
                let probes = Codec::get(&mut r)?;
                let (source_path, source_hash, src, toks) = get_surface(&mut r, &store, images)?;
                if source_path != path || Some(source_hash) != hash { return None; }
                Some(Arc::new(Scanned { src_len, probes, src, toks,
                    defs: get_defs(&mut r, &store, images, &mut definitions)?.into_iter().map(|def| (*def).clone()).collect(), children: Codec::get(&mut r)?,
                    aliases: Codec::get(&mut r)?, macro_use: Codec::get(&mut r)?, uses: Codec::get(&mut r)?, mods: Codec::get(&mut r)? }))
            } else { None };
            part.scans.push((path, module, hash, scan, Codec::get(&mut r)?, Codec::get(&mut r)?));
        }
        let strings = strings::read_table(&mut r)?;
        part.aliases = Pack::unpack(&mut r, &strings)?;
        part.dependencies = Pack::unpack(&mut r, &strings)?;
        // the same bytes as Vec<u8>::get, which decodes them one at a time
        let namespace = usize::get(&mut r)?;
        part.namespace = r.take(namespace)?.to_vec();
        part.namespace_id = Codec::get(&mut r)?;
        part.surface_id = Codec::get(&mut r)?;
        part.input_id = Codec::get(&mut r)?;
        let mut macro_records = Vec::new();
        part.macros = MacroPart::get_referenced(&mut r, &store, images, &mut definitions, &strings, &mut macro_records)?;
        part.files = Pack::unpack(&mut r, &strings)?;
        part.unindexed = Pack::unpack(&mut r, &strings)?;
        part.items = Pack::unpack(&mut r, &strings)?;
        part.eager = Pack::unpack(&mut r, &strings)?;
        part.outlined = Pack::unpack(&mut r, &strings)?;
        part.failures = Pack::unpack(&mut r, &strings)?;
        part.boundary = Pack::unpack(&mut r, &strings)?;
        part.roots = Pack::unpack(&mut r, &strings)?;
        part.closure_delta = Codec::get(&mut r)?;
        part.reached = Pack::unpack(&mut r, &strings)?;
        part.counts = Pack::unpack(&mut r, &strings)?;
        r.bytes.is_empty().then_some(part)
    }
}

// The alias and expansion maps are the table's own per-crate maps, shared: a
// deferred file's capture copied them once per file (0.17 GB on a cold Stage).
#[derive(Clone, Default)]
pub(super) struct MacroPart {
    definitions: Vec<Arc<crate::macros::MacroDef>>,
    aliases: Arc<HashMap<String, (String, String)>>,
    uses: Vec<String>,
    modules: Option<Arc<crate::macros::CrateMods>>,
    expansions: Arc<HashMap<(String, String), Vec<String>>>,
    s2_fmt: bool,
}
fields!(MacroPart, definitions, aliases, uses, modules, expansions, s2_fmt);
impl MacroPart {
    fn referenced_size(&self) -> usize {
        4 + self.definitions.len() * 16 + self.aliases.plain_size() + self.uses.plain_size()
            + self.modules.plain_size() + self.expansions.plain_size() + 1
    }
    fn put_referenced(&self, w: &mut Vec<u8>, images: &mut dyn DemandWriter,
        strings: &mut Strings, records: &mut HashMap<Vec<u8>, usize>) -> Option<()> {
        let mut bytes = Vec::new();
        put_defs(self.definitions.iter().map(AsRef::as_ref), &mut bytes, images)?;
        self.aliases.pack(&mut bytes, strings); self.uses.pack(&mut bytes, strings);
        self.modules.pack(&mut bytes, strings); self.expansions.pack(&mut bytes, strings); self.s2_fmt.put(&mut bytes);
        // Deferred files often capture the same crate macro environment. Keep
        // one record in this part, including its referenced definition list.
        if let Some(id) = records.get(&bytes) { id.put(w); } else {
            let id = records.len(); id.put(w);
            bytes.put(w); records.insert(bytes, id);
        }
        Some(())
    }
    fn get_referenced(r: &mut Decode<'_>, store: &str, images: &mut Resolve<'_>, definitions: &mut HashMap<u128, Arc<crate::macros::MacroDef>>,
        strings: &[String], records: &mut Vec<Self>) -> Option<Self> {
        let id = usize::get(r)?;
        if id < records.len() { return Some(records[id].clone()); }
        if id != records.len() { return None; }
        let len = usize::get(r)?;
        let mut r = Decode::new(r.take(len)?);
        let value = Self { definitions: get_defs(&mut r, store, images, definitions)?,
            aliases: Pack::unpack(&mut r, strings)?, uses: Pack::unpack(&mut r, strings)?, modules: Pack::unpack(&mut r, strings)?,
            expansions: Pack::unpack(&mut r, strings)?, s2_fmt: Codec::get(&mut r)? };
        if !r.bytes.is_empty() { return None; }
        records.push(value.clone());
        Some(value)
    }

    pub(super) fn inherit_definitions(&mut self, other: &Self) { self.definitions = other.definitions.clone(); }
    pub(super) fn note_definition(&mut self, krate: &str, def: &Arc<crate::macros::MacroDef>) {
        if def.krate == krate { self.definitions.push(def.clone()); }
    }
    pub(super) fn finish_capture(&mut self, table: &crate::macros::MacroTable, krate: &str) {
        // Only the current crate's maps; definitions were already encountered
        // while publishing the outline, never rescan/sort the whole table.
        self.definitions.sort_by(|a, b| a.name.cmp(&b.name));
        self.aliases = table.aliases.get(krate).cloned().unwrap_or_default();
        self.uses = table.macro_use.get(krate).cloned().unwrap_or_default();
        self.modules = table.crates.get(krate).cloned();
        self.expansions = table.exp_uses.get(krate).cloned().unwrap_or_default();
        self.s2_fmt = table.s2_fmt;
    }
    pub(super) fn publish(&self, table: &mut crate::macros::MacroTable, krate: &str) {
        table.clear_lookup_cache();
        let mut existing = table.defs.iter().map(|(name, defs)|
            (name.clone(), defs.iter().filter(|def| def.krate == krate).count())).collect::<HashMap<_, _>>();
        for def in &self.definitions {
            let count = existing.entry(def.name.clone()).or_default();
            if *count > 0 { *count -= 1; } else { table.push(def.clone()); }
        }
        table.aliases.insert(krate.to_owned(), self.aliases.clone());
        table.macro_use.insert(krate.to_owned(), self.uses.clone());
        if let Some(modules) = &self.modules { table.add_crate(krate.to_owned(), modules.clone()); }
        table.exp_uses.insert(krate.to_owned(), self.expansions.clone());
        table.forget_expansion_index(krate);
        table.s2_fmt = self.s2_fmt;
    }
}
impl DemandPart {
    pub(super) fn seal_identity(&mut self) {
        let mut hash = crate::sip::Sip128::new();
        hash.u128(self.namespace_id);
        hash.u128(crate::crate_image::file_hash("", &crate::crate_image::encode(&self.macros)));
        self.surface_id = hash.finish128();
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Boundary {
    pub declared: BTreeSet<String>,
    pub used: BTreeSet<String>,
    pub members: BTreeSet<String>,
    pub traits: BTreeMap<String, BTreeSet<String>>,
}
fields!(Boundary, declared, used, members, traits);
strings::fields!(Boundary, declared, used, members, traits);
impl Boundary {
    pub fn extend(&mut self, other: &Self) {
        self.declared.extend(other.declared.iter().cloned());
        self.used.extend(other.used.iter().cloned());
        self.members.extend(other.members.iter().cloned());
        for (name, members) in &other.traits { self.traits.entry(name.clone()).or_default().extend(members.iter().cloned()); }
    }
    pub fn same_outgoing(&self, other: &Self, krate: &str) -> bool {
        let external = |name: &&String| !demand::within(name, krate);
        self.declared.iter().filter(external).eq(other.declared.iter().filter(external))
            && self.used.iter().filter(external).eq(other.used.iter().filter(external))
            && self.members == other.members && self.traits == other.traits
    }
}
impl DemandPart {
    pub(super) fn same_transfer(&self, old: &Self) -> bool {
        self.aliases == old.aliases && self.unindexed == old.unindexed
            && self.files.len() == old.files.len() && self.files.iter().zip(&old.files).all(|(a, b)|
                (a.path.as_str(), a.module.as_str(), a.force) == (b.path.as_str(), b.module.as_str(), b.force))
            && self.scans.len() == old.scans.len() && self.scans.iter().zip(&old.scans).all(|(a, b)| {
                if a.0 != b.0 || a.1 != b.1 { return false; }
                match (&a.3, &b.3) {
                    (Some(a_scan), Some(b_scan)) => items::same_transfer(
                        (&a_scan.src, &a_scan.toks, self.eager.get(&(a.0.clone(), a.1.clone()))),
                        (&b_scan.src, &b_scan.toks, old.eager.get(&(b.0.clone(), b.1.clone())))),
                    (None, None) => true,
                    _ => false,
                }
            })
    }
}

// Planning gives the writer the data it already owns. Namespace serialization,
// identity hashing and part encoding all happen there.
pub(crate) struct Recording {
    pending: relax_session::sync::Mutex<Option<(Vec<PreCrate>, Names, Vec<DemandPart>)>>,
    ready: relax_session::sync::Mutex<Vec<Arc<DemandPart>>>,
}
impl Recording {
    pub(super) fn new(crates: &[PreCrate], names: Names, parts: Vec<DemandPart>) -> Self {
        Self { pending: relax_session::sync::Mutex::new(Some((crates.to_vec(), names, parts))), ready: relax_session::sync::Mutex::new(Vec::new()) }
    }
}
impl DemandImage {
    pub fn recorded_parts(&self) -> Vec<Arc<DemandPart>> {
        let _mem = crate::memstat::tag(crate::memstat::Tag::Planner);
        let Some(recording) = &self.recording else { return self.parts.clone(); };
        // held while recording: a writer that comes while the load's prepare thread records
        // waits for its parts instead of finding neither pending nor ready ones
        let mut pending = recording.pending.lock().unwrap();
        let Some((crates, final_names, mut parts)) = pending.take() else { return recording.ready.lock().unwrap().clone(); };
        let profile = crates.first().is_some_and(|c| c.cfg.session.runtime.profile);
        let _profile = crate::load_profile::span(profile, "rust-demand-record", "graph writer");
        let t0 = std::time::Instant::now();
        let mut names = Names::new(&crates);
        // A crate's namespace is final once it is prepared: imports bind only in scopes of
        // their own crate (Names::prepare_crate). So the namespaces are taken after the loop,
        // in one pass over the tables, except where crates share a name (a package's library
        // and binary): those are taken in place, as each one's projection at that point.
        let mut shared = HashMap::<&str, usize>::default();
        for c in &crates { *shared.entry(c.name.as_str()).or_default() += 1; }
        let mut later = Vec::new();
        let mut fresh = vec![false; parts.len()];
        for (ci, part) in parts.iter_mut().enumerate() {
            if part.namespace.is_empty() {
                for (_, module, _, scan, ..) in &part.scans {
                    if let Some(scan) = scan { names.add(ci, module, &scan.src, &scan.toks, &scan.uses, &scan.mods); }
                }
                names.recorded_indexes(&part.files);
                names.prepare_crate(ci);
                if shared[crates[ci].name.as_str()] > 1 { (part.namespace, part.input_id) = names.snapshot(ci); } else { later.push(ci); }
                fresh[ci] = true;
            } else { names.restore(ci, &part.namespace, false).expect("planner namespace"); }
        }
        let t_loop = t0.elapsed();
        for (ci, (namespace, input)) in later.iter().copied().zip(names.snapshots(&later)) { (parts[ci].namespace, parts[ci].input_id) = (namespace, input); }
        let t_snap = t0.elapsed();
        let mut identities = Vec::new();
        for (ci, part) in parts.iter_mut().enumerate() {
            if fresh[ci] {
                part.namespace_id = crate::crate_image::file_hash("", &part.namespace);
                part.dependencies = names.reference_crates(ci).into_iter().filter(|&d| d < ci).map(|d| identities[d]).collect();
                part.seal_identity();
            }
            identities.push(part.surface_id);
        }
        let t_seal = t0.elapsed();
        if let Some(first) = parts.first_mut() {
            let value = final_names.closure_delta(&names);
            // Both namespaces end here (planner + rebuilt, ~1 GB on a cold Stage): not held
            // through the delta's encode, its shrink copy and the parts' publication.
            drop((final_names, names));
            let legacy = if profile { value.legacy_size(true) } else { 0 };
            let delta = demand::ClosureDelta::new(&value, legacy);
            drop(value);
            // The closure delta covers this graph, including other edited crates.
            // An equivalent local edit leaves this row untouched too.
            if first.closure_delta.as_ref().is_none_or(|old| !old.same_image(&delta)) {
                first.key = 0;
            }
            first.closure_delta = Some(Arc::new(delta));
        }
        if crate::preload::trace() {
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
            eprintln!("rx-demand-record: {} parts in {:.0} ms: prepare {:.0}, namespaces {:.0} ({} in one pass), identities+boundaries {:.0}, closure delta {:.0}",
                parts.len(), ms(t0.elapsed()), ms(t_loop), ms(t_snap - t_loop), later.len(), ms(t_seal - t_snap), ms(t0.elapsed() - t_seal));
        }
        let parts = parts.into_iter().map(Arc::new).collect::<Vec<_>>();
        *recording.ready.lock().unwrap() = parts.clone();
        drop(pending);
        parts
    }
    pub fn recording_written(&self, headers: Vec<Arc<DemandPart>>) {
        if let Some(recording) = &self.recording {
            let old = std::mem::replace(&mut *recording.ready.lock().unwrap(), headers);
            // Release the decoded outlines and reference sets on the writer.
            drop(old);
        }
    }
}
