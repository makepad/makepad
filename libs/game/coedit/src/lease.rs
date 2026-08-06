//! Soft region leases.
//!
//! An author says what it is about to touch ("vehicles"); the host records it
//! with a TTL so other authors can queue or pick different work. Leases are
//! advisory by design: holding one does not make a submit succeed, and missing
//! one does not make it fail — they only make conflicts rarer (game.md). That
//! keeps a crashed or forgetful author from locking a region forever.

use crate::AuthorId;

#[derive(Clone, Debug, PartialEq)]
pub struct Lease {
    pub region: String,
    pub author: AuthorId,
    pub expires_at: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LeaseOutcome {
    /// Granted (or renewed by the same author).
    Granted { expires_at: f64 },
    /// Someone else holds it. Not a refusal to edit — a hint to pick other work.
    Held { by: AuthorId, expires_at: f64 },
    /// The table is full; a submit is still allowed.
    TooMany,
}

#[derive(Clone, Debug)]
pub struct LeaseTable {
    leases: Vec<Lease>,
    max: usize,
}

impl LeaseTable {
    pub fn new(max: usize) -> Self {
        Self {
            leases: Vec::new(),
            max,
        }
    }

    fn reap(&mut self, now: f64) {
        self.leases.retain(|lease| lease.expires_at > now);
    }

    pub fn acquire(
        &mut self,
        author: AuthorId,
        region: &str,
        ttl: f64,
        now: f64,
    ) -> LeaseOutcome {
        self.reap(now);
        let expires_at = now + ttl.max(0.0);

        if let Some(existing) = self.leases.iter_mut().find(|l| l.region == region) {
            if existing.author == author {
                existing.expires_at = expires_at;
                return LeaseOutcome::Granted { expires_at };
            }
            return LeaseOutcome::Held {
                by: existing.author,
                expires_at: existing.expires_at,
            };
        }

        if self.leases.len() >= self.max {
            return LeaseOutcome::TooMany;
        }
        self.leases.push(Lease {
            region: region.to_string(),
            author,
            expires_at,
        });
        LeaseOutcome::Granted { expires_at }
    }

    pub fn release(&mut self, author: AuthorId, region: &str) {
        self.leases
            .retain(|lease| !(lease.region == region && lease.author == author));
    }

    pub fn release_author(&mut self, author: AuthorId) {
        self.leases.retain(|lease| lease.author != author);
    }

    /// Live leases, so an author can be told what is taken before it starts.
    pub fn active(&mut self, now: f64) -> &[Lease] {
        self.reap(now);
        &self.leases
    }

    pub fn holder(&mut self, region: &str, now: f64) -> Option<AuthorId> {
        self.reap(now);
        self.leases
            .iter()
            .find(|l| l.region == region)
            .map(|l| l.author)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_free_region_is_granted_and_renewable_by_its_holder() {
        let mut table = LeaseTable::new(4);
        assert_eq!(
            table.acquire(AuthorId(1), "vehicles", 10.0, 100.0),
            LeaseOutcome::Granted { expires_at: 110.0 }
        );
        assert_eq!(
            table.acquire(AuthorId(1), "vehicles", 10.0, 105.0),
            LeaseOutcome::Granted { expires_at: 115.0 },
            "the holder renews rather than blocking itself"
        );
        assert_eq!(table.active(105.0).len(), 1);
    }

    #[test]
    fn a_held_region_reports_its_holder() {
        let mut table = LeaseTable::new(4);
        table.acquire(AuthorId(1), "vehicles", 10.0, 100.0);
        assert_eq!(
            table.acquire(AuthorId(2), "vehicles", 10.0, 101.0),
            LeaseOutcome::Held {
                by: AuthorId(1),
                expires_at: 110.0
            }
        );
    }

    #[test]
    fn an_expired_lease_stops_holding_the_region() {
        let mut table = LeaseTable::new(4);
        table.acquire(AuthorId(1), "vehicles", 10.0, 100.0);
        assert_eq!(
            table.acquire(AuthorId(2), "vehicles", 10.0, 111.0),
            LeaseOutcome::Granted { expires_at: 121.0 },
            "a crashed author must not lock a region forever"
        );
        assert_eq!(table.holder("vehicles", 111.0), Some(AuthorId(2)));
    }

    #[test]
    fn release_frees_only_the_authors_own_lease() {
        let mut table = LeaseTable::new(4);
        table.acquire(AuthorId(1), "vehicles", 10.0, 100.0);
        table.release(AuthorId(2), "vehicles");
        assert_eq!(table.holder("vehicles", 100.0), Some(AuthorId(1)));
        table.release(AuthorId(1), "vehicles");
        assert_eq!(table.holder("vehicles", 100.0), None);
    }

    #[test]
    fn the_table_is_bounded() {
        let mut table = LeaseTable::new(2);
        table.acquire(AuthorId(1), "a", 10.0, 0.0);
        table.acquire(AuthorId(1), "b", 10.0, 0.0);
        assert_eq!(
            table.acquire(AuthorId(1), "c", 10.0, 0.0),
            LeaseOutcome::TooMany
        );
    }

    #[test]
    fn leaving_releases_everything_an_author_held() {
        let mut table = LeaseTable::new(4);
        table.acquire(AuthorId(7), "a", 10.0, 0.0);
        table.acquire(AuthorId(7), "b", 10.0, 0.0);
        table.acquire(AuthorId(8), "c", 10.0, 0.0);
        table.release_author(AuthorId(7));
        assert_eq!(table.active(0.0).len(), 1);
    }
}
