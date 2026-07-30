use crate::EMPTY_REF;
use crate::set::node::{Color, Node};
use crate::set::pool::Pool;
use crate::set::sort::{KeyValue, SetCollection};
use core::cmp::Ordering;
use core::marker::PhantomData;

pub struct SetTree<K, V> {
    pub(super) store: Pool<V>,
    pub(super) root: u32,
    phantom_data: PhantomData<K>,
}

const NIL_INDEX: u32 = 0;

impl<K, V: Clone + Default> SetTree<K, V> {
    #[inline]
    pub fn new(capacity: usize) -> Self {
        let mut store = Pool::new(capacity);
        let nil_index = store.get_free_index();
        assert_eq!(nil_index, NIL_INDEX);
        Self {
            store,
            root: EMPTY_REF,
            phantom_data: Default::default(),
        }
    }
}
impl<K: Ord, V: KeyValue<K> + Clone + Default> SetCollection<K, V> for SetTree<K, V> {
    #[inline]
    fn insert(&mut self, val: V) {
        self.insert_value(val);
    }

    #[inline]
    fn delete_by_index(&mut self, index: u32) {
        self.delete_index(index);
    }

    #[inline]
    fn index_before(&self, mut index: u32) -> u32 {
        let node = self.node(index);
        if node.left != EMPTY_REF {
            self.find_right_minimum(node.left)
        } else {
            // find first parent where we not left
            let mut parent_index = node.parent;
            let mut parent = self.node(parent_index);
            while parent.left == index {
                index = parent_index;
                parent_index = parent.parent;
                parent = self.node(parent_index);
            }
            parent_index
        }
    }

    #[inline]
    fn first_index_less_by<F>(&self, f: F) -> u32
    where
        F: Fn(&K) -> Ordering,
    {
        self.search_first_less_by(f)
    }

    unsafe fn value_by_index(&self, index: u32) -> &V {
        &self.node(index).value
    }

    #[inline]
    unsafe fn value_by_index_mut(&mut self, index: u32) -> &mut V {
        &mut self.node_mut(index).value
    }
}

impl<K: Ord, V: Clone + Default + KeyValue<K>> SetTree<K, V> {
    #[inline(always)]
    fn is_black(&self, index: u32) -> bool {
        index == EMPTY_REF || self.node(index).color == Color::Black
    }

    #[inline(always)]
    pub(super) fn node(&self, index: u32) -> &Node<V> {
        unsafe { self.store.buffer.get_unchecked(index as usize) }
    }

    #[inline(always)]
    pub(super) fn node_mut(&mut self, index: u32) -> &mut Node<V> {
        unsafe { self.store.buffer.get_unchecked_mut(index as usize) }
    }

    #[inline]
    fn create_nil_node(&mut self, parent: u32) {
        let node = self.node_mut(NIL_INDEX);
        node.parent = parent;
        node.left = EMPTY_REF;
        node.right = EMPTY_REF;
        node.color = Color::Red;
    }

    #[inline]
    fn insert_root(&mut self, value: V) {
        let new_index = self.store.get_free_index();
        let new_node = self.node_mut(new_index);
        new_node.parent = EMPTY_REF;
        new_node.left = EMPTY_REF;
        new_node.right = EMPTY_REF;
        new_node.color = Color::Black;
        new_node.value = value;
        self.root = new_index;
    }

    #[inline]
    fn search_first_less_by<F>(&self, f: F) -> u32
    where
        F: Fn(&K) -> Ordering,
    {
        let mut index = self.root;
        let mut result = EMPTY_REF;
        while index != EMPTY_REF {
            let node = self.node(index);
            match f(node.value.key()) {
                Ordering::Equal => return index,
                Ordering::Less => {
                    result = index;
                    index = node.right;
                }
                Ordering::Greater => index = node.left,
            }
        }

        result
    }

    #[inline]
    fn insert_value(&mut self, value: V) {
        let mut index = self.root;
        if index == EMPTY_REF {
            self.insert_root(value);
            return;
        }

        let key = value.key();

        loop {
            let p_index = index;
            let node = self.node(index);
            if key < node.value.key() {
                index = node.left;
                if index == EMPTY_REF {
                    self.insert_as_left(value, p_index);
                    return;
                }
            } else {
                index = node.right;
                if index == EMPTY_REF {
                    self.insert_as_right(value, p_index);
                    return;
                }
            }
        }
    }

