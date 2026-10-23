//! The startup image of a manifest (cache.rs): the code of the slots a program's start installed
//! before its first present, stored once in the manifest entry -- each slot's code as its code
//! entry has it, with its relocations re-expressed against tables of the identities they name
//! (slot keys, symbols, files), each once, and the crates its code is made from with their whole
//! fingerprints. A later run with the same manifest resolves each identity once, places the code
//! of every slot whose crates are unchanged in one block of code memory, links it there in one
//! pass and installs those slots before main (`prelink`), instead of a key, a lookup, a
//! certificate check, a link and an install per slot on its first call. The image is part of
//! the manifest entry, not a store of its own; it is written again with the manifest when it did
//! not prelink whole.
//!
//! The slots the image's keys name are made in one go, in image order, with their identities
//! still encoded (`ImageIds`): relocations between image slots go by image key index, and a key
//! is decoded (`complete_identities`) after main started, on a background thread, or at once by
//! the first thread to make a unit call for anything else (jit.rs `ids_on_serve`), so every
//! caller on the owner thread sees every slot's key. A lookup by encoded bytes finds an image slot through
//! the image's sorted key index, with or without its identity decoded.

use super::*;

impl Cache {
    /// The prelinked startup image of slots `ids` (the manifest's slots before the first present):
    /// each slot's code as its code entry has it -- the entry the next run's lookup would link
    /// (`image_payload`) -- with its relocations re-expressed against tables of the identities
    /// they name (slot keys, symbols, files: each once), and the crates its code is made from
    /// (the crates its key names and those of its dependency certificate) with their whole
    /// fingerprints now. A slot whose entry cannot be read here is left out (it links lazily).
    /// With the image, the slots it holds.
    /// Err: `stop` said yes (asked every 256 slots); the caller drops the manifest for now.
    pub(super) fn build_image(&mut self, u: &Unit, ids: &[u32], stop: &dyn Fn() -> bool) -> Result<Option<(Vec<u8>, u32)>, ()> {
        let t0 = std::time::Instant::now();
        let mut ph = [0f64; 5];
        // the entry each slot takes, decided first (slot keys need the cache mutably)
        let mut chosen: Vec<(u32, u128, Vec<u32>)> = Vec::with_capacity(ids.len());
        let pend: HashMap<u128, usize> = self.pending.iter().enumerate().filter(|(_, e)| e.1.kind == KIND_CODE).map(|(i, e)| (e.1.key, i)).collect();
        for (i, &id) in ids.iter().enumerate() {
            if i % 256 == 255 && stop() { return Err(()); }
            // interpreted slots link one by one (their code is not machine code to place)
            if relax_codegen::interp::is_entry(u.fns[id as usize].code) { continue; }
            let Some(key0) = self.slot_key(u, id) else { continue };
            let crates = self.slot_crates.get(&id).cloned().unwrap_or_default();
            if let Some(k) = self.image_entry(id, key0, &pend) {
                chosen.push((id, k, crates));
            }
        }
        let mut tab = ImageTables::default();
        let mut slots = E::default();
        let mut blob: Vec<u8> = Vec::new();
        let mut n = 0u32;
        for (i, (id, k, crates)) in chosen.iter().enumerate() {
            if i % 256 == 255 && stop() { return Err(()); }
            let Some(payload) = self.image_payload(*k, &pend) else { continue };
            let Some(bytes) = self.slot_bytes_known(*id) else { continue };
            let mut cr: Vec<u32> = crates.clone();
            if image_slot(&mut tab, &mut slots, &mut blob, *k, bytes, payload, &mut cr, &self.crate_by_ident).is_some() {
                n += 1;
            }
        }
        ph[0] = t0.elapsed().as_secs_f64() * 1e3;
        if n == 0 {
            return Ok(None);
        }
        // the data objects the symbols name (tag 2), and the ones those name in turn: the
        // prelink places them as one block (place_data) instead of a link_data per object
        let pend_data: HashMap<u128, usize> = self.pending.iter().enumerate().filter(|(_, e)| e.1.kind == KIND_DATA).map(|(i, e)| (e.1.key, i)).collect();
        let mut data = ImageData::default();
        let mut i = 0;
        while i < tab.syms.list.len() {
            let b = &tab.syms.list[i];
            if b.first() == Some(&2) && b.len() >= 17 {
                let k = u128::from_le_bytes(b[1..17].try_into().unwrap());
                let o = data.index(k);
                data.sym.push((i as u32, o));
            } else if b.first() == Some(&3) {
                // a static: its cached initial value (const_value's object) goes with the data
                let b = b.clone();
                if let Some(o) = self.static_value_obj(u, &b, &mut data) {
                    data.statics.push((i as u32, o));
                }
            }
            i += 1;
            while let Some(o) = data.queue.pop() {
                self.image_data_obj(&mut tab, &mut data, o as usize, &pend_data);
            }
        }
        data.settle();
        ph[1] = t0.elapsed().as_secs_f64() * 1e3;
        // the functions thunk symbols name (fn pointers in data: vtables, callbacks) are keys
        // too: the prelink makes their slots with the others, so linking a thunk is a lookup by
        // bytes instead of a fn key decode per symbol (Stage: ~18k of 22k thunks)
        let mut thunk_keys = Vec::new();
        for i in 0..tab.syms.list.len() {
            let b = &tab.syms.list[i];
            if b.first() == Some(&7) && b.len() > 9 {
                let key = b[1..b.len() - 8].to_vec();
                thunk_keys.push(tab.keys.id(&key));
            }
        }
        thunk_keys.sort_unstable();
        thunk_keys.dedup();
        // per key, the crates it names: a later run decodes a key that names a crate changed
        // since (or one not known here) before it links any slot to it (`prelink`)
        let key_crates: Vec<Option<Vec<u32>>> = (0..tab.keys.list.len()).map(|k| {
            let b = std::mem::take(&mut tab.keys.list[k]);
            let r = self.key_crates(u, &b);
            tab.keys.list[k] = b;
            r
        }).collect();
        ph[2] = t0.elapsed().as_secs_f64() * 1e3;
        for c in key_crates.iter().flatten().flatten() {
            let next = tab.crates.len() as u32;
            if *tab.crate_at.entry(*c).or_insert(next) == next {
                tab.crates.push(*c);
            }
        }
        let mut e = E::default();
        e.u32(tab.crates.len() as u32);
        for &c in &tab.crates {
            e.u128(self.crate_ident[c as usize]);
            e.u128(self.fps[c as usize]);
        }
        for t in [&tab.keys, &tab.syms, &tab.files] {
            e.u32(t.list.len() as u32);
            for b in &t.list {
                e.bytes(b);
            }
        }
        // the keys' indices in the order of their bytes: a lookup by bytes is a binary search
        let mut sorted: Vec<u32> = (0..tab.keys.list.len() as u32).collect();
        sorted.sort_unstable_by(|&a, &b| tab.keys.list[a as usize].cmp(&tab.keys.list[b as usize]));
        e.u32(sorted.len() as u32);
        for k in sorted {
            e.u32(k);
        }
        e.u32(thunk_keys.len() as u32);
        for k in thunk_keys {
            e.u32(k);
        }
        for kc in &key_crates {
            match kc {
                Some(cs) => {
                    e.u32(cs.len() as u32);
                    for c in cs {
                        e.u32(tab.crate_at[c]);
                    }
                }
                None => e.u32(u32::MAX),
            }
        }
        data.encode(&mut e);
        ph[3] = t0.elapsed().as_secs_f64() * 1e3;
        e.u32(n);
        e.0.extend_from_slice(&slots.0);
        e.bytes(&blob);
        if self.verbose {
            eprintln!("makepad-run cache: startup image of {} slots ({} keys, {} symbols, {} crates, {} KB code); ms: slots to {:.1}, data to {:.1}, key crates to {:.1}, encoded {:.1}", n, tab.keys.list.len(), tab.syms.list.len(), tab.crates.len(), blob.len() / 1024, ph[0], ph[1], ph[2], ph[3]);
        }
        Ok(Some((e.0, n)))
    }

    /// The loaded startup image is not to be trusted (`why`): code reached one of its slots
    /// without an identity (a key that decodes for sure did not, after code was linked to its
    /// slot). That may not happen; if it does, the manifest is replaced at once by an empty one
    /// (no slots, no image), so the next run starts without it and records a fresh one: a broken
    /// image never outlives the run that found it.
    pub(super) fn image_broken(&mut self, why: &str) {
        if self.image_bad {
            return;
        }
        self.image_bad = true;
        self.image_rewrite = true;
        eprintln!("makepad-run cache: startup image dropped: {}", why);
        let Some(root) = self.manifest_root else { return };
        let Some(key) = self.manifest_key(root) else { return };
        if self.mode == Mode::ReadOnly {
            return;
        }
        let mut e = E::default();
        e.u32(ENTRY_VERSION);
        e.u32(0);
        e.u32(0);
        self.push_pending(root, NewEntry { key, kind: KIND_MANIFEST, payload: e.0 });
        if let Err(err) = self.flush() {
            eprintln!("makepad-run cache: dropping the startup image: {}", err);
        }
    }

