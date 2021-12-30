use {
    std::{
        ops::Deref,
        ops::DerefMut,
        collections::{HashSet, HashMap,},
        collections::hash_map::Entry
    },
    makepad_render::{Cx, LivePtr}
};

#[derive(Clone)]
pub struct ComponentGc<K,V>{
    map: HashMap<K,V>,
    visible: HashSet<K>
}

impl<K,V> Default for ComponentGc<K,V>{
    fn default()->Self{
        Self{
            map: HashMap::new(),
            visible: HashSet::new()
        }
    }
}

impl<K: std::cmp::Eq + std::hash::Hash + Copy,V> ComponentGc<K,V>{
    pub fn retain_visible(&mut self) {
        let visible = &self.visible;
        self.map.retain( | k, _ | visible.contains(&k));
        self.visible.clear();
    }
    
    pub fn retain_visible_and<CB>(&mut self, cb:CB)
    where CB: Fn(&K, &V)->bool
    {
        let visible = &self.visible;
        self.map.retain( | k, v | visible.contains(&k) || cb(k,v));
        self.visible.clear();
    }
    
    pub fn get_or_insert_with_ptr<'a, CB>(&'a mut self, cx:&mut Cx, key:K, ptr:Option<LivePtr>, cb:CB)->&'a mut V
    where CB: FnOnce(&mut Cx, LivePtr)->V{
        self.visible.insert(key);
        match self.map.entry(key){
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(cb(cx, ptr.unwrap()))
        }
    }
    
    pub fn get_or_insert<'a, CB>(&'a mut self, cx:&mut Cx, key:K, cb:CB)->&'a mut V
    where CB: FnOnce(&mut Cx)->V{
        self.visible.insert(key);
        match self.map.entry(key){
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(cb(cx))
        }
    }
}
 
impl<K,V> Deref for ComponentGc<K,V> {
    type Target = HashMap<K,V>;
    fn deref(&self) -> &Self::Target {&self.map}
}

impl<K,V> DerefMut for ComponentGc<K,V> {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.map}
}
