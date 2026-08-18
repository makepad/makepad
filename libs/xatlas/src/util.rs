//! Containers, hash, sort, RNG, BVH, and the single-threaded scheduler.
//! From `vendor/xatlas.cpp` (Array / HashMap / Bit* / BVH / KISSRng / RadixSort / TaskScheduler).

use crate::math::*;

pub const UINT32_MAX: u32 = u32::MAX;

// xatlas.cpp:1906
pub fn sdbm_hash(data: &[u8], mut h: u32) -> u32 {
    for &b in data {
        h = h
            .wrapping_shl(16)
            .wrapping_add(h.wrapping_shl(6))
            .wrapping_sub(h)
            .wrapping_add(b as u32);
    }
    h
}

pub fn hash_bytes(data: &[u8]) -> u32 {
    sdbm_hash(data, 5381)
}

pub fn insertion_sort<T: Copy + PartialOrd>(data: &mut [T]) {
    for i in 1..data.len() as i32 {
        let x = data[i as usize];
        let mut j = i - 1;
        while j >= 0 && x < data[j as usize] {
            data[(j + 1) as usize] = data[j as usize];
            j -= 1;
        }
        data[(j + 1) as usize] = x;
    }
}

/// Insertion-order multimap matching xatlas `HashMap` (xatlas.cpp:1940).
pub struct HashMap<K> {
    size_hint: u32,
    num_slots: u32,
    slots: Vec<u32>,
    keys: Vec<K>,
    next: Vec<u32>,
    hash_fn: fn(&K) -> u32,
    eq_fn: fn(&K, &K) -> bool,
}

impl<K: Clone> HashMap<K> {
    pub fn new(size: u32, hash_fn: fn(&K) -> u32, eq_fn: fn(&K, &K) -> bool) -> Self {
        Self {
            size_hint: size,
            num_slots: 0,
            slots: Vec::new(),
            keys: Vec::new(),
            next: Vec::new(),
            hash_fn,
            eq_fn,
        }
    }

    pub fn destroy(&mut self) {
        self.slots.clear();
        self.keys.clear();
        self.next.clear();
        self.num_slots = 0;
    }

    fn alloc(&mut self) {
        debug_assert!(self.size_hint > 0);
        self.num_slots = next_power_of_two(self.size_hint);
        let min_num_slots = (self.size_hint as f32 * 1.3) as u32;
        if self.num_slots < min_num_slots {
            self.num_slots = next_power_of_two(min_num_slots);
        }
        self.slots = vec![UINT32_MAX; self.num_slots as usize];
        self.keys.reserve(self.size_hint as usize);
        self.next.reserve(self.size_hint as usize);
    }

    fn compute_hash(&self, key: &K) -> u32 {
        (self.hash_fn)(key) & (self.num_slots - 1)
    }

    fn find(&self, key: &K, mut current: u32) -> u32 {
        while current != UINT32_MAX {
            if (self.eq_fn)(&self.keys[current as usize], key) {
                return current;
            }
            current = self.next[current as usize];
        }
        current
    }

    pub fn add(&mut self, key: K) -> u32 {
        if self.slots.is_empty() {
            self.alloc();
        }
        let hash = self.compute_hash(&key);
        self.keys.push(key);
        self.next.push(self.slots[hash as usize]);
        let idx = self.next.len() as u32 - 1;
        self.slots[hash as usize] = idx;
        self.keys.len() as u32 - 1
    }

    pub fn get(&self, key: &K) -> u32 {
        if self.slots.is_empty() {
            return UINT32_MAX;
        }
        self.find(key, self.slots[self.compute_hash(key) as usize])
    }

    pub fn get_next(&self, key: &K, current: u32) -> u32 {
        self.find(key, self.next[current as usize])
    }

    pub fn key(&self, i: u32) -> &K {
        &self.keys[i as usize]
    }
}

pub fn hash_u32(k: &u32) -> u32 {
    *k
}

pub fn eq_u32(a: &u32, b: &u32) -> bool {
    a == b
}

