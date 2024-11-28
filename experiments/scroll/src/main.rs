use {btree_vec::Metric, std::time::Instant};

// This file illustrates how to leverage the `BTreeVec` type provided by the `btree_vec` crate.
//
// Conceptually, a `BTreeVec` is a sequence, like a `Vec`. However, a `BTreeVec` is implemented as a
// balanced tree, which makes it much more performant for large number of items. More specifically,
// a `BTreeVec` is implemented as a btree (hence the name). BTrees are much more cache efficient than
// binary trees, because each leaf node contains up to 1024 items in a contiguous region of memory,
// and each non-leaf node contains between 4-8 children. The latter is in fact an invariant maintained
// by the btree, which ensures that it remains balanced, and the number of nodes from the root to any
// leaf node is never more than O(log n), where n is the number of items in the tree.
//
// It's important to realise that the nodes in a `BTreeVec` are reference counted. This means that
// unlike a `Vec`, cloning a `BTreeVec` is a essentially free. If the cloned `BTreeVec` is later
// modified, as many nodes a possible will be kept shared with the original. That is, only the
// nodes that need to be modified will be cloned. Since nodes can be cloned, this requires the items
// in a `BTreeVec` to be cheaply cloneable, unlike a `Vec`, which can simply move the items when it
// needs to resize itself.
//
// To give you an idea of the performance characteristics of a `BTreeVec`, a `BTreeVec` essentially
// just supports two operations: concatenation, and splitting. Concatenating two btrees is an O(log n)
// operation that results in a newly balanced btree. Splitting a btree at a given index is also a
// O(log n) operation. Both operations can cause nodes to be modified (and therefore items to be
// cloned), either because we needed to split a node in two, or move some items from one node to the
// other.
//
// All other operations on a `BTreeVec` are essentially built on top of these two operations (although
// there are some performance optimisations here and there). For instance, inserting an item in the
// middle of a tree amounts to creating a new tree containing the new item, splitting the original
// tree in two at the insertion points, and then concatenating the resulting trees to obtain the
// final tree.
//
// Most operations on a `BTreeVec` are therefore O(log n). For moderate number of items, a `Vec` will
// almost always be faster. Only when the number of items becomes very large will the `BTreeVec` start
// outperforming the `Vec`.
//
// There is one VERY important exception to this rule: searching on accumulated values. That is, given
// a sequence of items, each with a value, we want to find the index of the item for which the sum of
// the values of the items preceding it is less than or equal to a given target value. This is a O(n)
// operation on a `Vec`, but a O(log n) operation on a `BTreeVec`. This is because a `BTreeVec` can
// store the sum of the values of the items in each subtree, which allows it to perform a binary search
// on the accumulated values to find the desired index (more on this below).

// Let's start by defining a simple item type.
#[derive(Clone)]
struct Item {
    height: u32
}

// A `BTreeVec` can optionally be annotated with a `Metric`. A `Metric` is a `tag` type that provides
// a method to the `BTreeVec` to measure each item. You can use any `Metric` you like, as long as the
// resulting measure implements the `Measure` trait.
//
// The `Measure` trait models a mathematical monoid. That is, a measure must have an identity element
// (provided by `Measure::identity`) and an associative binary operation (provided by
// `Measure::combine`). This allows to `BTreeVec` to store the combined measure of the items in each
// subtree with each subtree.
struct ItemMetric;

impl Metric<Item> for ItemMetric {
    type Measure = u32;

    fn measure(item: &Item) -> Self::Measure {
        item.height
    }
}