    /// The crates (of this run) image key `b` names: the crates of the definitions it names and
    /// of the files whose closures it names. None: one of them is not known here.
    pub(super) fn key_crates(&mut self, u: &Unit, b: &[u8]) -> Option<Vec<u32>> {
        let mut atoms = Vec::new();
        skip_fnkey(&mut D { b, p: 0 }, 0, &mut |a| atoms.push(a))?;
        let mut out = Vec::with_capacity(atoms.len());
        for a in atoms {
            out.push(match a {
                Atom::Crate(ident) => *self.crate_by_ident.get(&ident)?,
                Atom::File(ident) => {
                    let f = self.file_by_ident(u, ident)?;
                    *u.prog.file_crate.get(f as usize)?
                }
            });
        }
        out.sort_unstable();
        out.dedup();
        Some(out)
    }

    /// The data object of the cached initial value of the static image symbol `b` names (tag 3:
    /// its definition), if the cache has one.
    fn static_value_obj(&mut self, u: &Unit, b: &[u8], data: &mut ImageData) -> Option<u32> {
        let def = self.dec_def(u, &mut D { b, p: 1 })?;
        let (key, _) = self.const_key(u, def, &[])?;
        let payload = match self.pending.iter().find(|e| e.1.key == key && e.1.kind == KIND_CONST) {
            Some(e) => &e.1.payload[..],
            None => match self.get(key) {
                Some((KIND_CONST, p)) => p,
                _ => return None,
            },
        };
        let dk = u128::from_le_bytes(payload.try_into().ok()?);
        Some(data.index(dk))
    }

    /// Data object `o` of `data` (its key known): its record from its data entry (recorded this
    /// run, or in the index); the objects it points to are queued.
    fn image_data_obj(&self, tab: &mut ImageTables, data: &mut ImageData, o: usize, pend: &HashMap<u128, usize>) {
        let key = data.objs[o].key;
        let payload = match pend.get(&key) {
            Some(&i) => Some(&self.pending[i].1.payload[..]),
            None => match self.get(key) {
                Some((KIND_DATA, p)) => Some(p),
                _ => None,
            },
        };
        let rec = payload.and_then(|p| (|| {
            let mut d = D { b: p, p: 0 };
            if d.u32()? != ENTRY_VERSION {
                return None;
            }
            let align = d.u64()?;
            let name = match d.u8()? {
                0 => None,
                1 => Some(d.bytes()?.to_vec()),
                _ => return None,
            };
            let bytes = d.bytes()?;
            let n = d.u32()? as usize;
            let mut rel = Vec::with_capacity(n);
            let mut end = 0;
            for _ in 0..n {
                let off = d.u32()?;
                let at = off as usize;
                if at < end || at.checked_add(8)? > bytes.len() {
                    return None;
                }
                end = at + 8;
                if d.b.get(d.p) == Some(&2) {
                    d.u8()?;
                    let k = d.u128()?;
                    let add = d.u64()?;
                    rel.push((off, DATA_OBJ, data.index(k), add));
                } else {
                    rel.push((off, DATA_SYM, tab.sym(&mut d)?, 0));
                }
            }
            (d.p == p.len()).then(|| (align, name, bytes.to_vec(), rel))
        })());
        let r = &mut data.objs[o];
        if let Some((align, name, bytes, rel)) = rec {
            r.ok = true;
            r.align = align;
            r.name = name;
            r.bytes = bytes;
            r.rel = rel;
        }
    }

    /// Slot `id`'s encoded key, already computed (memo of `slot_bytes`), or the key the prelink
    /// made it from.
    fn slot_bytes_known(&self, id: u32) -> Option<Vec<u8>> {
        self.fnkey_bytes.get(&id).cloned().flatten().or_else(|| Some(self.image_ids.as_ref()?.bytes_of(id)?.to_vec()))
    }

    /// The code entry a later run's lookup takes for slot `id` (its key `key0`), from what this
    /// run knows to be valid: the entry recorded now, the entry the slot was linked from, or an
    /// indexed entry whose certificate holds on its crates' fingerprints alone. (An entry counts
    /// as valid when this run recorded it, linked it, or its certificate's fast path holds.)
    fn image_entry(&self, id: u32, key0: u128, pend: &HashMap<u128, usize>) -> Option<u128> {
        let linked = self.linked_key.get(&id).copied();
        let valid = |k: u128| -> bool {
            let Some(p) = self.image_payload(k, pend) else { return false };
            !self.cert_rejected.contains(&k) && (pend.contains_key(&k) || linked == Some(k) || self.cert_fast_path(p))
        };
        if let Some(k) = linked.filter(|&k| valid(k)) {
            return Some(k);
        }
        valid(key0).then_some(key0)
    }

    /// A code entry's payload: recorded this run (not yet written), or in the index.
    fn image_payload<'a>(&'a self, k: u128, pend: &HashMap<u128, usize>) -> Option<&'a [u8]> {
        if let Some(&i) = pend.get(&k) {
            return Some(&self.pending[i].1.payload);
        }
        match self.get(k) {
            Some((KIND_CODE, p)) => Some(p),
            _ => None,
        }
    }

    /// Whether a code entry's dependency certificate holds by its fast path (the crates it names
    /// have the fingerprints it was made with).
    fn cert_fast_path(&self, payload: &[u8]) -> bool {
        (|| {
            let mut d = D { b: payload, p: 0 };
            if d.u32()? != ENTRY_VERSION {
                return None;
            }
            d.str()?;
            let (_, nounwind_end) = skip_build_inputs(&mut d)?;
            let mut d = D { b: payload, p: nounwind_end };
            let nc = d.u32()? as usize;
            let mut idents = Vec::with_capacity(nc.min(64));
            for _ in 0..nc {
                idents.push(d.u128()?);
            }
            let fps = d.u128()?;
            Some(self.cert_fps(&idents) == Some(fps))
        })().unwrap_or(false)
    }
}

/// The startup image section of a manifest ("PLI7" after the slot list).
pub(super) const IMAGE_MAGIC: u32 = 0x3749_4c50;

/// A loaded manifest's image section, read in place: the store it is mapped from stays open.
pub(super) struct ManifestImage {
    pub(super) _store: std::sync::Arc<Store>,
    pub(super) at: usize,
    pub(super) len: usize,
}

impl ManifestImage {
    pub(super) fn bytes(&self) -> &'static [u8] {
        // the Arc keeps the store (and so the mapping) alive; callers hold a clone of it
        unsafe { std::slice::from_raw_parts(self.at as *const u8, self.len) }
    }
}

/// Byte strings numbered in first-seen order (an image's identity tables).
#[derive(Default)]
struct Interned {
    list: Vec<Vec<u8>>,
    at: HashMap<Vec<u8>, u32>,
}

impl Interned {
    fn id(&mut self, b: &[u8]) -> u32 {
        if let Some(&i) = self.at.get(b) {
            return i;
        }
        let i = self.list.len() as u32;
        self.list.push(b.to_vec());
        self.at.insert(b.to_vec(), i);
        i
    }
}

/// An image's identity tables: slot keys, symbols and files, each once.
#[derive(Default)]
struct ImageTables {
    keys: Interned,
    syms: Interned,
    files: Interned,
    crates: Vec<u32>,
    crate_at: HashMap<u32, u32>,
}

impl ImageTables {
    /// One encoded symbol (`enc_sym`: tag, identity, offset) as a symbol table entry (its bytes as
    /// they are: `link_sym` reads them).
    fn sym(&mut self, d: &mut D) -> Option<u32> {
        let s = d.p;
        match d.u8()? {
            1 | 5 | 9 => {
                d.str()?;
            }
            2 | 10 => {
                d.u128()?;
            }
            3 | 4 => skip_def(d, &mut no_atoms)?,
            6 => {
                skip_file(d, &mut no_atoms)?;
                d.u32()?;
                d.str()?;
            }
            7 | 12 => skip_fnkey(d, 0, &mut no_atoms)?,
            8 => {
                d.u64()?;
                d.u64()?;
                d.u32()?;
            }
            _ => return None,
        }
        d.u64()?;
        Some(self.syms.id(&d.b[s..d.p]))
    }
}

/// A data object's pointer word in an image: another data object of the image (index, addend),
/// or a symbol of its table.
const DATA_OBJ: u8 = 0;
const DATA_SYM: u8 = 1;
/// An image data relocation record: offset u32, target u8, index u32, addend u64.
const DREL_SIZE: usize = 17;

/// The data objects of an image being built: by key, the ones still to read, and the symbols
/// (table index) that name each.
#[derive(Default)]
struct ImageData {
    objs: Vec<DataRec>,
    at: HashMap<u128, u32>,
    queue: Vec<u32>,
    sym: Vec<(u32, u32)>,
    /// static symbols and the object of their cached initial value
    statics: Vec<(u32, u32)>,
}

#[derive(Default)]
struct DataRec {
    key: u128,
    /// read, and every object it points to is too
    ok: bool,
    align: u64,
    name: Option<Vec<u8>>,
    bytes: Vec<u8>,
    rel: Vec<(u32, u8, u32, u64)>,
}

impl ImageData {
    /// The object keyed `k` (queued to be read on first sight).
    fn index(&mut self, k: u128) -> u32 {
        if let Some(&o) = self.at.get(&k) {
            return o;
        }
        let o = self.objs.len() as u32;
        self.objs.push(DataRec { key: k, ..Default::default() });
        self.at.insert(k, o);
        self.queue.push(o);
        o
    }

    /// An object pointing to one that could not be read cannot be placed either.
    fn settle(&mut self) {
        loop {
            let mut changed = false;
            for o in 0..self.objs.len() {
                if self.objs[o].ok && self.objs[o].rel.iter().any(|&(_, t, x, _)| t == DATA_OBJ && !self.objs[x as usize].ok) {
                    self.objs[o].ok = false;
                    changed = true;
                }
            }
            if !changed {
                return;
            }
        }
    }