    #[inline]
    fn insert_new(&mut self, value: V, p_index: u32) -> u32 {
        let new_index = self.store.get_free_index();
        let new_node = self.node_mut(new_index);
        new_node.parent = p_index;
        new_node.left = EMPTY_REF;
        new_node.right = EMPTY_REF;
        new_node.color = Color::Red;
        new_node.value = value;

        new_index
    }

    #[inline]
    fn insert_as_left(&mut self, value: V, p_index: u32) {
        let new_index = self.insert_new(value, p_index);

        let parent = self.node_mut(p_index);
        parent.left = new_index;

        if parent.color == Color::Red {
            self.fix_red_black_properties_after_insert(new_index, p_index);
        }
    }

    #[inline]
    fn insert_as_right(&mut self, value: V, p_index: u32) {
        let new_index = self.insert_new(value, p_index);

        let parent = self.node_mut(p_index);
        parent.right = new_index;

        if parent.color == Color::Red {
            self.fix_red_black_properties_after_insert(new_index, p_index);
        }
    }

    fn fix_red_black_properties_after_insert(&mut self, n_index: u32, p_origin: u32) {
        // parent is red!
        let mut p_index = p_origin;
        // Case 2:
        // Not having a grandparent means that parent is the root. If we enforce black roots
        // (rule 2), grandparent will never be null, and the following if-then block can be
        // removed.
        let g_index = self.node(p_index).parent;
        if g_index == EMPTY_REF {
            // As this method is only called on red nodes (either on newly inserted ones - or -
            // recursively on red grandparents), all we have to do is to recolor the root black.
            self.node_mut(p_index).color = Color::Black;
            return;
        }

        // Case 3: Uncle is red -> recolor parent, grandparent and uncle
        let u_index = self.get_uncle(p_index);

        if u_index != EMPTY_REF && self.node(u_index).color == Color::Red {
            self.node_mut(p_index).color = Color::Black;
            self.node_mut(g_index).color = Color::Red;
            self.node_mut(u_index).color = Color::Black;

            // Call recursively for grandparent, which is now red.
            // It might be root or have a red parent, in which case we need to fix more...
            let gg_index = self.node(g_index).parent;
            if gg_index != EMPTY_REF && self.node(gg_index).color == Color::Red {
                self.fix_red_black_properties_after_insert(g_index, gg_index);
            }
        } else if p_index == self.node(g_index).left {
            // Parent is left child of grandparent
            // Case 4a: Uncle is black and node is left->right "inner child" of its grandparent
            if n_index == self.node(p_index).right {
                self.rotate_left(p_index);

                // Let "parent" point to the new root node of the rotated subtree.
                // It will be recolored in the next step, which we're going to fall-through to.
                p_index = n_index;
            }

            // Case 5a: Uncle is black and node is left->left "outer child" of its grandparent
            self.rotate_right(g_index);

            // Recolor original parent and grandparent
            self.node_mut(p_index).color = Color::Black;
            self.node_mut(g_index).color = Color::Red;
        } else {
            // Parent is right child of grandparent
            // Case 4b: Uncle is black and node is right->left "inner child" of its grandparent
            if n_index == self.node(p_index).left {
                self.rotate_right(p_index);

                // Let "parent" point to the new root node of the rotated subtree.
                // It will be recolored in the next step, which we're going to fall-through to.
                p_index = n_index;
            }

            // Case 5b: Uncle is black and node is right->right "outer child" of its grandparent
            self.rotate_left(g_index);

            // Recolor original parent and grandparent
            self.node_mut(p_index).color = Color::Black;
            self.node_mut(g_index).color = Color::Red;
        }
    }

    fn rotate_right(&mut self, index: u32) {
        let n = self.node(index);
        let p = n.parent;
        let lt_index = n.left;

        let lt_node = self.node_mut(lt_index);
        let lt_right = lt_node.right;
        lt_node.right = index;

        if lt_right != EMPTY_REF {
            self.node_mut(lt_right).parent = index;
        }

        let node = self.node_mut(index);
        node.left = lt_right;
        node.parent = lt_index;

        self.replace_parents_child(p, index, lt_index);
    }

