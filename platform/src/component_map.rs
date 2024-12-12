use {
    std::{
        ops::{Index, IndexMut, Deref, DerefMut},
        collections::{HashSet, HashMap,},
        collections::hash_map::Entry
    },
    crate::cx::Cx
};

#[derive(Debug, Clone)]
pub struct ComponentMap<K,V>{
    map: HashMap<K,V>,
    visible: HashSet<K>
}

impl<K,V> Default for ComponentMap<K,V>{
    fn default()->Self{
        Self{
            map: HashMap::new(),
            visible: HashSet::new()
        }
    }
}

impl<K: std::cmp::Eq + std::hash::Hash + Copy,V> ComponentMap<K,V>{
    pub fn retain_visible(&mut self) {
        let visible = &self.visible;
        self.map.retain( | k, _ | visible.contains(&k));
        self.visible.clear();
    }
    
    pub fn retain_visible_with<CB>(&mut self, mut cb:CB)
    where CB: FnMut(V),
          V: Default
    {
        let visible = &self.visible;
        self.map.retain( | k, v | if visible.contains(&k){true}else
        {
            let mut v2 = V::default();
            std::mem::swap(v, &mut v2);
            cb(v2);
            false
        });
        self.visible.clear();
    } 

    pub fn retain_visible_and<CB>(&mut self, cb:CB)
    where CB: Fn(&K, &V)->bool
    {
        let visible = &self.visible;
        self.map.retain( | k, v | visible.contains(&k) || cb(k,v));
        self.visible.clear();
    } 

    pub fn get_or_insert<'a, CB>(&'a mut self, cx:&mut Cx, key:K, cb:CB)->&'a mut V
    where CB: FnOnce(&mut Cx)->V{
        self.visible.insert(key);
        match self.map.entry(key){
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(cb(cx))
        }
    }

    pub fn entry(&mut self, key: K) -> Entry<K, V> {
        self.visible.insert(key);
        self.map.entry(key)
    }
}
 
impl<K,V> Deref for ComponentMap<K,V> {
    type Target = HashMap<K,V>;
    fn deref(&self) -> &Self::Target {&self.map}
}

impl<K,V> DerefMut for ComponentMap<K,V> {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.map}
}

impl<K: std::cmp::Eq + std::hash::Hash + Copy, V> Index<K> for ComponentMap<K,V>{
    type Output = V;
    fn index(&self, index:K)->&Self::Output{
        self.map.get(&index).unwrap()
    }
}

impl<K: std::cmp::Eq + std::hash::Hash + Copy, V> IndexMut<K> for ComponentMap<K,V>{
    fn index_mut(&mut self, index:K)->&mut Self::Output{
        self.map.get_mut(&index).unwrap()
    }
}
