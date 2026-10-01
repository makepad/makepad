//! An object graph for writing offset-linked tables (the layout tables, the
//! variation stores): each object is its bytes plus links (an offset field,
//! its width, the object it points at and the object it is relative to).
//! Packing places every object after all objects linking to it, children
//! close to their parents, and fails when an offset does not fit, so a
//! caller can retry with another layout (extension lookups, no sharing).

use {crate::SubsetError, std::collections::{BinaryHeap, HashMap}, std::cmp::Reverse};

pub(crate) type Id = usize;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct Link {
    at: usize,
    width: u8,
    target: Id,
    /// The object the offset is measured from; `None`: the linking object.
    base: Option<Id>,
}

/// One object being written.
#[derive(Default)]
pub(crate) struct W {
    pub data: Vec<u8>,
    links: Vec<Link>,
}

impl W {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.data.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.data.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.data.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.data.extend_from_slice(b);
        self
    }
    /// A 16-bit offset to `target` (null when `None`).
    pub fn off16(&mut self, target: Option<Id>) -> &mut Self {
        self.link(2, target, None)
    }
    pub fn off32(&mut self, target: Option<Id>) -> &mut Self {
        self.link(4, target, None)
    }
    /// A 16-bit offset measured from `base` instead of this object.
    pub fn off16_from(&mut self, target: Option<Id>, base: Id) -> &mut Self {
        self.link(2, target, Some(base))
    }
    fn link(&mut self, width: u8, target: Option<Id>, base: Option<Id>) -> &mut Self {
        if let Some(target) = target {
            self.links.push(Link {
                at: self.data.len(),
                width,
                target,
                base,
            });
        }
        self.data.resize(self.data.len() + width as usize, 0);
        self
    }
}

struct Object {
    data: Vec<u8>,
    links: Vec<Link>,
    /// Lower tiers are placed first (lookup headers before subtables).
    tier: u8,
}

pub(crate) struct Graph {
    objects: Vec<Option<Object>>,
    shared: HashMap<Vec<u8>, Id>,
    /// Share identical link-free objects (coverage, class definitions,
    /// anchors, device tables).
    pub share: bool,
}

impl Graph {
    pub fn new(share: bool) -> Self {
        Self {
            objects: Vec::new(),
            shared: HashMap::new(),
            share,
        }
    }

    /// An id for an object written later with [`Graph::set`] (so its
    /// children can measure offsets from it).
    pub fn reserve(&mut self) -> Id {
        self.objects.push(None);
        self.objects.len() - 1
    }

    pub fn set(&mut self, id: Id, w: W, tier: u8) {
        self.objects[id] = Some(Object {
            data: w.data,
            links: w.links,
            tier,
        });
    }

    pub fn add(&mut self, w: W, tier: u8) -> Id {
        if self.share && w.links.is_empty() {
            if let Some(&id) = self.shared.get(&w.data) {
                return id;
            }
            let id = self.reserve();
            self.shared.insert(w.data.clone(), id);
            self.set(id, w, tier);
            return id;
        }
        let id = self.reserve();
        self.set(id, w, tier);
        id
    }

    /// Bytes of an object already added (for patching before packing).
    pub fn data_mut(&mut self, id: Id) -> &mut Vec<u8> {
        &mut self.objects[id].as_mut().expect("object written").data
    }

    /// The table rooted at `root`.
    pub fn pack(&self, root: Id) -> Result<Vec<u8>, SubsetError> {
        let object = |id: Id| self.objects[id].as_ref().ok_or(SubsetError::Malformed("unwritten object"));
        // Depth-first discovery order: the placement preference.
        let mut order = vec![usize::MAX; self.objects.len()];
        let mut parents = vec![0usize; self.objects.len()];
        let mut stack = vec![root];
        let mut next = 0;
        order[root] = 0;
        let mut seen = vec![false; self.objects.len()];
        seen[root] = true;
        let mut reachable = Vec::new();
        while let Some(id) = stack.pop() {
            order[id] = next;
            next += 1;
            reachable.push(id);
            for link in object(id)?.links.iter().rev() {
                if !seen[link.target] {
                    seen[link.target] = true;
                    stack.push(link.target);
                }
            }
        }
        for &id in &reachable {
            for link in &object(id)?.links {
                parents[link.target] += 1;
            }
        }
        let mut ready = BinaryHeap::new();
        ready.push(Reverse((object(root)?.tier, order[root], root)));
        let mut position = vec![usize::MAX; self.objects.len()];
        let mut out = Vec::new();
        while let Some(Reverse((_, _, id))) = ready.pop() {
            position[id] = out.len();
            out.extend_from_slice(&object(id)?.data);
            for link in &object(id)?.links {
                parents[link.target] -= 1;
                if parents[link.target] == 0 {
                    let target = object(link.target)?;
                    ready.push(Reverse((target.tier, order[link.target], link.target)));
                }
            }
        }
        if reachable.iter().any(|&id| position[id] == usize::MAX) {
            return Err(SubsetError::Malformed("cyclic table graph"));
        }
        for &id in &reachable {
            for link in &object(id)?.links {
                let base = position[link.base.unwrap_or(id)];
                let target = position[link.target];
                let offset = target.checked_sub(base).ok_or(SubsetError::Overflow)?;
                let at = position[id] + link.at;
                match link.width {
                    2 => {
                        let offset = u16::try_from(offset).map_err(|_| SubsetError::Overflow)?;
                        out[at..at + 2].copy_from_slice(&offset.to_be_bytes());
                    }
                    _ => {
                        let offset = u32::try_from(offset).map_err(|_| SubsetError::Overflow)?;
                        out[at..at + 4].copy_from_slice(&offset.to_be_bytes());
                    }
                }
            }
        }
        Ok(out)
    }
}
