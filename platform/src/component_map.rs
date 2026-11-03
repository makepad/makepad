use {
    crate::cx::Cx,
    foldhash::fast::FixedState,
    std::{
        collections::hash_map::Entry,
        collections::{HashMap, HashSet},
        ops::{Deref, DerefMut, Index, IndexMut},
    },
};

/// A std `HashMap` using foldhash's fixed-seed hasher, a lot faster than SipHash for small keys
/// like `LiveId`s and uids. It isn't DoS-resistant, so don't key it by untrusted input.
pub type FastHashMap<K, V> = HashMap<K, V, FixedState>;
pub type FastHashSet<K> = HashSet<K, FixedState>;

#[derive(Debug, Clone)]
pub struct ComponentMap<K, V> {
    map: FastHashMap<K, V>,
    visible: FastHashSet<K>,
}

impl<K, V> Default for ComponentMap<K, V> {
    fn default() -> Self {
        Self {
            map: FastHashMap::default(),
            visible: FastHashSet::default(),
        }
    }
}

impl<K: std::cmp::Eq + std::hash::Hash + Copy, V> ComponentMap<K, V> {
    pub fn retain_visible(&mut self) {
        let visible = &self.visible;
        self.map.retain(|k, _| visible.contains(&k));
        self.visible.clear();
    }

    /// Whether `key` was touched (drawn) since the last retain: what the
    /// next `retain_visible*` keeps.
    pub fn is_visible(&self, key: &K) -> bool {
        self.visible.contains(key)
    }

    pub fn retain_visible_with<CB>(&mut self, mut cb: CB)
    where
        CB: FnMut(V),
        V: Default,
    {
        let visible = &self.visible;
        self.map.retain(|k, v| {
            if visible.contains(&k) {
                true
            } else {
                let mut v2 = V::default();
                std::mem::swap(v, &mut v2);
                cb(v2);
                false
            }
        });
        self.visible.clear();
    }

    pub fn retain_visible_and<CB>(&mut self, cb: CB)
    where
        CB: Fn(&K, &V) -> bool,
    {
        let visible = &self.visible;
        self.map.retain(|k, v| visible.contains(&k) || cb(k, v));
        self.visible.clear();
    }

    pub fn get_or_insert<'a, CB>(&'a mut self, cx: &mut Cx, key: K, cb: CB) -> &'a mut V
    where
        CB: FnOnce(&mut Cx) -> V,
    {
        self.visible.insert(key);
        match self.map.entry(key) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(cb(cx)),
        }
    }

    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        self.visible.insert(key);
        self.map.entry(key)
    }
}

impl<K, V> Deref for ComponentMap<K, V> {
    type Target = FastHashMap<K, V>;
    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl<K, V> DerefMut for ComponentMap<K, V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.map
    }
}

impl<K: std::cmp::Eq + std::hash::Hash + Copy, V> Index<K> for ComponentMap<K, V> {
    type Output = V;
    fn index(&self, index: K) -> &Self::Output {
        self.map.get(&index).unwrap()
    }
}

impl<K: std::cmp::Eq + std::hash::Hash + Copy, V> IndexMut<K> for ComponentMap<K, V> {
    fn index_mut(&mut self, index: K) -> &mut Self::Output {
        self.map.get_mut(&index).unwrap()
    }
}