fn main() {
    // Let's build a `Vec` of items.
    let instant = Instant::now();
    let mut vec = Vec::new();
    for index in 0..100_000_000 {
        vec.push(Item { height: index % 10 });
    }
    println!("Building the `Vec` took {:?}", instant.elapsed());

    // Now let's build an equivalent `BTreeVec` of items.
    //
    // The `btree_vec` crate provides a builder API to build the initial btree_vec. It's also
    // possible to create a `BTreeVec` with a call to `BTreeVec::new` and repeated calls to
    // `BTreeVec::push`, but this is slower, and the resulting `BTreeVec` will be less well
    // balanced, so the builder API is recommended.
    let instant = Instant::now();
    let mut builder = btree_vec::Builder::<_, ItemMetric>::new();
    for index in 0..100_000_000 {
        builder.push(Item { height: index % 10 });
    }
    let btree_vec = builder.finish();
    
    // We expect building the `BTreeVec` to be slower than building the `Vec`: the `Vec` consists
    // of a single contiguous block of memory, while the `BTreeVec` chops this up into hunks of 1024
    // items each, and organises these chunks in a binary tree.
    println!("Building the `BTreeVec` took {:?}", instant.elapsed());

    // Now, lets see if we can find the index of the item for which the sum of the heights of the
    // items preceding it is less than or equal to 50_000_000.
    let target_height = 50_000_000;

    // First, lets try to find the index in the `Vec`. Since we want the summed height, we need to
    // perform a linear scan.
    let instant = Instant::now();
    let mut summed_height = 0;
    let index_0 = vec.iter().position(|item| {
        let next_summed_height = summed_height + item.height;
        if target_height < next_summed_height {
            return true;
        }
        summed_height = next_summed_height;
        false
    }).unwrap();
    // Check that the computed summed height is indeed less than or equal to the target height.
    assert!(summed_height <= target_height);
    println!("Searching in the `Vec` took {:?}", instant.elapsed());

    // Next, lets try to find the index in the `BTreeVec`. The `BTreeVec::search_by` methods takes a
    // closure that will be called with a candidate summed height.
    // 
    // If the candidate does not match the desired criteria the closure should return `false`. In
    // that case, the closure will be called again with a new, strictly larger candidate summed
    // height.
    //
    // If the closure returns `true`, the search will either stop (if a leaf has been reached) or
    // the closure will be called again, with a new, strictly smaller candidate summed height
    // (smaller because we're descending to a lower level with more granular summed heights).
    let instant = Instant::now();
    let (index_1, summed_height) = btree_vec.search_by(|next_summed_height| target_height < next_summed_height).unwrap();
    // Check that the computed summed height is indeed less than or equal to the target height.
    assert!(summed_height <= target_height);

    // Check that we got the same result in both cases.
    assert_eq!(index_0, index_1);

    // We expect searching the `BTreeVec` to be faster than searching the `Vec`, provided the number
    // of items is large: the `Vec` has to perform a linear search, whereas the `BTreeVec` can
    // perform a much faster layered search. That is, it performs a linear search at each level of
    // the tree to find the subtree that contains the item we're looking for, and then repeats the
    // search in that subtree. Because each non-leaf node in the `BTreeVec` contains at least 4
    // children, this greatly reduces the number of comparisons that need to be made.
    println!("Searching in the `BTreeVec` took {:?}", instant.elapsed());
    
    // We also want to be able to perform what I call a `reverse search`. That is, given the target
    // index of an item in the `Vec` or `BTreeVec`, we want to compute the summed height of all
    // items preceding it.
    let target_index = 50_000_000;
    
    let instant = Instant::now();
    let summed_height_0 = vec.iter().take(target_index).fold(0, |summed_height, item| {
        summed_height + item.height
    });
    println!("Reverse searching in the `Vec` took {:?}", instant.elapsed());

    let instant = Instant::now();
    let summed_height_1 = btree_vec.measure_to(target_index);
    
    // We expect reverse searching the `BTreeVec` to be much faster than reverse searching the
    // `Vec`: reverse searching the `Vec` requires a linear scan, but we can obtain the summed
    // height of any item in the `BTreeVec` by simply walking the tree from the root to the
    // corresponding leaf for that item, which takes only O(log n) operations.
    println!("Reverse searching in the `BTreeVec` took {:?}", instant.elapsed());

    // Check that we got the same result in both cases.
    assert_eq!(summed_height_0, summed_height_1);
}