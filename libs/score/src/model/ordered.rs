use makepad_micro_serde::{DeBin, DeBinErr, SerBin};
use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

/// A deterministically iterated map used by the serialized score model.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OrderedMap<K, V>(BTreeMap<K, V>);

impl<K: Ord, V> OrderedMap<K, V> {
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.0.insert(key, value)
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.0.remove(key)
    }
}

impl<K, V> Deref for OrderedMap<K, V> {
    type Target = BTreeMap<K, V>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<K, V> DerefMut for OrderedMap<K, V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<K: Ord, V> FromIterator<(K, V)> for OrderedMap<K, V> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<K: SerBin, V: SerBin> SerBin for OrderedMap<K, V> {
    fn ser_bin(&self, output: &mut Vec<u8>) {
        (self.len() as u64).ser_bin(output);
        for (key, value) in self.iter() {
            key.ser_bin(output);
            value.ser_bin(output);
        }
    }
}

impl<K: DeBin + Ord, V: DeBin> DeBin for OrderedMap<K, V> {
    fn de_bin(offset: &mut usize, input: &[u8]) -> Result<Self, DeBinErr> {
        let count = u64::de_bin(offset, input)?;
        let count = usize::try_from(count).map_err(|_| DeBinErr {
            msg: "OrderedMap length".to_string(),
            o: *offset,
            l: 0,
            s: input.len(),
        })?;
        let mut map = BTreeMap::new();
        for _ in 0..count {
            let key = K::de_bin(offset, input)?;
            let value = V::de_bin(offset, input)?;
            if map.insert(key, value).is_some() {
                return Err(DeBinErr {
                    msg: "duplicate OrderedMap key".to_string(),
                    o: *offset,
                    l: 0,
                    s: input.len(),
                });
            }
        }
        Ok(Self(map))
    }
}
