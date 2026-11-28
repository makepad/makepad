#[cfg(any(not(target_arch = "wasm32"), test))]
use crate::FastMap;

/// Larger indices, including lent handles, use the sparse overflow map.
pub(crate) const DENSE_MAX: u32 = 65_536;

/// Objects indexed by handle within a single session.
#[cfg(any(not(target_arch = "wasm32"), test))]
#[derive(Debug)]
pub(crate) struct DenseMap<T> {
    dense: Vec<Option<T>>,
    overflow: FastMap<u32, T>,
    len: usize,
}

#[cfg(any(not(target_arch = "wasm32"), test))]
impl<T> Default for DenseMap<T> {
    fn default() -> Self {
        Self { dense: Vec::new(), overflow: FastMap::default(), len: 0 }
    }
}

#[cfg(any(not(target_arch = "wasm32"), test))]
impl<T> DenseMap<T> {
    #[inline]
    pub(crate) fn get(&self, index: u32) -> Option<&T> {
        if index < DENSE_MAX {
            self.dense.get(index as usize).and_then(Option::as_ref)
        } else {
            self.overflow.get(&index)
        }
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, index: u32) -> Option<&mut T> {
        if index < DENSE_MAX {
            self.dense.get_mut(index as usize).and_then(Option::as_mut)
        } else {
            self.overflow.get_mut(&index)
        }
    }

    pub(crate) fn insert(&mut self, index: u32, val: T) -> Option<T> {
        let old = if index < DENSE_MAX {
            let i = index as usize;
            if i >= self.dense.len() {
                self.dense.resize_with(i + 1, || None);
            }
            self.dense[i].replace(val)
        } else {
            self.overflow.insert(index, val)
        };
        if old.is_none() {
            self.len += 1;
        }
        old
    }

    pub(crate) fn remove(&mut self, index: u32) -> Option<T> {
        let val = if index < DENSE_MAX {
            self.dense.get_mut(index as usize)?.take()?
        } else {
            self.overflow.remove(&index)?
        };
        self.len -= 1;
        Some(val)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[cfg(test)]
    pub(crate) fn retain(&mut self, mut f: impl FnMut(u32, &mut T) -> bool) {
        for (i, slot) in self.dense.iter_mut().enumerate() {
            if slot.as_mut().is_some_and(|val| !f(i as u32, val)) {
                *slot = None;
                self.len -= 1;
            }
        }
        self.overflow.retain(|&index, val| {
            let keep = f(index, val);
            if !keep {
                self.len -= 1;
            }
            keep
        });
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, &T)> + '_ {
        self.dense.iter().enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|val| (i as u32, val)))
            .chain(self.overflow.iter().map(|(&index, val)| (index, val)))
    }

    #[cfg(any(target_os = "linux", target_os = "android", windows, test))]
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (u32, &mut T)> + '_ {
        self.dense.iter_mut().enumerate()
            .filter_map(|(i, slot)| slot.as_mut().map(|val| (i as u32, val)))
            .chain(self.overflow.iter_mut().map(|(&index, val)| (index, val)))
    }

    pub(crate) fn drain_all(&mut self) -> Vec<(u32, T)> {
        let mut entries = Vec::with_capacity(self.len);
        entries.extend(self.dense.drain(..).enumerate()
            .filter_map(|(i, slot)| slot.map(|val| (i as u32, val))));
        entries.extend(self.overflow.drain());
        self.len = 0;
        entries
    }
}

/// Per-session maps for backends that serve several sessions.
#[cfg(any(not(target_arch = "wasm32"), test))]
#[derive(Debug)]
pub(crate) struct SessionTable<T> {
    sessions: Vec<(u32, DenseMap<T>)>,
}

#[cfg(any(not(target_arch = "wasm32"), test))]
impl<T> Default for SessionTable<T> {
    fn default() -> Self {
        Self { sessions: Vec::new() }
    }
}

#[cfg(any(not(target_arch = "wasm32"), test))]
impl<T> SessionTable<T> {
    #[inline]
    pub(crate) fn session(&self, sid: u32) -> Option<&DenseMap<T>> {
        self.sessions.iter().find(|(s, _)| *s == sid).map(|(_, map)| map)
    }

