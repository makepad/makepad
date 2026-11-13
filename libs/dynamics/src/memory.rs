//! Owned scratch pools and constraint arena invalidation.
//!
//! Translated from `src/engine/engine_memory.c` and `src/engine/engine_memory.h`
//! of the engine named in CREDITS.md (Apache-2.0,
//! Copyright 2021 DeepMind Technologies Limited); changed: translated to Rust.
//! Return borrowed vectors to their pool when the scope ends; capacity survives warm steps.

use crate::collision_gjk::CcdBuffer;
use crate::collision_driver::{Pair, Packed32, Sap, CollisionTree, MidItem};
use crate::data::{Data, Contact, PreContact};
use crate::sensor::ContactInfo;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scratch {
    nums: Classes<f64>,
    ints: Classes<i32>,
    bytes: Classes<u8>,
    pairs: Vec<Vec<Pair>>,
    packed_pairs: Vec<Vec<Packed32>>,
    saps: Vec<Vec<Sap>>,
    collision_trees: Vec<Vec<CollisionTree>>,
    mid_items: Vec<Vec<MidItem>>,
    pre_contacts: Vec<Vec<PreContact>>,
    contact_counts: Vec<Vec<i32>>,
    contact_infos: Vec<Vec<ContactInfo>>,
    ccd_buffers: Vec<(usize, CcdBuffer)>,
    /// The primal solver's context, kept from one solve to the next (its arrays as the reference's arena keeps
    /// its memory); a solve takes it and puts it back.
    pub(crate) primal: Option<Box<crate::solver::PrimalContext>>,
}

/// Pooled vectors by size class: class c holds the vectors of capacity 2^c up to 2^(c+1) - 1. Taking and
/// giving back is a pop and a push, as the reference's stack allocations are a pointer bump: a search of the
/// whole pool took about a tenth of the islands' solve on 100 humanoids (3500 takes a step).
#[derive(Clone, Debug, Default, PartialEq)]
struct Classes<T>(Vec<Vec<Vec<T>>>);

/// A pooled vector of `n` elements: one of the class of `n` rounded up to a power of two, else of a larger class,
/// else a new one of that capacity. Like the reference's stack allocations it is not cleared: the elements it
/// keeps hold what an earlier user left, and every caller writes before it reads.
#[inline]
fn take<T: Copy + Default>(pool: &mut Classes<T>, n: usize) -> Vec<T> {
    if n == 0 {
        return Vec::new();
    }
    let class = (usize::BITS - (n - 1).leading_zeros()) as usize;
    let found = pool.0.iter_mut().skip(class).find_map(|vs| vs.pop());
    let mut v = found.unwrap_or_else(|| Vec::with_capacity(1 << class));
    // resize sets only the newly exposed tail
    v.truncate(n);
    v.resize(n, T::default());
    v
}

/// Gives `v` back to its class (by its capacity rounded down to a power of two).
#[inline]
fn put<T>(pool: &mut Classes<T>, v: Vec<T>) {
    if v.capacity() == 0 {
        return;
    }
    let class = (usize::BITS - 1 - v.capacity().leading_zeros()) as usize;
    match pool.0.get_mut(class) {
        Some(vs) => vs.push(v),
        None => put_new_class(pool, class, v),
    }
}

// The first vector of a class above every class so far.
#[cold]
fn put_new_class<T>(pool: &mut Classes<T>, class: usize, v: Vec<T>) {
    pool.0.resize_with(class + 1, Vec::new);
    pool.0[class].push(v);
}

/// `n` elements of a pooled vector that keeps its length. Like the reference's stack allocations,
/// taking one writes nothing once its vector has been that long: a vector lent for fewer elements is
/// never cut, so a larger request after it does not fill it again.
pub(crate) struct Buf<T> {
    v: Vec<T>,
    n: usize,
}

impl<T> std::ops::Deref for Buf<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.v[..self.n]
    }
}

impl<T> std::ops::DerefMut for Buf<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.v[..self.n]
    }
}