    fn encode(&self, e: &mut E) {
        e.u32(self.objs.len() as u32);
        for r in &self.objs {
            e.u128(r.key);
            e.u8(r.ok as u8);
            if !r.ok {
                continue;
            }
            e.u64(r.align);
            match &r.name {
                Some(n) => {
                    e.u8(1);
                    e.bytes(n);
                }
                None => e.u8(0),
            }
            e.bytes(&r.bytes);
            e.u32(r.rel.len() as u32);
            for &(off, t, x, add) in &r.rel {
                e.u32(off);
                e.u8(t);
                e.u32(x);
                e.u64(add);
            }
        }
        for list in [&self.sym, &self.statics] {
            let ok: Vec<_> = list.iter().filter(|&&(_, o)| self.objs[o as usize].ok).collect();
            e.u32(ok.len() as u32);
            for &&(s, o) in &ok {
                e.u32(s);
                e.u32(o);
            }
        }
    }
}

/// Relocation targets in an image: a slot's table word, its thunk, a symbol, or the fixed values.
const IMG_SLOT: u8 = 0;
const IMG_THUNK: u8 = 1;
const IMG_SYM: u8 = 2;
const IMG_TLS_KEY: u8 = 3;
const IMG_TLS_SLOW: u8 = 4;
const IMG_RESUME: u8 = 5;
const IMG_CALL: u8 = 6;

/// One slot of a startup image, from code entry `entry` (key `entry_key`, slot key encoding
/// `key`): appended to `out` (its record) and `blob` (its code) unless the payload does not
/// decode, or names a crate this program does not have. `crates`:
/// the crates its key names (the certificate's are added).
#[allow(clippy::too_many_arguments)]
fn image_slot(tab: &mut ImageTables, out: &mut E, blob: &mut Vec<u8>, entry_key: u128, key: Vec<u8>, payload: &[u8], crates: &mut Vec<u32>, crate_by_ident: &HashMap<u128, u32>) -> Option<()> {
    let mut d = D { b: payload, p: 0 };
    if d.u32()? != ENTRY_VERSION {
        return None;
    }
    d.str()?;
    let tier = d.u8()?;
    let ni = d.u32()?;
    let mut inl = Vec::with_capacity(ni as usize);
    for _ in 0..ni {
        inl.push(tab.keys.id(d.bytes()?));
    }
    let n = d.u32()?;
    for _ in 0..n {
        d.bytes()?;
        d.u8()?;
    }
    let nc = d.u32()?;
    for _ in 0..nc {
        crates.push(*crate_by_ident.get(&d.u128()?)?);
    }
    d.u128()?;
    let n = d.u32()?;
    for _ in 0..n {
        d.bytes()?;
        d.u128()?;
    }
    let code = d.bytes()?;
    let nr = d.u32()? as usize;
    let mut rel: Vec<(u32, u8, u8, u32)> = Vec::with_capacity(nr);
    for _ in 0..nr {
        let at = d.u32()?;
        let kind = d.u8()?;
        kind_of(kind)?;
        let tag = d.b.get(d.p).copied()?;
        match tag {
            20 | 21 | 25 => {
                d.u8()?;
                rel.push((at, kind, match tag { 20 => IMG_SLOT, 25 => IMG_CALL, _ => IMG_THUNK }, tab.keys.id(d.bytes()?)));
            }
            22..=24 => {
                d.u8()?;
                rel.push((at, kind, IMG_TLS_KEY + (tag - 22), 0));
            }
            _ => rel.push((at, kind, IMG_SYM, tab.sym(&mut d)?)),
        }
    }
    let raw = crate::jit::PcMap::decode(d.bytes()?);
    let nf = d.u32()? as usize;
    let mut files = Vec::with_capacity(nf);
    for _ in 0..nf {
        let start = d.p;
        d.u128()?;
        files.push(tab.files.id(&payload[start..d.p]));
    }
    crates.sort_unstable();
    crates.dedup();
    let key = tab.keys.id(&key);
    let mut e = E::default();
    e.u128(entry_key);
    e.u32(key);
    e.u8(tier);
    e.u32(inl.len() as u32);
    for k in inl {
        e.u32(k);
    }
    e.u32(crates.len() as u32);
    for &c in crates.iter() {
        let next = tab.crates.len() as u32;
        let i = *tab.crate_at.entry(c).or_insert(next);
        if i == next {
            tab.crates.push(c);
        }
        e.u32(i);
    }
    e.u32(blob.len() as u32);
    e.u32(code.len() as u32);
    // the code without its unwind trailer (the trailer stays in the image: Unit::unwind_at)
    e.u32(relax_backend::rir::split_unwind_info(code).0 as u32);
    e.u32(rel.len() as u32);
    for (at, kind, t, ix) in rel {
        e.u32(at);
        e.u8(kind);
        e.u8(t);
        e.u32(ix);
    }
    // the pc map as this run's maps hold it (jit.rs PcMap), positions naming image files
    let mut pc = Vec::with_capacity(raw.len() * 3);
    crate::jit::PcMap::encode_to(&mut pc, raw.into_iter().map(|(o, p)| (o, (files.get(relax_backend::rir::pos_file(p) as usize).copied().unwrap_or(u32::MAX) as u64) << 32 | (p & 0xffff_ffff) | (p & !relax_backend::rir::pos_plain(u64::MAX)))));
    e.bytes(&pc);
    blob.extend_from_slice(code);
    out.0.extend_from_slice(&e.0);
    Some(())
}

// Skipping encoded identities without resolving them (enc_* / dec_* mirror these); `v` sees
// the crates and files they name (`key_crates`).

/// What an encoded identity names that a later run may not have: a crate (by its identity) or
/// a file version (enc_file's identity: path, text and owner).
#[derive(Clone, Copy)]
pub(super) enum Atom {
    Crate(u128),
    File(u128),
}

/// A visitor that ignores what a skip sees.
pub(super) fn no_atoms(_: Atom) {}

fn skip_def(d: &mut D, v: &mut dyn FnMut(Atom)) -> Option<()> {
    v(Atom::Crate(d.u128()?));
    d.u128()?;
    Some(())
}

fn skip_file(d: &mut D, v: &mut dyn FnMut(Atom)) -> Option<()> {
    v(Atom::File(d.u128()?));
    Some(())
}

fn skip_tys(d: &mut D, depth: u32, v: &mut dyn FnMut(Atom)) -> Option<()> {
    let n = d.u32()?;
    for _ in 0..n {
        skip_ty(d, depth + 1, v)?;
    }
    Some(())
}

fn skip_region(d: &mut D, _v: &mut dyn FnMut(Atom)) -> Option<()> {
    match d.u8()? {
        0 => {}
        1 => {
            d.u32()?;
            d.u32()?;
        }
        _ => return None,
    }
    Some(())
}

fn skip_regions(d: &mut D, v: &mut dyn FnMut(Atom)) -> Option<()> {
    let n = d.u32()?;
    for _ in 0..n {
        skip_region(d, v)?;
    }
    Some(())
}

fn skip_const(d: &mut D, depth: u32, v: &mut dyn FnMut(Atom)) -> Option<()> {
    if depth > 64 {
        return None;
    }
    match d.u8()? {
        0 => {
            skip_ty(d, depth + 1, v)?;
            match d.u8()? {
                0 => {
                    d.u8()?;
                    d.u128()?;
                }
                1 => {
                    let n = d.u32()?;
                    for _ in 0..n {
                        skip_const(d, depth + 1, v)?;
                    }
                }
                _ => return None,
            }
        }
        1 => {
            d.u8()?;
            d.u8()?;
            skip_def(d, v)?;
            let n = d.u32()?;
            for _ in 0..n {
                match d.u8()? {
                    0 => skip_ty(d, depth + 1, v)?,
                    1 => skip_const(d, depth + 1, v)?,
                    2 => skip_region(d, v)?,
                    _ => return None,
                }
            }
        }
        2 => {
            d.u32()?;
        }
        3 => {
            d.u32()?;
            d.u32()?;
        }
        _ => return None,
    }
    Some(())
}

// Mirrors Cache::enc_ty tag for tag: a skip that stops short or runs long cuts the key bytes of
// every symbol and closure parent after it (the cache::tests::skip_matches_encoding test holds
// the two together).
fn skip_ty(d: &mut D, depth: u32, v: &mut dyn FnMut(Atom)) -> Option<()> {
    if depth > 64 {
        return None;
    }
    match d.u8()? {
        1 | 2 | 3 | 6 => {}
        4 | 5 => {
            d.u8()?;
        }
        7 => skip_tys(d, depth, v)?,
        8 => {
            skip_ty(d, depth + 1, v)?;
            skip_const(d, depth + 1, v)?;
        }
        9 => skip_ty(d, depth + 1, v)?,
        10 => {
            d.u8()?;
            skip_region(d, v)?;
            skip_ty(d, depth + 1, v)?;
        }
        11 => {
            d.u8()?;
            skip_ty(d, depth + 1, v)?;
        }
        12 | 14 => {
            skip_def(d, v)?;
            skip_regions(d, v)?;
            skip_tys(d, depth, v)?;
        }
        13 => {
            skip_tys(d, depth, v)?;
            skip_ty(d, depth + 1, v)?;
            d.u8()?;
            d.u8()?;
            d.u8()?;
            d.u32()?;
        }
        17 => {
            skip_file(d, v)?;
            d.u32()?;
            d.u64()?;
            skip_ty(d, depth + 1, v)?;
            skip_ty(d, depth + 1, v)?;
            skip_ty(d, depth + 1, v)?;
            skip_ty(d, depth + 1, v)?;
        }
        18 => {
            d.u8()?;
            d.u8()?;
            skip_def(d, v)?;
            skip_regions(d, v)?;
            skip_tys(d, depth, v)?;
        }
        19 => {
            skip_def(d, v)?;
            skip_regions(d, v)?;
            skip_region(d, v)?;
            d.u32()?;
            skip_tys(d, depth, v)?;
            let n = d.u32()?;
            for _ in 0..n {
                d.u32()?;
                skip_def(d, v)?;
                skip_ty(d, depth + 1, v)?;
            }
            let n = d.u32()?;
            for _ in 0..n {
                skip_def(d, v)?;
            }
        }
        20 => skip_const(d, depth + 1, v)?,
        27 => skip_def(d, v)?,
        _ => return None,
    }
    Some(())
}