    #[inline]
    pub(crate) fn session_mut(&mut self, sid: u32) -> Option<&mut DenseMap<T>> {
        self.sessions.iter_mut().find(|(s, _)| *s == sid).map(|(_, map)| map)
    }

    #[inline]
    pub(crate) fn get(&self, sid: u32, index: u32) -> Option<&T> {
        self.session(sid)?.get(index)
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, sid: u32, index: u32) -> Option<&mut T> {
        self.session_mut(sid)?.get_mut(index)
    }

    pub(crate) fn insert(&mut self, sid: u32, index: u32, val: T) -> Option<T> {
        if let Some(map) = self.session_mut(sid) {
            return map.insert(index, val);
        }
        let mut map = DenseMap::default();
        map.insert(index, val);
        self.sessions.push((sid, map));
        None
    }

    pub(crate) fn remove(&mut self, sid: u32, index: u32) -> Option<T> {
        self.session_mut(sid)?.remove(index)
    }

    #[cfg(target_vendor = "apple")]
    pub(crate) fn contains(&self, sid: u32, index: u32) -> bool {
        self.get(sid, index).is_some()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.sessions.iter().map(|(_, map)| map.len()).sum()
    }

    pub(crate) fn clear(&mut self) {
        self.sessions.clear();
    }

    /// Removes the session, releasing its slot storage and returning its live entries.
    pub(crate) fn take_session(&mut self, sid: u32) -> Vec<(u32, T)> {
        let Some(i) = self.sessions.iter().position(|(s, _)| *s == sid) else {
            return Vec::new();
        };
        self.sessions.swap_remove(i).1.drain_all()
    }

    #[cfg(test)]
    pub(crate) fn retain(&mut self, mut f: impl FnMut(u32, u32, &mut T) -> bool) {
        for (sid, map) in &mut self.sessions {
            map.retain(|index, val| f(*sid, index, val));
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = ((u32, u32), &T)> + '_ {
        self.sessions.iter().flat_map(|(sid, map)| {
            map.iter().map(move |(index, val)| ((*sid, index), val))
        })
    }

    #[cfg(any(target_os = "linux", target_os = "android", windows, test))]
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = ((u32, u32), &mut T)> + '_ {
        self.sessions.iter_mut().flat_map(|(sid, map)| {
            let sid = *sid;
            map.iter_mut().map(move |(index, val)| ((sid, index), val))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LENT_BASE;

    #[test]
    fn dense_map_and_session_table_bounds_and_overflow() {
        let mut table = SessionTable::default();
        let indices = [0, 1024, DENSE_MAX - 1, DENSE_MAX, LENT_BASE, u32::MAX];
        table.insert(0, 0, 10);
        for index in indices {
            assert_eq!(table.insert(1, index, 20), None);
        }
        table.insert(2, u32::MAX, 30);
        let map = table.session(1).unwrap();
        assert_eq!((map.dense.len(), map.overflow.len(), map.len()), (DENSE_MAX as usize, 3, 6));
        assert!(table.session(2).unwrap().dense.is_empty());

        for index in [0, LENT_BASE] {
            assert_eq!(table.insert(1, index, 21), Some(20));
            *table.get_mut(1, index).unwrap() += 1;
            assert_eq!(table.get(1, index), Some(&22));
        }
        assert_eq!(table.len(), 8, "overwriting live entries preserves the count");
        for index in [1024, u32::MAX] {
            assert_eq!(table.remove(1, index), Some(20));
            assert_eq!(table.remove(1, index), None);
        }
        table.retain(|sid, index, _| sid != 1 || (index != DENSE_MAX - 1 && index != DENSE_MAX));
        assert_eq!(table.len(), 4);
        for ((sid, _), val) in table.iter_mut() {
            *val += sid;
        }
        let mut live: Vec<_> = table.iter().map(|(key, val)| (key, *val)).collect();
        live.sort_unstable();
        assert_eq!(live, [((0, 0), 10), ((1, 0), 23), ((1, LENT_BASE), 23), ((2, u32::MAX), 32)]);

        let mut removed = table.take_session(1);
        removed.sort_unstable();
        assert_eq!(removed, [(0, 23), (LENT_BASE, 23)]);
        assert!(table.session(1).is_none());
        assert_eq!(table.sessions.len(), 2);
        assert_eq!(table.get(0, 0), Some(&10));
        assert_eq!(table.get(2, u32::MAX), Some(&32));
    }
}
