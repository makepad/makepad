//! Staging buffers cannot accept another GPU copy while mapped. Return and unmap are
//! queued to the HAL thread, including releases belonging to a replaced device. A returned
//! buffer waits for reuse until it has been free for `IDLE_NS` (`Pool::trim`).
use super::device::{Device, hr, lock};
use crate::ledger::{self, Charge};
use makepad_windows_sys::Win32::Graphics::Direct3D11::*;
use std::{collections::BTreeMap, sync::{Arc, Mutex}};

/// How long a free staging buffer is kept for reuse.
const IDLE_NS: u64 = 5_000_000_000;

/// A staging buffer and its ledger charge.
struct Held { buffer: ID3D11Buffer, _charge: Charge }

pub(super) struct Pool {
    dev: Arc<Device>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// free buffers by size, each with when it came back
    free: Mutex<BTreeMap<u32, Vec<(Held, u64)>>>,
}

struct Returned { pool: Arc<Pool>, held: Held, size: u32, mapped: bool }
static RETURNED: Mutex<Vec<Returned>> = Mutex::new(Vec::new());

pub(super) fn drain() {
    let returned = std::mem::take(&mut *lock(&RETURNED));
    let now = crate::clock_ns();
    for r in returned {
        if r.mapped { unsafe { r.pool.dev.ctx.Unmap(r.held.buffer.as_ID3D11Resource(), 0); } }
        lock(&r.pool.free).entry(r.size).or_default().push((r.held, now));
    }
}

impl Pool {
    pub fn new(dev: Arc<Device>, wake: Arc<dyn Fn() + Send + Sync>) -> Arc<Self> {
        Arc::new(Self { dev, wake, free: Mutex::new(BTreeMap::new()) })
    }
    pub fn alloc(self: &Arc<Self>, len: u64) -> Result<Staging, String> {
        let size = u32::try_from(len.max(4)).map_err(|_| "staging size overflow")?;
        let size = size.checked_next_power_of_two().unwrap_or(size);
        let available = lock(&self.free).get_mut(&size).and_then(Vec::pop).map(|(held, _)| held);
        let held = match available {
            Some(held) => held,
            None => {
                let desc = D3D11_BUFFER_DESC { ByteWidth: size, Usage: D3D11_USAGE_STAGING, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..Default::default() };
                let mut buffer = None;
                unsafe { self.dev.device.CreateBuffer(&desc, None, Some(&mut buffer)) }.map_err(|e| hr("readback lease", e))?;
                Held { buffer: buffer.ok_or("no staging buffer")?, _charge: self.dev.pools.charge(ledger::Pool::Leases, size as u64) }
            }
        };
        Ok(Staging { pool: self.clone(), held: Some(held), size, mapped: false })
    }

    /// Releases the buffers free for `IDLE_NS`.
    pub fn trim(&self, now: u64) {
        let mut free = lock(&self.free);
        for list in free.values_mut() { list.retain(|(_, back)| now.saturating_sub(*back) < IDLE_NS); }
        free.retain(|_, list| !list.is_empty());
    }
}

pub(super) struct Staging { pool: Arc<Pool>, held: Option<Held>, size: u32, mapped: bool }
impl Staging {
    pub fn buffer(&self) -> &ID3D11Buffer { &self.held.as_ref().unwrap().buffer }
    pub fn map(mut self, ranges: &[(u64, u64)]) -> Result<Vec<crate::lease::Lease>, String> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { self.pool.dev.ctx.Map(self.buffer().as_ID3D11Resource(), 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.map_err(|e| hr("map readback lease", e))?;
        self.mapped = true;
        let owner = Arc::new(self);
        Ok(ranges.iter().enumerate().map(|(order, (offset, len))| {
            // SAFETY: the staging resource is mapped only after GPU completion and remains
            // immutable until the last range releases its common owner.
            unsafe { crate::lease::Lease::new(owner.clone(), (mapped.pData as *const u8).add(*offset as usize), *len as usize, order as u32) }
        }).collect())
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        lock(&RETURNED).push(Returned { pool: self.pool.clone(), held: self.held.take().unwrap(), size: self.size, mapped: self.mapped });
        (self.pool.wake)();
    }
}