pub(super) fn skip_fnkey(d: &mut D, depth: u32, v: &mut dyn FnMut(Atom)) -> Option<()> {
    if depth > 32 {
        return None;
    }
    match d.u8()? {
        0 => {
            skip_def(d, v)?;
            skip_tys(d, 0, v)
        }
        1 => {
            skip_file(d, v)?;
            d.u32()?;
            d.u64()?;
            skip_fnkey(d, depth + 1, v)
        }
        2 => {
            d.u8()?;
            skip_ty(d, 0, v)
        }
        3 => {
            let tag = d.u8()?;
            skip_def(d, v)?;
            match tag {
                0 => {}
                1 => {
                    d.u8()?;
                }
                2 | 4 | 5 => skip_ty(d, 0, v)?,
                3 => {
                    skip_def(d, v)?;
                    d.u8()?;
                }
                _ => return None,
            }
            skip_tys(d, 0, v)
        }
        // an unverified-mode twin: its base's key
        4 => skip_fnkey(d, depth + 1, v),
        _ => None,
    }
}


/// A slot installed from the startup image: its code (linked, at `at`), unwind trailer, pc map
/// and, for tier-1 code, the slots its body inlined.
pub(crate) struct PrelinkedSlot {
    pub id: u32,
    pub at: u64,
    pub len: u32,
    /// its unwind trailer, read in place from the image (Unit::unwind_at)
    pub trailer: &'static [u8],
    pub pc: crate::jit::PcMap,
    pub inlined: Option<Vec<u32>>,
    /// O2 code (cache.rs TIER_O2): no O2 build for it this run
    pub o2: bool,
    /// baseline code (cache.rs TIER_BASE): built again at O2 after the first present
    pub base: bool,
    /// the kind of its key (its encoding's first byte: 0 an instance, 1 a closure, 2 glue), for
    /// what the install decides by it before the identity is decoded
    pub kind: u8,
}

/// One slot record of a loaded image (ranges into the flat arrays of `Img`).
struct ImgSlot {
    entry: u128,
    key: u32,
    tier: u8,
    inl: (usize, usize),
    crates: (usize, usize),
    /// its code entry's bytes in the blob (offset, length), and the length of its code without
    /// the unwind trailer that ends them
    code: (usize, usize),
    code_n: usize,
    /// byte ranges of its relocation records (REL_SIZE each) and its pc map (PcMap's bytes) in
    /// the image, read in place
    rel: (usize, usize),
    pc: (usize, usize),
}

/// An image relocation record: offset u32, kind u8, target u8, index u32.
const REL_SIZE: usize = 10;
/// No slot for an image key (not made: no installed slot names it; or its key did not decode).
const NO_SLOT: u32 = u32::MAX;

/// A loaded startup image, read in place, and what this run resolved of it so far.
struct Img {
    crate_ok: Vec<bool>,
    /// per key: every crate it names is unchanged, so it decodes (the others are decoded before
    /// any slot is linked to them: `prelink`)
    key_ok: Vec<bool>,
    keys: Vec<&'static [u8]>,
    /// the key indices in the order of their bytes (u32 each)
    sorted: &'static [u8],
    syms: Vec<&'static [u8]>,
    files: Vec<&'static [u8]>,
    slots: Vec<ImgSlot>,
    flat: Vec<u32>,
    /// keys of the functions thunk symbols name (their slots are made with the others)
    thunks: Vec<u32>,
    /// the image (relocation records and pc entries are read from it in place)
    bytes: &'static [u8],
    blob: &'static [u8],
    /// per key: its slot in this run (NO_SLOT: none)
    slot: Vec<u32>,
    /// per symbol: what it resolved to (img_sym, place_data)
    sym_sym: Vec<Option<Option<relax_backend::rir::Sym>>>,
    /// the data objects (place_data), and the object each symbol names (NO_SLOT: none)
    data: Vec<ImgData>,
    sym_obj: Vec<u32>,
    /// static symbols and the object of their cached initial value (placed with the data, so
    /// the static's initialization links it from there)
    static_obj: Vec<(u32, u32)>,
    /// the image's files in this run (img_files), and the store that keeps the image mapped
    pc_files: Option<std::sync::Arc<crate::jit::PcFiles>>,
    store: std::sync::Arc<Store>,
    /// per relocation (its record's position in the image): the writable block made for it (`img_sym`)
    fresh: HashMap<usize, relax_backend::rir::Sym>,
    /// slots placed (or found with code) so far
    seen: HashSet<u32>,
    changed: u64,
    failed: u64,
    had_code: u64,
    t_syms: u64,
    /// (trace) per symbol tag: resolve ns and count; place_data ns
    t_tag: [(u64, u32); 10],
    t_place: u64,
    /// the symbols part of the prelink time is measured (verbose: a clock read per symbol
    /// otherwise costs thousands of clock reads at every warm start)
    timed: bool,
}

/// Reads a startup image (`build_image`'s layout); `crate_ok(ident, fp)`: whether that crate is
/// in this program with that fingerprint.
fn parse_image(img: &'static [u8], store: std::sync::Arc<Store>, crate_ok: impl Fn(u128, u128) -> bool) -> Option<Img> {
    let mut d = D { b: img, p: 0 };
    let nc = d.u32()? as usize;
    let mut ok = Vec::with_capacity(nc.min(1 << 16));
    for _ in 0..nc {
        let ident = d.u128()?;
        let fp = d.u128()?;
        ok.push(crate_ok(ident, fp));
    }
    let mut tables: [Vec<&'static [u8]>; 3] = Default::default();
    for t in tables.iter_mut() {
        let n = d.u32()? as usize;
        t.reserve(n.min(1 << 20));
        for _ in 0..n {
            t.push(d.bytes()?);
        }
    }
    let nsorted = d.u32()? as usize;
    if nsorted != tables[0].len() {
        return None;
    }
    let sorted = d.take(nsorted.checked_mul(4)?)?;
    let nthunks = d.u32()? as usize;
    let mut thunks = Vec::with_capacity(nthunks.min(1 << 20));
    for _ in 0..nthunks {
        let k = d.u32()?;
        if k as usize >= tables[0].len() { return None; }
        thunks.push(k);
    }
    // per key: whether every crate it names is unchanged (else it may not decode in this run)
    let mut key_ok = Vec::with_capacity(tables[0].len());
    for _ in 0..tables[0].len() {
        let n = d.u32()?;
        let mut all = n != u32::MAX;
        for _ in 0..if all { n } else { 0 } {
            all &= ok.get(d.u32()? as usize) == Some(&true);
        }
        key_ok.push(all);
    }
    let nd = d.u32()? as usize;
    let mut data = Vec::with_capacity(nd.min(1 << 20));
    for _ in 0..nd {
        let key = d.u128()?;
        if d.u8()? == 0 {
            data.push(ImgData { key, ok: false, align: 1, name: None, bytes: &[], rel: (0, 0) });
            continue;
        }
        let align = d.u64()?;
        let name = match d.u8()? {
            0 => None,
            _ => Some(d.bytes()?),
        };
        let bytes = d.bytes()?;
        let nr = d.u32()? as usize;
        let rel = (d.p, d.p + nr.checked_mul(DREL_SIZE)?);
        d.take(rel.1 - rel.0)?;
        data.push(ImgData { key, ok: true, align, name, bytes, rel });
    }
    let mut sym_obj = vec![NO_SLOT; tables[1].len()];
    for _ in 0..d.u32()? {
        let s = d.u32()? as usize;
        let o = d.u32()?;
        if s >= sym_obj.len() || o as usize >= data.len() {
            return None;
        }
        sym_obj[s] = o;
    }
    let mut static_obj = Vec::new();
    for _ in 0..d.u32()? {
        let s = d.u32()?;
        let o = d.u32()?;
        if s as usize >= sym_obj.len() || o as usize >= data.len() {
            return None;
        }
        static_obj.push((s, o));
    }
    let ns = d.u32()? as usize;
    let mut slots = Vec::with_capacity(ns.min(1 << 20));
    let mut flat = Vec::new();
    for _ in 0..ns {
        let entry = d.u128()?;
        let key = d.u32()?;
        let tier = d.u8()?;
        let a = flat.len();
        for _ in 0..d.u32()? {
            flat.push(d.u32()?);
        }
        let inl = (a, flat.len());
        for _ in 0..d.u32()? {
            flat.push(d.u32()?);
        }
        let crates = (inl.1, flat.len());
        let off = d.u32()? as usize;
        let len = d.u32()? as usize;
        let code_n = d.u32()? as usize;
        if code_n > len {
            return None;
        }
        let nr = d.u32()? as usize;
        let rel = (d.p, d.p + nr.checked_mul(REL_SIZE)?);
        d.take(rel.1 - rel.0)?;
        let np = d.u32()? as usize;
        let pc = (d.p, d.p.checked_add(np)?);
        d.take(np)?;
        slots.push(ImgSlot { entry, key, tier, inl, crates, code: (off, len), code_n, rel, pc });
    }
    let blob = d.bytes()?;
    let [keys, syms, files] = tables;
    Some(Img {
        crate_ok: ok,
        key_ok,
        slot: vec![NO_SLOT; keys.len()],
        sym_sym: vec![None; syms.len()],
        data,
        sym_obj,
        static_obj,
        pc_files: None,
        store,
        keys,
        sorted,
        syms,
        files,
        slots,
        flat,
        thunks,
        bytes: img,
        blob,
        fresh: HashMap::new(),
        seen: HashSet::new(),
        changed: 0,
        failed: 0,
        had_code: 0,
        t_syms: 0,
        t_tag: [(0, 0); 10],
        t_place: 0,
        timed: false,
    })
}

/// A data object of a loaded image (bytes and relocation records read in place).
struct ImgData {
    key: u128,
    ok: bool,
    align: u64,
    name: Option<&'static [u8]>,
    bytes: &'static [u8],
    rel: (usize, usize),
}

/// The index of the key encoded as `b` in an image's key table (`sorted`: the key indices in
/// the order of their bytes).
fn key_index(keys: &[&'static [u8]], sorted: &[u8], b: &[u8]) -> Option<usize> {
    let at = |i: usize| u32::from_le_bytes([sorted[i * 4], sorted[i * 4 + 1], sorted[i * 4 + 2], sorted[i * 4 + 3]]) as usize;
    let (mut lo, mut hi) = (0, sorted.len() / 4);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let k = at(mid);
        match (*keys.get(k)?).cmp(b) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
            std::cmp::Ordering::Equal => return Some(k),
        }
    }
    None
}

