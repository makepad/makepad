//! Quota policy and store-owned byte accounting.

use super::durability::StorageValues;
use makepad_platform::{StorageError, StorageEstimate};

pub const DEFAULT_STORE_BUDGET: u64 = 512 * 1024 * 1024;
pub const DEFAULT_MIN_RESERVE: u64 = 64 * 1024 * 1024;
pub const DEFAULT_CATALOG_HEADROOM: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaPolicy {
    pub store_budget: u64,
    pub min_reserve: u64,
    pub reserve_percent: u8,
    pub catalog_headroom: u64,
}

impl Default for QuotaPolicy {
    fn default() -> Self {
        Self {
            store_budget: DEFAULT_STORE_BUDGET,
            min_reserve: DEFAULT_MIN_RESERVE,
            reserve_percent: 10,
            catalog_headroom: DEFAULT_CATALOG_HEADROOM,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaStatus {
    pub origin_usage: u64,
    pub origin_quota: u64,
    pub store_owned: u64,
    pub reserve: u64,
    pub available: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaExceeded {
    pub required: u64,
    pub available: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct QuotaManager {
    policy: QuotaPolicy,
    store_owned: u64,
}

impl QuotaManager {
    pub fn new(policy: QuotaPolicy) -> Self {
        Self { policy, store_owned: 0 }
    }

    pub fn policy(&self) -> QuotaPolicy {
        self.policy
    }

    pub fn store_owned_bytes(&self) -> u64 {
        self.store_owned
    }

    pub fn set_store_owned_bytes(&mut self, bytes: u64) {
        self.store_owned = bytes;
    }

    pub fn status(&self, estimate: StorageEstimate) -> QuotaStatus {
        let reserve = self.policy.min_reserve.max(
            estimate
                .quota
                .saturating_mul(self.policy.reserve_percent as u64)
                / 100,
        );
        let origin_available = estimate
            .quota
            .saturating_sub(estimate.usage)
            .saturating_sub(reserve);
        let store_available = self.policy.store_budget.saturating_sub(self.store_owned);
        QuotaStatus {
            origin_usage: estimate.usage,
            origin_quota: estimate.quota,
            store_owned: self.store_owned,
            reserve,
            available: origin_available.min(store_available),
        }
    }

    /// Refuse before any blob or catalog write. Catalog headroom is always
    /// included so an admitted object can still be named atomically.
    pub fn preflight(
        &self,
        estimate: StorageEstimate,
        declared_bytes: u64,
    ) -> Result<QuotaStatus, QuotaExceeded> {
        let status = self.status(estimate);
        let required = declared_bytes.saturating_add(self.policy.catalog_headroom);
        if required > status.available {
            Err(QuotaExceeded { required, available: status.available })
        } else {
            Ok(status)
        }
    }

    pub fn account_write(&mut self, old_len: u64, new_len: u64) {
        self.store_owned = self
            .store_owned
            .saturating_sub(old_len)
            .saturating_add(new_len);
    }

    pub fn account_delete(&mut self, old_len: u64) {
        self.store_owned = self.store_owned.saturating_sub(old_len);
    }

    /// Crash reconciliation is intentionally explicit and maintenance-only;
    /// normal restore uses descriptors and never scans the namespace.
    pub fn reconcile(
        &mut self,
        storage: &dyn StorageValues,
    ) -> Result<u64, StorageError> {
        let mut total = 0u64;
        for key in storage.list("")? {
            if let Some(bytes) = storage.get(&key)? {
                total = total.saturating_add(bytes.len() as u64);
            }
        }
        self.store_owned = total;
        Ok(total)
    }
}