/// The shortest pooled vector at least `n` long, else the longest, grown to `n` (only the growth is
/// written). Its elements hold what an earlier user left: every caller writes before it reads.
fn take_buf<T: Copy + Default>(pool: &mut Vec<Vec<T>>, n: usize) -> Buf<T> {
    let mut best: Option<usize> = None;
    for (i, v) in pool.iter().enumerate() {
        let better = best.is_none_or(|j| {
            let (len, best_len) = (v.len(), pool[j].len());
            if best_len >= n { len >= n && len < best_len } else { len > best_len }
        });
        if better {
            best = Some(i);
        }
    }
    let mut v = best.map_or_else(Vec::new, |i| pool.swap_remove(i));
    if v.len() < n {
        v.resize(n, T::default());
    }
    Buf { v, n }
}

// The typed pools below hand out vectors like take(): the elements they keep are not cleared.
impl Scratch {
    pub(crate) fn contact_info(&mut self, n: usize) -> Vec<ContactInfo> {
        let pool = &mut self.contact_infos;
        let mut best = None;
        for (i, v) in pool.iter().enumerate() {
            if v.capacity() >= n && best.is_none_or(|j: usize| v.capacity() < pool[j].capacity()) {
                best = Some(i);
            }
        }
        let index = best.or_else(|| pool.len().checked_sub(1));
        let mut v = index.map_or_else(Vec::new, |i| pool.swap_remove(i));
        v.resize(n, ContactInfo::default());
        v
    }
    pub(crate) fn put_contact_info(&mut self, v: Vec<ContactInfo>) {
        self.contact_infos.push(v);
    }

    pub(crate) fn pair(&mut self, n: usize) -> Vec<Pair> {
        // Reuse an undersized vector too: resize grows it, never shrink capacity.
        let pool = &mut self.pairs;
        let mut best = None;
        for (i, v) in pool.iter().enumerate() {
            if v.capacity() >= n && best.is_none_or(|j: usize| v.capacity() < pool[j].capacity()) { best = Some(i); }
        }
        let index = best.or_else(|| pool.len().checked_sub(1));
        let mut v = index.map_or_else(Vec::new, |i| pool.swap_remove(i));
        v.resize(n, Pair::default());
        v
    }
    pub(crate) fn put_pair(&mut self, v: Vec<Pair>) { self.pairs.push(v); }
    /// The broadphase's pair lists: sized for every pair of bodies, of which few are written.
    pub(crate) fn packed_pair(&mut self, n: usize) -> Buf<Packed32> {
        take_buf(&mut self.packed_pairs, n)
    }
    pub(crate) fn put_packed_pair(&mut self, b: Buf<Packed32>) { self.packed_pairs.push(b.v); }
    /// The midphase's broadphase pairs, empty, with the room an earlier step left.
    pub(crate) fn mid_items(&mut self) -> Vec<MidItem> {
        let mut v = self.mid_items.pop().unwrap_or_default();
        v.clear();
        v
    }
    pub(crate) fn put_mid_items(&mut self, v: Vec<MidItem>) { self.mid_items.push(v); }
    pub(crate) fn sap(&mut self, n: usize) -> Vec<Sap> {
        // Reuse an undersized vector too: resize grows it, never shrink capacity.
        let pool = &mut self.saps;
        let mut best = None;
        for (i, v) in pool.iter().enumerate() {
            if v.capacity() >= n && best.is_none_or(|j: usize| v.capacity() < pool[j].capacity()) { best = Some(i); }
        }
        let index = best.or_else(|| pool.len().checked_sub(1));
        let mut v = index.map_or_else(Vec::new, |i| pool.swap_remove(i));
        v.resize(n, Sap::default());
        v
    }
    pub(crate) fn put_sap(&mut self, v: Vec<Sap>) { self.saps.push(v); }
    pub(crate) fn collision_tree(&mut self, n: usize) -> Vec<CollisionTree> {
        // Reuse an undersized vector too: resize grows it, never shrink capacity.
        let pool = &mut self.collision_trees;
        let mut best = None;
        for (i, v) in pool.iter().enumerate() {
            if v.capacity() >= n && best.is_none_or(|j: usize| v.capacity() < pool[j].capacity()) { best = Some(i); }
        }
        let index = best.or_else(|| pool.len().checked_sub(1));
        let mut v = index.map_or_else(Vec::new, |i| pool.swap_remove(i));
        v.resize(n, CollisionTree::default());
        v
    }
    pub(crate) fn put_collision_tree(&mut self, v: Vec<CollisionTree>) { self.collision_trees.push(v); }
    pub(crate) fn pre_contact(&mut self, n: usize) -> Vec<PreContact> {
        // Reuse an undersized vector too: resize grows it, never shrink capacity.
        let pool = &mut self.pre_contacts;
        let mut best = None;
        for (i, v) in pool.iter().enumerate() {
            if v.capacity() >= n && best.is_none_or(|j: usize| v.capacity() < pool[j].capacity()) { best = Some(i); }
        }
        let index = best.or_else(|| pool.len().checked_sub(1));
        let mut v = index.map_or_else(Vec::new, |i| pool.swap_remove(i));
        v.resize(n, PreContact::default());
        v
    }
    pub(crate) fn put_pre_contact(&mut self, v: Vec<PreContact>) { self.pre_contacts.push(v); }
    pub(crate) fn contact_count(&mut self, n: usize) -> Vec<i32> {
        // Reuse an undersized vector too: resize grows it, never shrink capacity.
        let pool = &mut self.contact_counts;
        let mut best = None;
        for (i, v) in pool.iter().enumerate() {
            if v.capacity() >= n && best.is_none_or(|j: usize| v.capacity() < pool[j].capacity()) { best = Some(i); }
        }
        let index = best.or_else(|| pool.len().checked_sub(1));
        let mut v = index.map_or_else(Vec::new, |i| pool.swap_remove(i));
        v.resize(n, i32::default());
        v
    }
    pub(crate) fn put_contact_count(&mut self, v: Vec<i32>) { self.contact_counts.push(v); }