    fn rotate_left(&mut self, index: u32) {
        let n = self.node(index);
        let p = n.parent;
        let rt_index = n.right;

        let rt_node = self.node_mut(rt_index);
        let rt_left = rt_node.left;
        rt_node.left = index;

        if rt_left != EMPTY_REF {
            self.node_mut(rt_left).parent = index;
        }
        let node = self.node_mut(index);
        node.right = rt_left;
        node.parent = rt_index;

        self.replace_parents_child(p, index, rt_index);
    }

    #[inline]
    fn replace_parents_child(&mut self, parent: u32, old_child: u32, new_child: u32) {
        self.node_mut(new_child).parent = parent;
        if parent == EMPTY_REF {
            self.root = new_child;
            return;
        }

        let p = self.node_mut(parent);
        debug_assert!(
            p.left == old_child || p.right == old_child,
            "Node is not a child of its parent"
        );

        if p.left == old_child {
            p.left = new_child;
        } else {
            p.right = new_child;
        }
    }

    #[inline]
    fn find_left_minimum(&self, mut i: u32) -> u32 {
        while self.node(i).left != EMPTY_REF {
            i = self.node(i).left;
        }
        i
    }

    #[inline]
    fn find_right_minimum(&self, mut i: u32) -> u32 {
        while self.node(i).right != EMPTY_REF {
            i = self.node(i).right;
        }
        i
    }

    pub(super) fn delete_index(&mut self, index: u32) {
        // Node has zero or one child
        let mut delete_index = index;

        let node = self.node(index);
        let mut nd_left = node.left;
        let mut nd_right = node.right;
        let mut nd_parent = node.parent;
        let mut nd_color = node.color;

        // if two children replace node with it left minimum
        if nd_left != EMPTY_REF && nd_right != EMPTY_REF {
            let successor_index = self.find_left_minimum(nd_right);
            let successor = self.node(successor_index);
            let value = successor.value.clone();
            nd_parent = successor.parent;
            nd_left = successor.left;
            nd_right = successor.right;
            nd_color = successor.color;

            self.node_mut(index).value = value;

            delete_index = successor_index;
        }

        // only one child can be!

        if nd_left != EMPTY_REF {
            self.replace_parents_child(nd_parent, delete_index, nd_left);
            self.fix_red_black_properties_after_delete(nd_left);
        } else if nd_right != EMPTY_REF {
            self.replace_parents_child(nd_parent, delete_index, nd_right);
            self.fix_red_black_properties_after_delete(nd_right);
        } else if nd_parent == EMPTY_REF {
            self.root = EMPTY_REF;
        } else {
            // Node has no children -->
            // * node is red --> just remove it
            // * node is black --> replace it by a temporary NIL node (needed to fix the R-B rules)
            if nd_color == Color::Black {
                self.create_nil_node(nd_parent);
                self.set_nil_parents_child(nd_parent, delete_index);
                self.fix_red_black_properties_after_delete(NIL_INDEX);
                self.fix_parents_nil_child();
            } else {
                self.remove_parents_child(nd_parent, delete_index);
            }
        }

        self.store.put_back(delete_index);
    }

    fn fix_red_black_properties_after_delete(&mut self, n_index: u32) {
        // Case 1: Examined node is root, end of recursion
        if n_index == self.root {
            // do not color root to black
            return;
        }

        let mut s_index = self.get_sibling(n_index);

        // Case 2: Red sibling
        if self.node(s_index).color == Color::Red {
            self.handle_red_sibling(n_index, s_index);
            s_index = self.get_sibling(n_index) // Get new sibling for fall-through to cases 3-6
        }

        let sibling = self.node(s_index);

        // Cases 3+4: Black sibling with two black children
        if self.is_black(sibling.left) && self.is_black(sibling.right) {
            self.node_mut(s_index).color = Color::Red;
            let p_index = self.node(n_index).parent;

            // Case 3: Black sibling with two black children + red parent
            let parent = self.node_mut(p_index);
            if parent.color == Color::Red {
                parent.color = Color::Black;
            } else {
                // Case 4: Black sibling with two black children + black parent
                self.fix_red_black_properties_after_delete(p_index);
            }
        } else {
            // Case 5+6: Black sibling with at least one red child
            self.handle_black_sibling_with_at_least_one_red_child(n_index, s_index);
        }
    }