impl Img {
    /// The relocation record at image position `at` (offset, kind, target, index).
    fn rel_at(&self, at: usize) -> Option<(u32, RelocKind, u8, u32)> {
        let b = self.bytes.get(at..at + REL_SIZE)?;
        let u = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Some((u(0), kind_of(b[4])?, b[5], u(6)))
    }

    /// The data relocation record at image position `at` (offset, target, index, addend).
    fn drel_at(&self, at: usize) -> (u32, u8, u32, u64) {
        let b = &self.bytes[at..at + DREL_SIZE];
        let u = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        (u(0), b[4], u(5), u64::from_le_bytes(b[9..17].try_into().unwrap()))
    }

    /// The slot image key `k` names in this run.
    fn slot_of(&self, k: u32) -> Option<u32> {
        self.slot.get(k as usize).copied().filter(|&s| s != NO_SLOT)
    }
}

/// The slots the prelinked startup image's keys name in this run (kept for the run; the store
/// keeps the image mapped): found by their encoded bytes, and the ones whose identity is still
/// encoded, decoded in image order by `complete_identities`.
pub(super) struct ImageIds {
    _store: std::sync::Arc<Store>,
    keys: Vec<&'static [u8]>,
    sorted: &'static [u8],
    /// per key: its slot (NO_SLOT: none, or its key did not decode)
    slot: Vec<u32>,
    /// per key: it decodes for sure (Img::key_ok); the others the prelink decoded already
    key_ok: Vec<bool>,
    /// the keys whose slots the prelink made, in image order (slots `first..`); the first `done`
    /// are decoded (later ones may be too: a lookup by key or a build decodes its slot's first)
    pending: Vec<u32>,
    first: u32,
    done: usize,
}

impl ImageIds {
    /// The slot of the image key encoded as `b`.
    pub(super) fn slot_of(&self, b: &[u8]) -> Option<u32> {
        let k = key_index(&self.keys, self.sorted, b)?;
        Some(self.slot[k]).filter(|&s| s != NO_SLOT)
    }

    /// The key slot `id` was made from, if the prelink made it (and the key decoded, or is
    /// still encoded).
    pub(super) fn bytes_of(&self, id: u32) -> Option<&'static [u8]> {
        let k = *self.pending.get(id.checked_sub(self.first)? as usize)? as usize;
        (self.slot[k] == id).then(|| self.keys[k])
    }

    /// Identities still encoded.
    pub(super) fn pending(&self) -> bool {
        self.done < self.pending.len()
    }
}

