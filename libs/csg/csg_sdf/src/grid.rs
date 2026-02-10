use crate::sdf::Sdf3;
use makepad_csg_math::Vec3d;
use std::sync::Arc;

/// Hierarchical octree grid that caches SDF distance evaluations for dual contouring.
/// Normals are NOT stored — they are computed lazily only at sign-change edges.
pub struct SdfGrid3 {
    pub root: Node,
    pub min: Vec3d,
    pub max: Vec3d,
}

impl SdfGrid3 {
    pub fn from_sdf(
        sdf: Arc<dyn Sdf3 + Send + Sync>,
        min: Vec3d,
        max: Vec3d,
        max_depth: usize,
    ) -> SdfGrid3 {
        let corners = cube_corners(min, max);
        let distances = corners.map(|c| sdf.distance(c));

        let tc = makepad_csg_math::thread_count();
        let root = if max_depth >= 2 && tc > 8 {
            // 64 tasks for >8 cores — better load balancing
            build_parallel_depth2(sdf, min, max, distances, max_depth)
        } else if max_depth >= 1 && tc > 1 {
            // 8 tasks for 2-8 cores
            build_parallel_depth1(sdf, min, max, distances, max_depth)
        } else {
            Node::from_sdf_seq(sdf.as_ref(), min, max, distances, 0, max_depth)
        };

        SdfGrid3 { root, min, max }
    }
}

pub enum Node {
    Leaf { distances: [f64; 8] },
    Branch { children: Box<[Node; 8]> },
}

fn cube_corners(min: Vec3d, max: Vec3d) -> [Vec3d; 8] {
    [
        Vec3d::new(min.x, min.y, min.z),
        Vec3d::new(max.x, min.y, min.z),
        Vec3d::new(min.x, max.y, min.z),
        Vec3d::new(max.x, max.y, min.z),
        Vec3d::new(min.x, min.y, max.z),
        Vec3d::new(max.x, min.y, max.z),
        Vec3d::new(min.x, max.y, max.z),
        Vec3d::new(max.x, max.y, max.z),
    ]
}

/// Build top level sequentially, then dispatch 8 subtrees to the pool.
fn build_parallel_depth1(
    sdf: Arc<dyn Sdf3 + Send + Sync>,
    min: Vec3d,
    max: Vec3d,
    distances: [f64; 8],
    max_depth: usize,
) -> Node {
    let mid = min.lerp(max, 0.5);
    let child_data = compute_child_data(sdf.as_ref(), min, mid, max, distances);

    let tasks: Vec<Box<dyn FnOnce() -> Node + Send>> = child_data
        .into_iter()
        .map(|(cmin, cmax, cdist)| {
            let sdf = sdf.clone();
            let f: Box<dyn FnOnce() -> Node + Send> =
                Box::new(move || Node::from_sdf_seq(sdf.as_ref(), cmin, cmax, cdist, 1, max_depth));
            f
        })
        .collect();

    let results = makepad_csg_math::parallel_for(tasks);
    let mut iter = results.into_iter();
    Node::Branch {
        children: Box::new([
            iter.next().unwrap(),
            iter.next().unwrap(),
            iter.next().unwrap(),
            iter.next().unwrap(),
            iter.next().unwrap(),
            iter.next().unwrap(),
            iter.next().unwrap(),
            iter.next().unwrap(),
        ]),
    }
}

/// Build the top 2 levels sequentially, then dispatch 64 subtrees to the pool.
fn build_parallel_depth2(
    sdf: Arc<dyn Sdf3 + Send + Sync>,
    min: Vec3d,
    max: Vec3d,
    distances: [f64; 8],
    max_depth: usize,
) -> Node {
    let mid0 = min.lerp(max, 0.5);
    let depth0_children = compute_child_data(sdf.as_ref(), min, mid0, max, distances);

    // For each depth-0 child, compute its 8 depth-1 grandchildren descriptions
    // (19 new samples each, sequential — total ~152 distance evals, cheap)
    let mut tasks: Vec<Box<dyn FnOnce() -> Node + Send>> = Vec::with_capacity(64);
    let mut depth1_data: Vec<(usize, [(Vec3d, Vec3d, [f64; 8]); 8])> = Vec::with_capacity(8);

    for (i, (cmin, cmax, cdist)) in depth0_children.iter().enumerate() {
        let cmid = cmin.lerp(*cmax, 0.5);
        let grandchildren = compute_child_data(sdf.as_ref(), *cmin, cmid, *cmax, *cdist);
        depth1_data.push((i, grandchildren));
    }

    // Now flatten all 64 grandchild subtrees into tasks
    for (_parent_i, grandchildren) in &depth1_data {
        for (gcmin, gcmax, gcdist) in grandchildren {
            let sdf = sdf.clone();
            let gcmin = *gcmin;
            let gcmax = *gcmax;
            let gcdist = *gcdist;
            let md = max_depth;
            tasks.push(Box::new(move || {
                Node::from_sdf_seq(sdf.as_ref(), gcmin, gcmax, gcdist, 2, md)
            }));
        }
    }

    // Dispatch all 64 tasks to the pool
    let results = makepad_csg_math::parallel_for(tasks);

    // Reassemble: 64 results → 8 groups of 8 → 8 Branch nodes → 1 root Branch
    let mut result_iter = results.into_iter();
    let mut root_children: Vec<Node> = Vec::with_capacity(8);
    for _ in 0..8 {
        let gc: [Node; 8] = [
            result_iter.next().unwrap(),
            result_iter.next().unwrap(),
            result_iter.next().unwrap(),
            result_iter.next().unwrap(),
            result_iter.next().unwrap(),
            result_iter.next().unwrap(),
            result_iter.next().unwrap(),
            result_iter.next().unwrap(),
        ];
        root_children.push(Node::Branch {
            children: Box::new(gc),
        });
    }

    Node::Branch {
        children: Box::new([
            root_children.remove(0),
            root_children.remove(0),
            root_children.remove(0),
            root_children.remove(0),
            root_children.remove(0),
            root_children.remove(0),
            root_children.remove(0),
            root_children.remove(0),
        ]),
    }
}