    fn handle_black_sibling_with_at_least_one_red_child(&mut self, n_index: u32, s_origin: u32) {
        let p_index = self.node(n_index).parent;

        let mut s_index = s_origin;
        let (mut sibling_left, mut sibling_right) = {
            let sibling = self.node(s_origin);
            (sibling.left, sibling.right)
        };

        let node_is_left_child = n_index == self.node(p_index).left;

        // Case 5: Black sibling with at least one red child + "outer nephew" is black
        // --> Recolor sibling and its child, and rotate around sibling
        if node_is_left_child && self.is_black(sibling_right) {
            if sibling_left != EMPTY_REF {
                self.node_mut(sibling_left).color = Color::Black;
            }
            self.node_mut(s_index).color = Color::Red;
            self.rotate_right(s_index);
            s_index = self.node(p_index).right;

            let sibling = self.node(s_index);
            sibling_left = sibling.left;
            sibling_right = sibling.right;
        } else if !node_is_left_child && self.is_black(sibling_left) {
            if sibling_right != EMPTY_REF {
                self.node_mut(sibling_right).color = Color::Black;
            }
            self.node_mut(s_index).color = Color::Red;
            self.rotate_left(s_index);
            s_index = self.node(p_index).left;

            let sibling = self.node(s_index);
            sibling_left = sibling.left;
            sibling_right = sibling.right;
        }

        // Fall-through to case 6...

        // Case 6: Black sibling with at least one red child + "outer nephew" is red
        // --> Recolor sibling + parent + sibling's child, and rotate around parent
        self.node_mut(s_index).color = self.node(p_index).color;
        self.node_mut(p_index).color = Color::Black;
        if node_is_left_child {
            if sibling_right != EMPTY_REF {
                self.node_mut(sibling_right).color = Color::Black;
            }
            self.rotate_left(p_index)
        } else {
            if sibling_left != EMPTY_REF {
                self.node_mut(sibling_left).color = Color::Black;
            }
            self.rotate_right(p_index)
        }
    }

    fn handle_red_sibling(&mut self, n_index: u32, s_index: u32) {
        // Recolor...

        self.node_mut(s_index).color = Color::Black;
        let p_index = self.node(n_index).parent;
        let parent = self.node_mut(p_index);

        parent.color = Color::Red;

        // ... and rotate
        if n_index == parent.left {
            self.rotate_left(p_index)
        } else {
            self.rotate_right(p_index)
        }
    }

    #[inline]
    fn get_uncle(&self, p_index: u32) -> u32 {
        let parent = self.node(p_index);
        debug_assert!(parent.parent != EMPTY_REF);
        let grandparent = self.node(parent.parent);

        debug_assert!(
            grandparent.left == p_index || grandparent.right == p_index,
            "Parent is not a child of its grandparent"
        );

        if grandparent.left == p_index {
            grandparent.right
        } else {
            grandparent.left
        }
    }

    #[inline(always)]
    fn get_sibling(&self, n_index: u32) -> u32 {
        let p_index = self.node(n_index).parent;
        let parent = self.node(p_index);
        debug_assert!(n_index == parent.left || n_index == parent.right);
        if n_index == parent.left {
            parent.right
        } else {
            parent.left
        }
    }

    #[inline]
    fn remove_parents_child(&mut self, parent: u32, old_child: u32) {
        let p = self.node_mut(parent);
        debug_assert!(
            p.left == old_child || p.right == old_child,
            "Node is not a child of its parent"
        );

        if p.left == old_child {
            p.left = EMPTY_REF;
        } else {
            p.right = EMPTY_REF;
        }
    }

    #[inline]
    fn set_nil_parents_child(&mut self, parent: u32, old_child: u32) {
        let p = self.node_mut(parent);
        debug_assert!(
            p.left == old_child || p.right == old_child,
            "Node is not a child of its parent"
        );

        if p.left == old_child {
            p.left = NIL_INDEX;
        } else {
            p.right = NIL_INDEX;
        }
    }

    #[inline]
    fn fix_parents_nil_child(&mut self) {
        let p_index = self.node(NIL_INDEX).parent;
        let p = self.node_mut(p_index);
        debug_assert!(
            p.left == NIL_INDEX || p.right == NIL_INDEX,
            "Node is not a child of its parent"
        );

        if p.left == NIL_INDEX {
            p.left = EMPTY_REF;
        } else {
            p.right = EMPTY_REF;
        }
    }
}