/// The store a startup image is mapped from, kept by the pc maps read from it.
struct KeepMapped(#[allow(dead_code)] std::sync::Arc<Store>);
// the store is only kept open (its mapping read-only, never written), not used
unsafe impl Send for KeepMapped {}
unsafe impl Sync for KeepMapped {}

/// How a slot of the image resolved: its slot (all identities resolved, or all but the statics
/// it names), or why it stays lazy.
enum Resolved { Ready(u32), NeedsStatics(u32), Lazy }

/// Static symbols (`enc_sym` tag 3): initializing one may run code.
fn is_static_sym(b: &[u8]) -> bool {
    b.first() == Some(&3)
}

impl Linker {
    /// The image's files in this run, for the pc maps read in place (made on first use).
    fn img_files(&mut self, u: &mut Unit, im: &mut Img) -> std::sync::Arc<crate::jit::PcFiles> {
        if let Some(f) = &im.pc_files {
            return f.clone();
        }
        let files = (0..im.files.len()).map(|ix| self.cache().dec_file(u, &mut D { b: im.files[ix], p: 0 }).unwrap_or(u32::MAX)).collect();
        // the pc maps of installed slots hold it: identity state, in the kept heap
        let files: Vec<u32> = files;
        let f = crate::zone_alloc::kept_with(|| std::sync::Arc::new(crate::jit::PcFiles { files: files.clone(), keep: Box::new(KeepMapped(im.store.clone())) }));
        im.pc_files = Some(f.clone());
        f
    }

    /// Resolves image symbol `ix` for relocation `ri` (memo; a promoted temporary's writable block
    /// -- tag 8 -- is made once per ordinal of `rw`, the blocks of the slot (or data object) being
    /// resolved, as link_entry does: never shared between slots).
    fn img_sym(&mut self, u: &mut Unit, im: &mut Img, ix: u32, ri: usize, rw: &mut Vec<relax_backend::rir::Sym>) -> Option<relax_backend::rir::Sym> {
        let ix = ix as usize;
        let &b = im.syms.get(ix)?;
        if b.first() != Some(&8) {
            if let Some(r) = im.sym_sym[ix] {
                return r;
            }
        }
        let t = if im.timed { crate::jit::wall_ns() } else { 0 };
        let r = self.link_symbol(u, &mut D { b, p: 0 }, rw);
        if im.timed {
            let e = &mut im.t_tag[(b.first().copied().unwrap_or(0) as usize).min(9)];
            e.0 += crate::jit::wall_ns() - t; e.1 += 1;
        }
        if b.first() == Some(&8) {
            if let Some(s) = r {
                im.fresh.insert(ri, s);
            }
        } else {
            im.sym_sym[ix] = Some(r);
        }
        if im.timed { im.t_syms += crate::jit::wall_ns() - t; }
        r
    }

    /// Places the image's data objects that the placeable slots' symbols reach as one block of
    /// data memory: their bytes copied in, their pointer words linked (to each other, and to
    /// symbols resolved once: img_sym; a static's initializer may run here, as link_data's
    /// would), registered in one go, and remembered as linked (`data_addr`: a later link of the
    /// same entry takes them). An object one of whose words does not resolve stays out, with the
    /// objects that point to it: their symbols link through link_data as before.
    fn place_data(&mut self, u: &mut Unit, im: &mut Img, need_sym: &[bool]) {
        let n = im.data.len();
        if n == 0 {
            return;
        }
        let mut reach = vec![false; n];
        let mut stack = Vec::new();
        let roots = im.sym_obj.iter().enumerate().filter(|&(_, &o)| o != NO_SLOT).map(|(s, &o)| (s, o)).chain(im.static_obj.iter().map(|&(s, o)| (s as usize, o)));
        for (s, o) in roots {
            if need_sym.get(s) == Some(&true) && !reach[o as usize] {
                reach[o as usize] = true;
                stack.push(o as usize);
            }
        }
        // (a static an object points to: the object of its initial value too)
        let mut static_of = vec![NO_SLOT; im.syms.len()];
        for &(s, o) in &im.static_obj {
            static_of[s as usize] = o;
        }
        while let Some(o) = stack.pop() {
            if !im.data[o].ok {
                continue;
            }
            for ri in (im.data[o].rel.0..im.data[o].rel.1).step_by(DREL_SIZE) {
                let (_, t, x, _) = im.drel_at(ri);
                let x = if t == DATA_OBJ { x } else { static_of.get(x as usize).copied().unwrap_or(NO_SLOT) };
                if (x as usize) < n && !reach[x as usize] {
                    reach[x as usize] = true;
                    stack.push(x as usize);
                }
            }
        }
        // addresses: an object linked already this run keeps its own; the rest in one block
        let mut addr = vec![0u64; n];
        let mut layout = Vec::new();
        let (mut total, mut max_align) = (0u64, 1u64);
        for o in 0..n {
            if !reach[o] || !im.data[o].ok {
                continue;
            }
            if let Some(&a) = self.cache().data_addr.get(&im.data[o].key) {
                addr[o] = a;
                continue;
            }
            let al = im.data[o].align.max(1);
            let off = (total + al - 1) / al * al;
            layout.push((o, off));
            // one byte more than the object (Unit::data): no object ends where the next begins
            total = off + im.data[o].bytes.len() as u64 + 1;
            max_align = max_align.max(al);
        }
        if layout.is_empty() {
            return;
        }
        let base = u.data_block(total, max_align);
        for &(o, off) in &layout {
            addr[o] = base + off;
            let b = im.data[o].bytes;
            unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), (base + off) as *mut u8, b.len()) };
        }
        // pointer words: other objects by address now, symbols resolved
        let mut ok = vec![false; n];
        let mut words: Vec<Vec<(u32, u8, u32, i64, Option<relax_backend::rir::Sym>)>> = Vec::with_capacity(layout.len());
        for &(o, _) in &layout {
            let mut w = Vec::with_capacity((im.data[o].rel.1 - im.data[o].rel.0) / DREL_SIZE);
            let mut good = true;
            let mut rw = Vec::new();
            for ri in (im.data[o].rel.0..im.data[o].rel.1).step_by(DREL_SIZE) {
                let (off, t, x, add) = im.drel_at(ri);
                if t == DATA_OBJ {
                    if (x as usize) >= n || addr[x as usize] == 0 {
                        good = false;
                        break;
                    }
                    w.push((off, t, x, add as i64, None));
                } else {
                    match self.img_sym(u, im, x, ri, &mut rw) {
                        Some(s) => w.push((off, t, x, 0, Some(s))),
                        None => {
                            good = false;
                            break;
                        }
                    }
                }
            }
            ok[o] = good;
            words.push(w);
        }
        // (objects linked earlier this run are as good as placed)
        let mut in_block = vec![false; n];
        for &(o, _) in &layout {
            in_block[o] = true;
        }
        for o in 0..n {
            if reach[o] && im.data[o].ok && addr[o] != 0 && !in_block[o] {
                ok[o] = true;
            }
        }
        // an object pointing to one that stays out stays out
        loop {
            let mut changed = false;
            for (k, &(o, _)) in layout.iter().enumerate() {
                if ok[o] && words[k].iter().any(|w| w.1 == DATA_OBJ && !ok[w.2 as usize]) {
                    ok[o] = false;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // registered together; a pointer word to another object of the block names its symbol
        let placed: Vec<usize> = (0..layout.len()).filter(|&k| ok[layout[k].0]).collect();
        let mut batch_of = vec![u32::MAX; n];
        for (i, &k) in placed.iter().enumerate() {
            batch_of[layout[k].0] = i as u32;
        }
        let objs: Vec<(u64, u64, u64, Option<String>)> = placed.iter().map(|&k| {
            let o = layout[k].0;
            let d = &im.data[o];
            (addr[o], d.bytes.len() as u64, d.align.max(1), d.name.map(|b| String::from_utf8_lossy(b).into_owned()))
        }).collect();
        let outside: HashMap<usize, relax_backend::rir::Sym> = placed.iter().flat_map(|&k| words[k].iter()).filter(|w| w.1 == DATA_OBJ && batch_of[w.2 as usize] == u32::MAX)
            .filter_map(|w| Some((w.2 as usize, u.syms.data_at(addr[w.2 as usize])?))).collect();
        let syms = u.syms.add_data_batch(&objs, |i, syms| {
            let k = placed[i];
            let o = layout[k].0;
            words[k].iter().filter_map(|&(off, t, x, add, s)| {
                let s = if t == DATA_OBJ {
                    let b = batch_of[x as usize];
                    let base = if b != u32::MAX { syms[b as usize] } else { *outside.get(&(x as usize))? };
                    relax_backend::rir::Sym { id: base.id, off: add }
                } else {
                    s?
                };
                let v = u.syms.addr(s);
                unsafe { ((addr[o] + off as u64) as *mut u64).write_unaligned(v) };
                Some((off, s))
            }).collect()
        });
        let c = self.cache();
        c.data_addr.reserve(placed.len());
        for &k in &placed {
            let o = layout[k].0;
            c.data_addr.insert(im.data[o].key, addr[o]);
        }
        // the symbols that name them
        for s in 0..im.sym_obj.len() {
            let o = im.sym_obj[s];
            if o == NO_SLOT || !ok[o as usize] || im.sym_sym[s].is_some() {
                continue;
            }
            let b = im.syms[s];
            let off = b.get(17..25).map_or(0, |x| u64::from_le_bytes(x.try_into().unwrap())) as i64;
            let sym = match batch_of[o as usize] {
                u32::MAX => u.syms.data_at(addr[o as usize]),
                i => Some(syms[i as usize]),
            };
            if let Some(sym) = sym {
                im.sym_sym[s] = Some(Some(relax_backend::rir::Sym { id: sym.id, off }));
            }
        }
        self.cache().stats.prelink_data += placed.len() as u64;
    }

    /// Whether image slot `si` can be placed (its crates unchanged, its code entry indexed):
    /// the keys it names are then marked in `need`.
    fn img_live(&mut self, im: &Img, si: usize, need: &mut [bool], need_sym: &mut [bool]) -> bool {
        let s = &im.slots[si];
        let in_range = s.code.0.checked_add(s.code.1).map_or(false, |e| e <= im.blob.len());
        if !in_range || !im.flat[s.crates.0..s.crates.1].iter().all(|&c| im.crate_ok.get(c as usize) == Some(&true)) || !self.cache().has_index(&s.entry) {
            return false;
        }
        // an O2/O3 build of the slot replaced its entry after this image was made: the slot is
        // left lazy, the first call links the entry (O2 code, marked: no build of it again).
        // Prelinking the image's lower-tier code would run it and build the entry's code again
        // on every launch (the image is made again past 2% of such slots: record_manifest).
        if matches!(s.tier, TIER_O1 | TIER_BASE) && self.cache().entry_tier(s.entry).is_some_and(|t| t == TIER_O2 || t == TIER_O3) {
            return false;
        }
        let nk = need.len();
        let mut mark = |k: u32| (k as usize) < nk && { need[k as usize] = true; true };
        if !mark(s.key) || !im.flat[s.inl.0..s.inl.1].iter().all(|&k| mark(k)) {
            return false;
        }
        for ri in (s.rel.0..s.rel.1).step_by(REL_SIZE) {
            match im.rel_at(ri) {
                Some((_, _, IMG_SLOT | IMG_THUNK | IMG_CALL, ix)) => {
                    if !mark(ix) {
                        return false;
                    }
                }
                Some((_, _, IMG_SYM, ix)) => {
                    if let Some(x) = need_sym.get_mut(ix as usize) {
                        *x = true;
                    }
                }
                Some(_) => {}
                None => return false,
            }
        }
        true
    }

    /// Resolves the symbols of image slot `si` (its keys have slots by now); `statics`: also the
    /// statics it names (their initializers may run code).
    fn img_resolve(&mut self, u: &mut Unit, im: &mut Img, si: usize, statics: bool) -> Resolved {
        let (key, inl, rel) = {
            let s = &im.slots[si];
            (s.key, s.inl, s.rel)
        };
        let Some(id) = im.slot_of(key) else {
            im.failed += 1;
            return Resolved::Lazy;
        };
        if u.fns[id as usize].compiled {
            im.had_code += 1;
            return Resolved::Lazy;
        }
        if im.flat[inl.0..inl.1].iter().any(|&k| im.slot_of(k).is_none()) {
            im.failed += 1;
            return Resolved::Lazy;
        }
        let mut needs = false;
        let mut rw = Vec::new();
        for ri in (rel.0..rel.1).step_by(REL_SIZE) {
            let Some((_, _, t, ix)) = im.rel_at(ri) else {
                im.failed += 1;
                return Resolved::Lazy;
            };
            let ok = match t {
                IMG_SLOT | IMG_THUNK | IMG_CALL => im.slot_of(ix).is_some(),
                IMG_SYM if !statics && im.syms.get(ix as usize).map_or(false, |b| is_static_sym(b)) => {
                    needs = true;
                    true
                }
                IMG_SYM => self.img_sym(u, im, ix, ri, &mut rw).is_some(),
                IMG_TLS_KEY..=IMG_RESUME => true,
                _ => false,
            };
            if !ok {
                im.failed += 1;
                return Resolved::Lazy;
            }
        }
        // its identity, for this run's manifest: the exit names a slot only by bytes already
        // encoded (slot_bytes), and a prelinked slot is never looked up
        let bytes = im.keys[key as usize];
        self.cache().fnkey_bytes.entry(id).or_insert_with(|| Some(bytes.to_vec()));
        if needs { Resolved::NeedsStatics(id) } else { Resolved::Ready(id) }
    }

    /// Places the code of resolved slots `keep` (that still have no code) in one block of code
    /// memory, each 16-byte aligned without its unwind trailer, links it there in one pass and
    /// writes the block with one write.
    fn img_place(&mut self, u: &mut Unit, im: &mut Img, keep: Vec<(usize, u32)>) -> Vec<PrelinkedSlot> {
        let mut out = Vec::new();
        let mut layout = Vec::with_capacity(keep.len());
        let mut total = 0usize;
        for (si, id) in keep {
            if u.fns[id as usize].compiled || !im.seen.insert(id) {
                im.had_code += 1;
                continue;
            }
            let s = &im.slots[si];
            let n = s.code_n;
            let off = (total + 15) & !15;
            layout.push((si, id, off, n, &im.blob[s.code.0 + n..s.code.0 + s.code.1]));
            total = off + n;
        }
        if total == 0 {
            return out;
        }
        let at = u.code_block(total as u64);
        // copied and linked in place: one pass over the block
        let mut linked = vec![false; layout.len()];
        {
            let env = &u.env;
            let syms = &u.syms;
            let im = &*im;
            unsafe {
                crate::os::mem::fill_code(at, total, |buf| {
                    let mut rl: Vec<RelocAt> = Vec::new();
                    for (k, &(si, _, off, n, _)) in layout.iter().enumerate() {
                        let s = &im.slots[si];
                        buf[off..off + n].copy_from_slice(&im.blob[s.code.0..s.code.0 + n]);
                        rl.clear();
                        let mut ok = true;
                        for ri in (s.rel.0..s.rel.1).step_by(REL_SIZE) {
                            let Some((r_at, kind, t, ix)) = im.rel_at(ri) else {
                                ok = false;
                                break;
                            };
                            let v = match t {
                                IMG_SLOT | IMG_THUNK | IMG_CALL => im.slot_of(ix).map(|x| env.reloc_value(match t { IMG_SLOT => Reloc::Slot(x), IMG_CALL => Reloc::Call(x), _ => Reloc::Thunk(x) })),
                                IMG_SYM => im.fresh.get(&ri).copied().or_else(|| im.sym_sym.get(ix as usize).copied().flatten().flatten()).map(|s| syms.value(s)),
                                IMG_TLS_KEY => Some(env.reloc_value(Reloc::TlsKey)),
                                IMG_TLS_SLOW => Some(env.reloc_value(Reloc::TlsSlow)),
                                _ => Some(env.reloc_value(Reloc::Resume)),
                            };
                            match v {
                                Some(v) if (r_at as usize) < n => rl.push(RelocAt { at: r_at, kind, target: Reloc::Value(v) }),
                                _ => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        linked[k] = ok && u.link_code(&mut buf[off..off + n], at + off as u64, &rl).is_ok();
                    }
                });
            }
        }
        {
            let c = self.cache();
            c.linked.reserve(layout.len());
            c.linked_key.reserve(layout.len());
        }
        for ((si, id, off, n, trailer), ok) in layout.into_iter().zip(linked) {
            if !ok {
                // its bytes stay in the block, unreachable: no table word points there
                im.failed += 1;
                continue;
            }
            let files = self.img_files(u, im);
            let s = &im.slots[si];
            let (pcs, tier, inl, entry, key) = (s.pc, s.tier, s.inl, s.entry, s.key);
            let pc = crate::jit::PcMap::mapped(&im.bytes[pcs.0..pcs.1], files);
            let inlined = (tier == TIER_O3).then(|| im.flat[inl.0..inl.1].iter().filter_map(|&k| im.slot_of(k)).collect());
            let kind = im.keys[key as usize].first().copied().unwrap_or(u8::MAX);
            let c = self.cache();
            c.linked.insert(id);
            c.linked_key.insert(id, entry);
            out.push(PrelinkedSlot { id, at: at + off as u64, len: n as u32, trailer, pc, inlined, o2: tier == TIER_O2, base: tier == TIER_BASE, kind });
        }
        out
    }

    /// Prelinks the startup image of the loaded manifest: makes the slots its keys name in one
    /// go (identities still encoded: `complete_identities`; decoded here already when `eager`),
    /// resolves its symbols in this run (each once), places the code of every slot whose crates
    /// are unchanged, whose identities resolve and that has no code yet, and hands those slots to
    /// `install` (jit.rs) -- in two blocks: the slots that name no static first, then, once the
    /// statics are initialized (their initializers run code: it finds the first block
    /// installed), the rest. Returns the slots installed (on the owner thread).
    fn prelink(&mut self, u: &mut Unit, eager: bool, install: &mut dyn FnMut(&mut Unit, Vec<PrelinkedSlot>)) -> usize {
        let t0 = crate::jit::wall_ns();
        let Some((img, store)) = self.cache().image.as_ref().map(|i| (i.bytes(), i._store.clone())) else { return 0 };
        let parsed = {
            let c = self.cache();
            parse_image(img, store.clone(), |ident, fp| c.crate_by_ident.get(&ident).map_or(false, |&k| c.fps.get(k as usize) == Some(&fp)))
        };
        let Some(mut im) = parsed else {
            self.cache().image_rewrite = true;
            return 0;
        };
        im.timed = self.cache().verbose;
        let t_parsed = crate::jit::wall_ns();
        // the slots that can be placed, and the keys they name
        let nk = im.keys.len();
        let mut need = vec![false; nk];
        let mut need_sym = vec![false; im.syms.len()];
        let live: Vec<bool> = (0..im.slots.len()).map(|si| self.img_live(&im, si, &mut need, &mut need_sym)).collect();
        im.changed += live.iter().filter(|&&l| !l).count() as u64;
        for &k in &im.thunks {
            need[k as usize] = true;
        }
        // keys this run has a slot for already (made before the prelink), found by their encoding
        for id in 0..u.fns.len() as u32 {
            if let Some(b) = self.cache().slot_bytes(u, id) {
                if let Some(k) = key_index(&im.keys, im.sorted, &b) {
                    im.slot[k] = id;
                }
            }
        }
        // the slots of the others, made in one go in image order
        let fresh: Vec<u32> = (0..nk as u32).filter(|&k| need[k as usize] && im.slot[k as usize] == NO_SLOT).collect();
        let first = u.new_slots(fresh.len());
        for (i, &k) in fresh.iter().enumerate() {
            im.slot[k as usize] = first + i as u32;
        }
        self.cache().image_ids = Some(ImageIds { _store: store, keys: im.keys.clone(), sorted: im.sorted, slot: im.slot.clone(), key_ok: im.key_ok.clone(), pending: fresh, first, done: 0 });
        if eager {
            // every identity now: a key that does not decode leaves the slots naming it lazy
            self.complete_identities(u, usize::MAX, false);
        } else {
            // a key naming a crate changed since the image was made may not decode (an edit
            // shifted a closure's expression or an item's offset, removed a definition): decoded
            // now, before any slot is linked to its slot. One that does not decode has no slot:
            // the slots and data naming it stay lazy and build from source, so no code reaches a
            // slot without an identity. The rest decode for sure, after main started (a build
            // or a lookup by key before -- a static's initializer below -- decodes them first).
            self.decode_unsure(u);
            if self.cache().image_ids.as_ref().map_or(false, |i| i.pending()) {
                crate::jit::ids_pending(u);
            }
        }
        if let Some(ids) = self.cache().image_ids.as_ref() {
            im.slot.copy_from_slice(&ids.slot);
        }
        let t_keys = crate::jit::wall_ns();
        let tagged = im.t_tag.iter().map(|e| e.0).sum::<u64>();
        self.place_data(u, &mut im, &need_sym);
        if im.timed {
            im.t_place = crate::jit::wall_ns() - t_keys;
            let inner = im.t_tag.iter().map(|e| e.0).sum::<u64>() - tagged;
            im.t_syms += im.t_place.saturating_sub(inner);
        }
        let (mut ready, mut later) = (Vec::with_capacity(im.slots.len()), Vec::new());
        for si in 0..im.slots.len() {
            if !live[si] {
                continue;
            }
            match self.img_resolve(u, &mut im, si, false) {
                Resolved::Ready(id) => ready.push((si, id)),
                Resolved::NeedsStatics(id) => later.push((si, id)),
                Resolved::Lazy => {}
            }
        }
        let first = self.img_place(u, &mut im, ready);
        let mut n = first.len();
        install(u, first);
        let mut ready = Vec::with_capacity(later.len());
        for (si, id) in later {
            match self.img_resolve(u, &mut im, si, true) {
                Resolved::Ready(id2) | Resolved::NeedsStatics(id2) if id2 == id => ready.push((si, id)),
                _ => {}
            }
        }
        let second = self.img_place(u, &mut im, ready);
        n += second.len();
        install(u, second);
        let c = self.cache();
        c.stats.prelink_parse_ns += t_parsed - t0;
        c.stats.prelink_keys_ns += t_keys - t_parsed;
        c.stats.prelink_syms_ns += im.t_syms;
        c.stats.prelinked += n as u64;
        c.stats.prelink_changed += im.changed;
        c.stats.prelink_failed += im.failed;
        c.stats.prelink_had_code += im.had_code;
        c.stats.prelink_ns += crate::jit::wall_ns() - t0;
        if im.timed && std::env::var_os("MAKEPAD_RUN_BOOT_TRACE").is_some() {
            let tags: Vec<String> = im.t_tag.iter().enumerate().filter(|(_, e)| e.1 != 0).map(|(k, e)| format!("tag{}={}/{:.1}ms", k, e.1, e.0 as f64 / 1e6)).collect();
            eprintln!("makepad-run cache: prelink symbols: place_data {:.1} ms (incl. its symbols), {} symbols in the image; resolved by tag: {}; thunks decoded (not image keys) {} in {:.1} ms",
                im.t_place as f64 / 1e6, im.syms.len(), tags.join(" "), super::THUNK_DECODED.load(std::sync::atomic::Ordering::Relaxed), super::THUNK_DECODE_NS.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6);
        }
        // what decides whether the next manifest carries a fresh image (record_manifest)
        c.image_slots = im.slots.len().max(1);
        c.image_stale += (im.changed + im.failed) as usize;
        n
    }

    /// The slot of the fn key encoded at `d` when it is a key of the startup image (`d` then
    /// moves past it): found by its bytes, decoded or not.
    /// A key that may not decode (`ImageIds::key_ok`) is decoded first (only while the prelink
    /// decodes those: its slot is never handed out undecoded); None when it does not decode.
    pub(super) fn image_slot_at(&mut self, u: &mut Unit, d: &mut D) -> Option<u32> {
        let ids = self.cache().image_ids.as_ref()?;
        let mut e = D { b: d.b, p: d.p };
        skip_fnkey(&mut e, 0, &mut no_atoms)?;
        let k = key_index(&ids.keys, ids.sorted, &d.b[d.p..e.p])?;
        let s = Some(ids.slot[k]).filter(|&s| s != NO_SLOT)?;
        if !ids.key_ok[k] && is_unnamed(u, s) {
            self.name_unnamed(u, s);
            self.cache().image_ids.as_ref()?.slot_of(&d.b[d.p..e.p])?;
        }
        d.p = e.p;
        Some(s)
    }

    /// Decodes the identities of the startup image's slots whose keys name a crate changed since
    /// the image was made, or one it did not know (`prelink`).
    fn decode_unsure(&mut self, u: &mut Unit) {
        let t0 = crate::jit::wall_ns();
        let Some(ids) = self.cache().image_ids.as_ref() else { return };
        let unsure: Vec<u32> = ids.pending.iter().enumerate().filter(|&(_, &k)| !ids.key_ok[k as usize]).map(|(i, _)| ids.first + i as u32).collect();
        if unsure.is_empty() {
            return;
        }
        for &id in &unsure {
            self.name_unnamed(u, id);
        }
        let c = self.cache();
        c.stats.ids_unsure += unsure.len() as u64;
        c.stats.ids_unsure_ns += crate::jit::wall_ns() - t0;
    }

    /// Decodes up to `budget` of the startup image slots' identities still encoded, in image
    /// order (on the owner thread); true when none is left. A key that does not decode leaves its
    /// slot unnamed (no lookup finds it); a key an earlier slot has already leaves the image's
    /// slot a second slot of that function (it keeps its code; the map names the first).
    fn complete_identities(&mut self, u: &mut Unit, budget: usize, bg: bool) -> bool {
        let t0 = crate::jit::wall_ns();
        let mut left = budget;
        let (mut dup, mut failed, mut n) = (0, 0, 0);
        let done = loop {
            let (k, id, b) = {
                let Some(ids) = self.cache().image_ids.as_mut() else { break true };
                if ids.done == ids.pending.len() {
                    break true;
                }
                if left == 0 {
                    break false;
                }
                let k = ids.pending[ids.done] as usize;
                let id = ids.first + ids.done as u32;
                ids.done += 1;
                // (a key that did not decode already has no slot)
                if ids.slot[k] != id {
                    continue;
                }
                (k, id, ids.keys[k])
            };
            if !is_unnamed(u, id) {
                continue;
            }
            left -= 1;
            n += 1;
            match self.name_image_slot(u, k, id, b) {
                Some(true) => {}
                Some(false) => dup += 1,
                None => failed += 1,
            }
        };
        let c = self.cache();
        c.stats.ids_decoded += n;
        if bg {
            c.stats.ids_bg += n;
        }
        c.stats.ids_dup += dup;
        c.stats.ids_failed += failed;
        c.stats.ids_ns += crate::jit::wall_ns() - t0;
        done
    }
}

/// Whether slot `id` is a startup image slot whose identity is still encoded.
fn is_unnamed(u: &Unit, id: u32) -> bool {
    matches!(u.fns[id as usize].key, crate::jit::FnKey::Glue(crate::jit::GLUE_UNNAMED, _))
}

impl Linker {
    /// Decodes image key `k` (bytes `b`) and names its slot `id` with it: Some(false) when the
    /// key was another slot's already (name_slot), None when it does not decode (no lookup
    /// finds the slot then).
    fn name_image_slot(&mut self, u: &mut Unit, k: usize, id: u32, b: &'static [u8]) -> Option<bool> {
        match self.dec_fnkey(u, &mut D { b, p: 0 }, 0) {
            Some(key) => Some(u.name_slot(id, key)),
            None => {
                if let Some(ids) = self.cache().image_ids.as_mut() {
                    ids.slot[k] = NO_SLOT;
                }
                None
            }
        }
    }

    /// Slot `id`'s identity, if it is a startup image slot whose key is still encoded: decoded
    /// now (before it is built). On the owner thread.
    fn name_unnamed(&mut self, u: &mut Unit, id: u32) {
        if !is_unnamed(u, id) {
            return;
        }
        let Some((k, b)) = self.cache().image_ids.as_ref().and_then(|ids| {
            let i = id.checked_sub(ids.first)? as usize;
            let k = *ids.pending.get(i)? as usize;
            // (a key that did not decode is not tried again)
            (ids.slot[k] == id).then(|| (k, ids.keys[k]))
        }) else { return };
        let t0 = crate::jit::wall_ns();
        let r = self.name_image_slot(u, k, id, b);
        let c = self.cache();
        c.stats.ids_decoded += 1;
        c.stats.ids_dup += (r == Some(false)) as u64;
        c.stats.ids_failed += r.is_none() as u64;
        c.stats.ids_ns += crate::jit::wall_ns() - t0;
    }

    /// The startup image slot keyed `key` while image identities are still encoded (a lookup by
    /// key that the map did not answer): found by the key's encoding, its identity decoded now.
    fn image_slot_by_key(&mut self, u: &mut Unit, key: &crate::jit::FnKey) -> Option<u32> {
        // a closure's key encodes its parents' keys: those named first
        let parent = |u: &Unit, k: &crate::jit::FnKey| match k {
            crate::jit::FnKey::Closure(_, _, p) | crate::jit::FnKey::Plain(p) if (*p as usize) < u.fns.len() => Some(*p),
            _ => None,
        };
        let mut p = parent(u, key);
        while let Some(id) = p {
            self.name_unnamed(u, id);
            p = parent(u, &u.fns[id as usize].key);
        }
        let b = {
            let c = self.cache();
            let mut e = E::default();
            let mut cr = Vec::new();
            let outer = std::mem::replace(&mut c.key_waits, false);
            let r = c.enc_fnkey(u, key, &mut e, &mut cr, 0);
            c.key_waits = outer;
            r?;
            e.0
        };
        let id = self.cache().image_ids.as_ref()?.slot_of(&b)?;
        self.name_unnamed(u, id);
        // (a key that did not decode: no slot)
        self.cache().image_ids.as_ref()?.slot_of(&b)
    }
}

/// Code reached startup image slot `id`, whose key did not decode (jit.rs): see `image_broken`.
pub(crate) unsafe fn image_broken_unit(up: *mut Unit, id: u32) {
    let Some(cache) = (&*up).cache.as_ref().map(|c| c.get()) else { return };
    (&mut *cache).image_broken(&format!("code reached slot {} without an identity", id));
}

/// See `Linker::name_unnamed` (on the owner thread).
pub(crate) unsafe fn name_unnamed_unit(up: *mut Unit, id: u32) {
    let Some(cache) = (&*up).cache.as_ref().map(|c| c.get()) else { return };
    Linker::new(cache).name_unnamed(&mut *up, id)
}

/// See `Linker::image_slot_by_key` (on the owner thread).
pub(crate) unsafe fn image_slot_by_key_unit(up: *mut Unit, key: &crate::jit::FnKey) -> Option<u32> {
    let Some(cache) = (&*up).cache.as_ref().map(|c| c.get()) else { return None };
    Linker::new(cache).image_slot_by_key(&mut *up, key)
}

/// Prelinks the startup image (see `Linker::prelink`); the caller is on the owner thread.
pub(crate) unsafe fn prelink_unit(up: *mut Unit, eager: bool, install: &mut dyn FnMut(&mut Unit, Vec<PrelinkedSlot>)) -> usize {
    let Some(cache) = (&*up).cache.as_ref().map(|c| c.get()) else { return 0 };
    Linker::new(cache).prelink(&mut *up, eager, install)
}

/// Decodes up to `budget` of the startup image's identities still encoded (see
/// `Linker::complete_identities`); true when none is left. On the owner thread.
pub(crate) unsafe fn complete_identities_unit(up: *mut Unit, budget: usize, bg: bool) -> bool {
    let Some(cache) = (&*up).cache.as_ref().map(|c| c.get()) else { return true };
    Linker::new(cache).complete_identities(&mut *up, budget, bg)
}

/// --verbose: how the startup identities were decoded (`by`: the thread that decoded the last).
pub(crate) unsafe fn ids_report_unit(up: *mut Unit, by: &str) {
    let Some(c) = (&*up).cache.as_ref() else { return };
    if !c.verbose {
        return;
    }
    let s = &c.stats;
    eprintln!("makepad-run cache: startup identities decoded: {} ({} in the background, {} in the prelink (keys naming a changed crate, {:.1} ms); {} duplicate, {} did not decode) in {:.1} ms, finished by {} (t={:.0} ms)",
        s.ids_decoded, s.ids_bg, s.ids_unsure, s.ids_unsure_ns as f64 / 1e6, s.ids_dup, s.ids_failed, s.ids_ns as f64 / 1e6, by, crate::clock::ms_since_start());
}