    // The typed buffer grows its constituent vectors as CCD uses them. Retain
    // both EPA and manifold storage, unlike C's overlapping byte allocation.
    pub fn ccd_buffer(&mut self, size: usize) -> CcdBuffer {
        let mut best = None;
        for (i, (capacity, _)) in self.ccd_buffers.iter().enumerate() {
            if *capacity >= size && best.is_none_or(|j: usize| *capacity < self.ccd_buffers[j].0) {
                best = Some(i);
            }
        }
        // Reuse a smaller buffer as well: CCD will grow it when necessary.
        let index = best.or_else(|| self.ccd_buffers.len().checked_sub(1));
        index.map_or_else(CcdBuffer::default, |i| self.ccd_buffers.swap_remove(i).1)
    }
    pub fn put_ccd_buffer(&mut self, buffer: CcdBuffer, size: usize) {
        self.ccd_buffers.push((size, buffer));
    }

    #[inline]
    pub fn num(&mut self, n: usize) -> Vec<f64> {
        take(&mut self.nums, n)
    }
    #[inline]
    pub fn put_num(&mut self, v: Vec<f64>) {
        put(&mut self.nums, v);
    }
    #[inline]
    pub fn int(&mut self, n: usize) -> Vec<i32> {
        take(&mut self.ints, n)
    }
    #[inline]
    pub fn put_int(&mut self, v: Vec<i32>) {
        put(&mut self.ints, v);
    }
    #[inline]
    pub fn byte(&mut self, n: usize) -> Vec<u8> {
        take(&mut self.bytes, n)
    }
    #[inline]
    pub fn put_byte(&mut self, v: Vec<u8>) {
        put(&mut self.bytes, v);
    }
}

// Translated from src/engine/engine_memory.c (Apache-2.0,
// Copyright 2021 DeepMind Technologies Limited); changed: translated to Rust.
// C's non-ASan red zone and sizeof/alignof(mjStackFrame) on x86-64.
const REDZONE: usize = 0;
const STACK_FRAME_SIZE: usize = 24;
const STACK_FRAME_ALIGN: usize = 8;