pub fn hash_vec3(k: &Vec3) -> u32 {
    hash_bytes(&k.as_bytes())
}

pub fn eq_vec3(a: &Vec3, b: &Vec3) -> bool {
    a == b
}

pub fn hash_vec2(k: &Vec2) -> u32 {
    hash_bytes(&k.as_bytes())
}

pub fn eq_vec2(a: &Vec2, b: &Vec2) -> bool {
    a == b
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeKey {
    pub v0: u32,
    pub v1: u32,
}

impl EdgeKey {
    pub fn new(v0: u32, v1: u32) -> Self {
        Self { v0, v1 }
    }
}

// xatlas.cpp:2371
pub fn hash_edge(k: &EdgeKey) -> u32 {
    k.v0.wrapping_mul(32768).wrapping_add(k.v1)
}

pub fn eq_edge(a: &EdgeKey, b: &EdgeKey) -> bool {
    a.v0 == b.v0 && a.v1 == b.v1
}

#[derive(Clone, Default)]
pub struct BitArray {
    size: u32,
    word_array: Vec<u32>,
}

impl BitArray {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_size(sz: u32) -> Self {
        let mut a = Self::new();
        a.resize(sz);
        a
    }

    pub fn resize(&mut self, new_size: u32) {
        self.size = new_size;
        self.word_array.resize(((self.size + 31) >> 5) as usize, 0);
    }

    pub fn get(&self, index: u32) -> bool {
        debug_assert!(index < self.size);
        (self.word_array[(index >> 5) as usize] & (1u32 << (index & 31))) != 0
    }

    pub fn set(&mut self, index: u32) {
        debug_assert!(index < self.size);
        self.word_array[(index >> 5) as usize] |= 1u32 << (index & 31);
    }

    pub fn unset(&mut self, index: u32) {
        debug_assert!(index < self.size);
        self.word_array[(index >> 5) as usize] &= !(1u32 << (index & 31));
    }

    pub fn zero_out_memory(&mut self) {
        for w in &mut self.word_array {
            *w = 0;
        }
    }
}

#[derive(Clone, Default)]
pub struct BitImage {
    width: u32,
    height: u32,
    row_stride: u32,
    data: Vec<u64>,
}

impl BitImage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_size(w: u32, h: u32) -> Self {
        let row_stride = (w + 63) >> 6;
        Self {
            width: w,
            height: h,
            row_stride,
            data: vec![0u64; (row_stride * h) as usize],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn copy_to(&self, other: &mut BitImage) {
        other.width = self.width;
        other.height = self.height;
        other.row_stride = self.row_stride;
        other.data = self.data.clone();
    }

    pub fn resize(&mut self, w: u32, h: u32, discard: bool) {
        let row_stride = (w + 63) >> 6;
        if discard {
            self.data.clear();
            self.data.resize((row_stride * h) as usize, 0);
        } else {
            let mut tmp = vec![0u64; (row_stride * h) as usize];
            if row_stride == self.row_stride {
                let n = self.row_stride * self.height.min(h);
                tmp[..n as usize].copy_from_slice(&self.data[..n as usize]);
            } else if self.width > 0 && self.height > 0 {
                let height = self.height.min(h);
                let copy_stride = row_stride.min(self.row_stride);
                for i in 0..height {
                    let dst = (i * row_stride) as usize;
                    let src = (i * self.row_stride) as usize;
                    tmp[dst..dst + copy_stride as usize]
                        .copy_from_slice(&self.data[src..src + copy_stride as usize]);
                }
            }
            self.data = tmp;
        }
        self.width = w;
        self.height = h;
        self.row_stride = row_stride;
    }

    pub fn get(&self, x: u32, y: u32) -> bool {
        debug_assert!(x < self.width && y < self.height);
        let index = (x >> 6) + y * self.row_stride;
        (self.data[index as usize] & (1u64 << (x as u64 & 63))) != 0
    }

    pub fn set(&mut self, x: u32, y: u32) {
        debug_assert!(x < self.width && y < self.height);
        let index = (x >> 6) + y * self.row_stride;
        self.data[index as usize] |= 1u64 << (x as u64 & 63);
    }

    pub fn zero_out_memory(&mut self) {
        for w in &mut self.data {
            *w = 0;
        }
    }

    // xatlas.cpp:1403
    pub fn can_blit(&self, image: &BitImage, offset_x: u32, offset_y: u32) -> bool {
        for y in 0..image.height {
            let this_y = y + offset_y;
            if this_y >= self.height {
                continue;
            }
            let mut x = 0u32;
            loop {
                let this_x = x + offset_x;
                if this_x >= self.width {
                    break;
                }
                let this_block_shift = this_x % 64;
                let this_block =
                    self.data[((this_x >> 6) + this_y * self.row_stride) as usize] >> this_block_shift;
                let block_shift = x % 64;
                let block =
                    image.data[((x >> 6) + y * image.row_stride) as usize] >> block_shift;
                if (this_block & block) != 0 {
                    return false;
                }
                x += 64 - this_block_shift.max(block_shift);
                if x >= image.width {
                    break;
                }
            }
        }
        true
    }

    pub fn dilate(&mut self, padding: u32) {
        let mut tmp = BitImage::with_size(self.width, self.height);
        for _ in 0..padding {
            tmp.zero_out_memory();
            for y in 0..self.height {
                for x in 0..self.width {
                    let mut b = self.get(x, y);
                    if !b {
                        if x > 0 {
                            b |= self.get(x - 1, y);
                            if y > 0 {
                                b |= self.get(x - 1, y - 1);
                            }
                            if y < self.height - 1 {
                                b |= self.get(x - 1, y + 1);
                            }
                        }
                        if y > 0 {
                            b |= self.get(x, y - 1);
                        }
                        if y < self.height - 1 {
                            b |= self.get(x, y + 1);
                        }
                        if x < self.width - 1 {
                            b |= self.get(x + 1, y);
                            if y > 0 {
                                b |= self.get(x + 1, y - 1);
                            }
                            if y < self.height - 1 {
                                b |= self.get(x + 1, y + 1);
                            }
                        }
                    }
                    if b {
                        tmp.set(x, y);
                    }
                }
            }
            self.data.clone_from(&tmp.data);
        }
    }
}

// xatlas.cpp:1466
pub struct Bvh {
    object_aabbs: Vec<Aabb>,
    object_ids: Vec<u32>,
    nodes: Vec<BvhNode>,
}

struct BvhNode {
    aabb: Aabb,
    start: u32,
    n_prims: u32,
    right_offset: u32,
}

#[derive(Clone, Copy)]
struct BvhBuildEntry {
    parent: u32,
    start: u32,
    end: u32,
}

impl Bvh {
    pub fn new(object_aabbs: &[Aabb], leaf_size: u32) -> Self {
        let mut s = Self {
            object_aabbs: object_aabbs.to_vec(),
            object_ids: Vec::new(),
            nodes: Vec::new(),
        };
        if object_aabbs.is_empty() {
            return s;
        }
        s.object_ids = (0..object_aabbs.len() as u32).collect();
        let mut todo = [BvhBuildEntry {
            parent: 0,
            start: 0,
            end: 0,
        }; 128];
        let mut stackptr = 0u32;
        const K_ROOT: u32 = 0xfffffffc;
        const K_UNTOUCHED: u32 = 0xffffffff;
        const K_TOUCHED_TWICE: u32 = 0xfffffffd;
        todo[stackptr as usize] = BvhBuildEntry {
            parent: K_ROOT,
            start: 0,
            end: object_aabbs.len() as u32,
        };
        stackptr += 1;
        s.nodes.reserve(object_aabbs.len() * 2);
        let mut n_nodes = 0u32;
        while stackptr > 0 {
            stackptr -= 1;
            let bnode = todo[stackptr as usize];
            let start = bnode.start;
            let end = bnode.end;
            let n_prims = end - start;
            n_nodes += 1;
            let mut node = BvhNode {
                aabb: Aabb::default(),
                start,
                n_prims,
                right_offset: K_UNTOUCHED,
            };
            let mut bb = object_aabbs[s.object_ids[start as usize] as usize];
            let mut bc = Aabb::from_point_radius(object_aabbs[s.object_ids[start as usize] as usize].centroid(), 0.0);
            for p in start + 1..end {
                bb.expand_to_include_aabb(object_aabbs[s.object_ids[p as usize] as usize]);
                bc.expand_to_include_point(object_aabbs[s.object_ids[p as usize] as usize].centroid());
            }
            node.aabb = bb;
            if n_prims <= leaf_size {
                node.right_offset = 0;
            }
            s.nodes.push(node);
            if bnode.parent != K_ROOT {
                s.nodes[bnode.parent as usize].right_offset -= 1;
                if s.nodes[bnode.parent as usize].right_offset == K_TOUCHED_TWICE {
                    s.nodes[bnode.parent as usize].right_offset = n_nodes - 1 - bnode.parent;
                }
            }
            if s.nodes.last().unwrap().right_offset == 0 {
                continue;
            }
            let split_dim = bc.max_dimension();
            let split_coord = 0.5 * (bc.min.axis(split_dim) + bc.max.axis(split_dim));
            let mut mid = start;
            for i in start..end {
                let centroid = object_aabbs[s.object_ids[i as usize] as usize].centroid();
                if centroid.axis(split_dim) < split_coord {
                    s.object_ids.swap(i as usize, mid as usize);
                    mid += 1;
                }
            }
            if mid == start || mid == end {
                mid = start + (end - start) / 2;
            }
            todo[stackptr as usize] = BvhBuildEntry {
                parent: n_nodes - 1,
                start: mid,
                end,
            };
            stackptr += 1;
            todo[stackptr as usize] = BvhBuildEntry {
                parent: n_nodes - 1,
                start,
                end: mid,
            };
            stackptr += 1;
        }
        s
    }

    pub fn query(&self, query_aabb: Aabb, result: &mut Vec<u32>) {
        result.clear();
        if self.nodes.is_empty() {
            return;
        }
        let mut todo = [0u32; 64];
        let mut stackptr: i32 = 0;
        todo[0] = 0;
        while stackptr >= 0 {
            let ni = todo[stackptr as usize] as usize;
            stackptr -= 1;
            let node = &self.nodes[ni];
            if node.right_offset == 0 {
                for o in 0..node.n_prims {
                    let obj = node.start + o;
                    if query_aabb.intersect(self.object_aabbs[self.object_ids[obj as usize] as usize]) {
                        result.push(self.object_ids[obj as usize]);
                    }
                }
            } else {
                let left = ni as u32 + 1;
                let right = ni as u32 + node.right_offset;
                if query_aabb.intersect(self.nodes[left as usize].aabb) {
                    stackptr += 1;
                    todo[stackptr as usize] = left;
                }
                if query_aabb.intersect(self.nodes[right as usize].aabb) {
                    stackptr += 1;
                    todo[stackptr as usize] = right;
                }
            }
        }
    }
}

// xatlas.cpp:2041
pub struct KissRng {
    x: u32,
    y: u32,
    z: u32,
    c: u32,
}

impl Default for KissRng {
    fn default() -> Self {
        let mut r = Self {
            x: 0,
            y: 0,
            z: 0,
            c: 0,
        };
        r.reset();
        r
    }
}

impl KissRng {
    pub fn reset(&mut self) {
        self.x = 123456789;
        self.y = 362436000;
        self.z = 521288629;
        self.c = 7654321;
    }

    pub fn get_range(&mut self, range: u32) -> u32 {
        if range == 0 {
            return 0;
        }
        self.x = 69069u32.wrapping_mul(self.x).wrapping_add(12345);
        self.y ^= self.y << 13;
        self.y ^= self.y >> 17;
        self.y ^= self.y << 5;
        let t = 698769069u64.wrapping_mul(self.z as u64).wrapping_add(self.c as u64);
        self.c = (t >> 32) as u32;
        self.z = t as u32;
        self.x
            .wrapping_add(self.y)
            .wrapping_add(self.z)
            % (range + 1)
    }
}

// xatlas.cpp:2075
pub struct RadixSort {
    buffer1: Vec<u32>,
    buffer2: Vec<u32>,
    use_buffer1: bool,
    valid_ranks: bool,
}

impl Default for RadixSort {
    fn default() -> Self {
        Self {
            buffer1: Vec::new(),
            buffer2: Vec::new(),
            use_buffer1: true,
            valid_ranks: false,
        }
    }
}

impl RadixSort {
    pub fn sort(&mut self, input: &mut [f32]) {
        if input.is_empty() {
            self.buffer1.clear();
            self.buffer2.clear();
            self.valid_ranks = false;
            return;
        }
        let n = input.len();
        self.buffer1.resize(n, 0);
        self.buffer2.resize(n, 0);
        self.use_buffer1 = true;
        self.valid_ranks = false;
        if n < 32 {
            self.insertion_sort(input);
        } else {
            for v in input.iter_mut() {
                *v = f32::from_bits(float_flip(v.to_bits()));
            }
            self.radix_sort_u32(input);
            for v in input.iter_mut() {
                *v = f32::from_bits(ifloat_flip(v.to_bits()));
            }
        }
    }

    pub fn ranks(&self) -> &[u32] {
        debug_assert!(self.valid_ranks);
        if self.use_buffer1 {
            &self.buffer1
        } else {
            &self.buffer2
        }
    }

    fn ranks_mut(&mut self) -> &mut [u32] {
        if self.use_buffer1 {
            &mut self.buffer1
        } else {
            &mut self.buffer2
        }
    }

    fn ranks2_mut(&mut self) -> &mut [u32] {
        if self.use_buffer1 {
            &mut self.buffer2
        } else {
            &mut self.buffer1
        }
    }

    fn insertion_sort(&mut self, input: &[f32]) {
        let n = input.len();
        if !self.valid_ranks {
            self.buffer1[0] = 0;
            for i in 1..n {
                let rank = i as u32;
                self.buffer1[i] = rank;
                let mut j = i;
                while j != 0 && input[rank as usize] < input[self.buffer1[j - 1] as usize] {
                    self.buffer1[j] = self.buffer1[j - 1];
                    j -= 1;
                }
                if i != j {
                    self.buffer1[j] = rank;
                }
            }
            self.use_buffer1 = true;
            self.valid_ranks = true;
        } else {
            let ranks = if self.use_buffer1 {
                &mut self.buffer1
            } else {
                &mut self.buffer2
            };
            for i in 1..n {
                let rank = ranks[i];
                let mut j = i;
                while j != 0 && input[rank as usize] < input[ranks[j - 1] as usize] {
                    ranks[j] = ranks[j - 1];
                    j -= 1;
                }
                if i != j {
                    ranks[j] = rank;
                }
            }
        }
    }

    fn radix_sort_u32(&mut self, input_as_float: &[f32]) {
        let n = input_as_float.len();
        let input: Vec<u32> = input_as_float.iter().map(|f| f.to_bits()).collect();
        let mut histogram = [0u32; 256 * 4];
        create_histograms(&input, &mut histogram);
        for j in 0..4 {
            let h = &histogram[j * 256..(j + 1) * 256];
            let first_byte = ((input[0] >> (8 * j)) & 0xff) as usize;
            if h[first_byte] == n as u32 {
                continue;
            }
            // Build output into ranks2 from current ranks.
            let mut offsets = [0usize; 256];
            let mut acc = 0usize;
            for i in 0..256 {
                offsets[i] = acc;
                acc += h[i] as usize;
            }
            let mut out = vec![0u32; n];
            if !self.valid_ranks {
                for i in 0..n {
                    let b = ((input[i] >> (8 * j)) & 0xff) as usize;
                    out[offsets[b]] = i as u32;
                    offsets[b] += 1;
                }
                self.valid_ranks = true;
            } else {
                let ranks = if self.use_buffer1 {
                    self.buffer1.clone()
                } else {
                    self.buffer2.clone()
                };
                for i in 0..n {
                    let idx = ranks[i] as usize;
                    let b = ((input[idx] >> (8 * j)) & 0xff) as usize;
                    out[offsets[b]] = idx as u32;
                    offsets[b] += 1;
                }
            }
            if self.use_buffer1 {
                self.buffer2 = out;
                self.use_buffer1 = false;
            } else {
                self.buffer1 = out;
                self.use_buffer1 = true;
            }
        }
        if !self.valid_ranks {
            for i in 0..n {
                self.buffer1[i] = i as u32;
            }
            self.use_buffer1 = true;
            self.valid_ranks = true;
        }
    }
}

fn float_flip(f: u32) -> u32 {
    // xatlas.cpp:2118 — `int32_t mask = (int32_t(f) >> 31) | 0x80000000`
    let mask = ((f as i32) >> 31) as u32 | 0x8000_0000;
    f ^ mask
}

fn ifloat_flip(f: u32) -> u32 {
    // xatlas.cpp:2125
    let mask = (f >> 31).wrapping_sub(1) | 0x8000_0000;
    f ^ mask
}

fn create_histograms(input: &[u32], histogram: &mut [u32]) {
    histogram.fill(0);
    for &v in input {
        let b = v.to_le_bytes();
        histogram[b[0] as usize] += 1;
        histogram[256 + b[1] as usize] += 1;
        histogram[512 + b[2] as usize] += 1;
        histogram[768 + b[3] as usize] += 1;
    }
}

// xatlas.cpp:2225
#[derive(Default)]
pub struct BoundingBox2D {
    pub major_axis: Vec2,
    pub minor_axis: Vec2,
    pub min_corner: Vec2,
    pub max_corner: Vec2,
    boundary_vertices: Vec<Vec2>,
    coords: Vec<f32>,
    top: Vec<Vec2>,
    bottom: Vec<Vec2>,
    hull: Vec<Vec2>,
    radix: RadixSort,
}

impl BoundingBox2D {
    pub fn clear(&mut self) {
        self.boundary_vertices.clear();
    }

    pub fn append_boundary_vertex(&mut self, v: Vec2) {
        self.boundary_vertices.push(v);
    }

    pub fn compute(&mut self, vertices: Option<&[Vec2]>) {
        debug_assert!(!self.boundary_vertices.is_empty());
        let verts: Vec<Vec2> = match vertices {
            Some(v) if !v.is_empty() => v.to_vec(),
            _ => self.boundary_vertices.clone(),
        };
        self.convex_hull(&self.boundary_vertices.clone(), 0.00001);
        let mut best_area = f32::MAX;
        let mut best_min = Vec2::splat(0.0);
        let mut best_max = Vec2::splat(0.0);
        let mut best_axis = Vec2::splat(0.0);
        let hull_count = self.hull.len();
        let mut j = hull_count.wrapping_sub(1);
        for i in 0..hull_count {
            if equal2(self.hull[i], self.hull[j], EPSILON) {
                j = i;
                continue;
            }
            let axis = normalize2(self.hull[i] - self.hull[j]);
            debug_assert!(is_finite2(axis));
            let mut box_min = Vec2::new(f32::MAX, f32::MAX);
            let mut box_max = Vec2::new(-f32::MAX, -f32::MAX);
            for &point in &verts {
                let x = dot2(axis, point);
                let y = dot2(Vec2::new(-axis.y, axis.x), point);
                box_min.x = box_min.x.min(x);
                box_max.x = box_max.x.max(x);
                box_min.y = box_min.y.min(y);
                box_max.y = box_max.y.max(y);
            }
            let area = (box_max.x - box_min.x) * (box_max.y - box_min.y);
            if area < best_area {
                best_area = area;
                best_min = box_min;
                best_max = box_max;
                best_axis = axis;
            }
            j = i;
        }
        self.major_axis = best_axis;
        self.minor_axis = Vec2::new(-best_axis.y, best_axis.x);
        self.min_corner = best_min;
        self.max_corner = best_max;
    }

    fn convex_hull(&mut self, input: &[Vec2], epsilon: f32) {
        self.coords.resize(input.len(), 0.0);
        for i in 0..input.len() {
            self.coords[i] = input[i].x;
        }
        self.radix.sort(&mut self.coords);
        let ranks: Vec<u32> = self.radix.ranks().to_vec();
        self.top.clear();
        self.bottom.clear();
        self.top.reserve(input.len());
        self.bottom.reserve(input.len());
        let p = input[ranks[0] as usize];
        let q = input[ranks[input.len() - 1] as usize];
        let topy = p.y.max(q.y);
        let boty = p.y.min(q.y);
        for i in 0..input.len() {
            let pt = input[ranks[i] as usize];
            if pt.y >= boty {
                self.top.push(pt);
            }
        }
        for i in 0..input.len() {
            let pt = input[ranks[input.len() - 1 - i] as usize];
            if pt.y <= topy {
                self.bottom.push(pt);
            }
        }
        self.hull.clear();
        debug_assert!(self.top.len() >= 2);
        self.hull.push(self.top[0]);
        self.hull.push(self.top[1]);
        let mut i = 2usize;
        while i < self.top.len() {
            let a = self.hull[self.hull.len() - 2];
            let b = self.hull[self.hull.len() - 1];
            let c = self.top[i];
            let area = triangle_area2(a, b, c);
            if area >= -epsilon {
                self.hull.pop();
            }
            if area < -epsilon || self.hull.len() == 1 {
                self.hull.push(c);
                i += 1;
            }
        }
        let top_count = self.hull.len();
        debug_assert!(self.bottom.len() >= 2);
        self.hull.push(self.bottom[1]);
        i = 2;
        while i < self.bottom.len() {
            let a = self.hull[self.hull.len() - 2];
            let b = self.hull[self.hull.len() - 1];
            let c = self.bottom[i];
            let area = triangle_area2(a, b, c);
            if area >= -epsilon {
                self.hull.pop();
            }
            if area < -epsilon || self.hull.len() == top_count {
                self.hull.push(c);
                i += 1;
            }
        }
        debug_assert!(!self.hull.is_empty());
        self.hull.pop();
    }
}

// xatlas.cpp:3304 — single-threaded TaskScheduler.
pub struct TaskGroupHandle {
    pub value: u32,
}

impl Default for TaskGroupHandle {
    fn default() -> Self {
        Self { value: UINT32_MAX }
    }
}

type TaskFn = Box<dyn FnOnce(&mut TaskScheduler)>;

struct TaskGroup {
    tasks: Vec<TaskFn>,
}

#[derive(Default)]
pub struct TaskScheduler {
    groups: Vec<Option<TaskGroup>>,
}

impl TaskScheduler {
    pub fn thread_count(&self) -> u32 {
        1
    }

    pub fn create_task_group(&mut self) -> TaskGroupHandle {
        self.groups.push(Some(TaskGroup { tasks: Vec::new() }));
        TaskGroupHandle {
            value: self.groups.len() as u32 - 1,
        }
    }

    pub fn run<F>(&mut self, handle: TaskGroupHandle, f: F)
    where
        F: FnOnce(&mut TaskScheduler) + 'static,
    {
        if let Some(g) = self.groups[handle.value as usize].as_mut() {
            g.tasks.push(Box::new(f));
        }
    }

    pub fn wait(&mut self, handle: &mut TaskGroupHandle) {
        if handle.value == UINT32_MAX {
            debug_assert!(false);
            return;
        }
        let idx = handle.value as usize;
        let group = self.groups[idx].take().unwrap();
        handle.value = UINT32_MAX;
        for task in group.tasks {
            task(self);
        }
    }
}

pub fn mesh_edge_face(edge: u32) -> u32 {
    edge / 3
}

pub fn mesh_edge_index0(edge: u32) -> u32 {
    edge
}

pub fn mesh_edge_index1(edge: u32) -> u32 {
    let face_first_edge = edge / 3 * 3;
    face_first_edge + (edge - face_first_edge + 1) % 3
}