impl Node {
    /// Build node sequentially (used by pool workers and for small trees).
    fn from_sdf_seq(
        sdf: &(dyn Sdf3 + Send + Sync),
        min: Vec3d,
        max: Vec3d,
        distances: [f64; 8],
        depth: usize,
        max_depth: usize,
    ) -> Node {
        if depth == max_depth {
            return Node::Leaf { distances };
        }

        let mid = min.lerp(max, 0.5);
        let child_data = compute_child_data(sdf, min, mid, max, distances);
        let children = child_data.map(|(cmin, cmax, cdist)| {
            Node::from_sdf_seq(sdf, cmin, cmax, cdist, depth + 1, max_depth)
        });
        Node::Branch {
            children: Box::new(children),
        }
    }
}

/// Compute the 19 new distance samples and return the 8 child (min, max, distances) tuples.
fn compute_child_data(
    sdf: &(dyn Sdf3 + Send + Sync),
    min: Vec3d,
    mid: Vec3d,
    max: Vec3d,
    distances: [f64; 8],
) -> [(Vec3d, Vec3d, [f64; 8]); 8] {
    let [d000, d002, d020, d022, d200, d202, d220, d222] = distances;

    let d001 = sdf.distance(Vec3d::new(mid.x, min.y, min.z));
    let d010 = sdf.distance(Vec3d::new(min.x, mid.y, min.z));
    let d011 = sdf.distance(Vec3d::new(mid.x, mid.y, min.z));
    let d012 = sdf.distance(Vec3d::new(max.x, mid.y, min.z));
    let d021 = sdf.distance(Vec3d::new(mid.x, max.y, min.z));
    let d100 = sdf.distance(Vec3d::new(min.x, min.y, mid.z));
    let d101 = sdf.distance(Vec3d::new(mid.x, min.y, mid.z));
    let d102 = sdf.distance(Vec3d::new(max.x, min.y, mid.z));
    let d110 = sdf.distance(Vec3d::new(min.x, mid.y, mid.z));
    let d111 = sdf.distance(Vec3d::new(mid.x, mid.y, mid.z));
    let d112 = sdf.distance(Vec3d::new(max.x, mid.y, mid.z));
    let d120 = sdf.distance(Vec3d::new(min.x, max.y, mid.z));
    let d121 = sdf.distance(Vec3d::new(mid.x, max.y, mid.z));
    let d122 = sdf.distance(Vec3d::new(max.x, max.y, mid.z));
    let d201 = sdf.distance(Vec3d::new(mid.x, min.y, max.z));
    let d210 = sdf.distance(Vec3d::new(min.x, mid.y, max.z));
    let d211 = sdf.distance(Vec3d::new(mid.x, mid.y, max.z));
    let d212 = sdf.distance(Vec3d::new(max.x, mid.y, max.z));
    let d221 = sdf.distance(Vec3d::new(mid.x, max.y, max.z));

    [
        (
            Vec3d::new(min.x, min.y, min.z),
            Vec3d::new(mid.x, mid.y, mid.z),
            [d000, d001, d010, d011, d100, d101, d110, d111],
        ),
        (
            Vec3d::new(mid.x, min.y, min.z),
            Vec3d::new(max.x, mid.y, mid.z),
            [d001, d002, d011, d012, d101, d102, d111, d112],
        ),
        (
            Vec3d::new(min.x, mid.y, min.z),
            Vec3d::new(mid.x, max.y, mid.z),
            [d010, d011, d020, d021, d110, d111, d120, d121],
        ),
        (
            Vec3d::new(mid.x, mid.y, min.z),
            Vec3d::new(max.x, max.y, mid.z),
            [d011, d012, d021, d022, d111, d112, d121, d122],
        ),
        (
            Vec3d::new(min.x, min.y, mid.z),
            Vec3d::new(mid.x, mid.y, max.z),
            [d100, d101, d110, d111, d200, d201, d210, d211],
        ),
        (
            Vec3d::new(mid.x, min.y, mid.z),
            Vec3d::new(max.x, mid.y, max.z),
            [d101, d102, d111, d112, d201, d202, d211, d212],
        ),
        (
            Vec3d::new(min.x, mid.y, mid.z),
            Vec3d::new(mid.x, max.y, max.z),
            [d110, d111, d120, d121, d210, d211, d220, d221],
        ),
        (
            Vec3d::new(mid.x, mid.y, mid.z),
            Vec3d::new(max.x, max.y, max.z),
            [d111, d112, d121, d122, d211, d212, d221, d222],
        ),
    ]
}