// mj_stackBytesAvailable: Scratch grows, so nothing runs out.
pub fn stack_bytes_available(_d: &Data) -> usize {
    usize::MAX
}

pub fn stack_bytes(bytes: usize, alignment: usize) -> usize {
    if bytes == 0 {
        return 0;
    }
    bytes + alignment - 1 + 2 * REDZONE
}

pub fn stack_frame_bytes() -> usize {
    stack_bytes(STACK_FRAME_SIZE, STACK_FRAME_ALIGN)
}

// engine_memory.h: mj_clearEfc. Contact storage is rebound, not discarded.
pub fn clear_efc(d: &mut Data) {
    d.efc_type.clear();
    d.efc_id.clear();
    d.efc_j_rownnz.clear();
    d.efc_j_rowadr.clear();
    d.efc_j_rowsuper.clear();
    d.efc_j_colind.clear();
    d.efc_j.clear();
    d.efc_pos.clear();
    d.efc_margin.clear();
    d.efc_frictionloss.clear();
    d.efc_diag_a.clear();
    d.efc_kbip.clear();
    d.efc_d.clear();
    d.efc_r.clear();
    d.tendon_efcadr.clear();
    d.efc_vel.clear();
    d.efc_aref.clear();
    d.efc_b.clear();
    d.efc_state.clear();
    d.efc_force.clear();
    d.efc_y_rownnz.clear();
    d.efc_y_rowadr.clear();
    d.efc_y_colind.clear();
    d.efc_y.clear();
    d.efc_ar_rownnz.clear();
    d.efc_ar_rowadr.clear();
    d.efc_ar_colind.clear();
    d.efc_ar.clear();
    d.tree_island.clear();
    d.island_ntree.clear();
    d.island_itreeadr.clear();
    d.map_itree2tree.clear();
    d.dof_island.clear();
    d.island_nv.clear();
    d.island_idofadr.clear();
    d.island_dofadr.clear();
    d.map_dof2idof.clear();
    d.map_idof2dof.clear();
    d.ifrc_smooth.clear();
    d.iacc_smooth.clear();
    d.iacc.clear();
    d.efc_island.clear();
    d.island_ne.clear();
    d.island_nf.clear();
    d.island_nefc.clear();
    d.island_iefcadr.clear();
    d.map_efc2iefc.clear();
    d.map_iefc2efc.clear();
    d.iefc_type.clear();
    d.iefc_id.clear();
    d.iefc_frictionloss.clear();
    d.iefc_d.clear();
    d.iefc_r.clear();
    d.iefc_aref.clear();
    d.iefc_state.clear();
    d.iefc_force.clear();
    d.ifrc_constraint.clear();
    d.efm_c.clear();
    d.efm_diag.clear();
    d.efm_ck.clear();
    d.efm_sdiag.clear();
    d.efm_fluid.clear();
    d.efm_tid.clear();
    d.efm_ts.clear();
    d.efm_tk.clear();
    d.efm_aid.clear();
    d.efm_as.clear();
    d.efm_ak.clear();
    d.efm_ca.clear();
    d.efm_k_rownnz.clear();
    d.efm_k_rowadr.clear();
    d.efm_k_colind.clear();
    d.efm_k_val.clear();
    d.efm_dofid.clear();
    d.efm_con_ind.clear();
    d.efm_con_val.clear();
    d.efm_l.clear();
    d.ne = 0; d.nf = 0; d.nl = 0; d.nefc = 0;
    d.nisland = 0; d.nidof = 0;
    d.n_j = 0; d.n_y = 0; d.n_a = 0;
    d.efm_active = 0;
    d.nefm_t = 0; d.nefm_a = 0; d.nefm_k = 0; d.nefm_l = 0; d.nefmdof = 0; d.nefmcon = 0;
    for i in 0..d.ncon as usize { d.contact[i].efc_address = -1; }
    d.parena = d.ncon as usize * std::mem::size_of::<Contact>();
}
