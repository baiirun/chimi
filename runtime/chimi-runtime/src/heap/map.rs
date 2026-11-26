//! The persistent Map tree, which Set shares (CHI-305 step 5, phase 4,
//! third PR): an AVL tree ordered by [`compare`]. Every operation copies the
//! nodes on its path and shares the rest, so a tree is never written once a
//! value holds it. Nodes are allocated in the order `runtime.c` allocated
//! them, so memory reports stay the same.

use core::ptr;

use alloc::vec::Vec;

use super::allocator::allocate;
use super::compare::compare;
use super::fragment::set_kind;
use super::layout::{AllocationKind, MapNode, Value};

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
}

/// The height of `node`, 0 for no node.
///
/// # Safety
/// `node` is null or live.
unsafe fn height(node: *const MapNode) -> u16 {
    // SAFETY: the caller's contract.
    if node.is_null() { 0 } else { unsafe { (*node).height } }
}

/// A new node over `left` and `right`.
///
/// # Safety
/// Runs on a fiber that may allocate; the children are null or live.
unsafe fn node(key: *mut Value, value: *mut Value, left: *mut MapNode, right: *mut MapNode) -> *mut MapNode {
    // SAFETY: the caller's contract.
    unsafe {
        let node = allocate(1, size_of::<MapNode>()).cast::<MapNode>();
        set_kind(node.cast(), AllocationKind::MAP_NODE);
        (*node).key = key;
        (*node).value = value;
        (*node).left = left;
        (*node).right = right;
        (*node).height = 1 + height(left).max(height(right));
        node
    }
}

/// # Safety
/// As for [`node`]; `node` has a right child.
unsafe fn rotate_left(node: *mut MapNode) -> *mut MapNode {
    // SAFETY: the caller's contract.
    unsafe {
        let right = (*node).right;
        let left = self::node((*node).key, (*node).value, (*node).left, (*right).left);
        self::node((*right).key, (*right).value, left, (*right).right)
    }
}

/// # Safety
/// As for [`node`]; `node` has a left child.
unsafe fn rotate_right(node: *mut MapNode) -> *mut MapNode {
    // SAFETY: the caller's contract.
    unsafe {
        let left = (*node).left;
        let right = self::node((*node).key, (*node).value, (*left).right, (*node).right);
        self::node((*left).key, (*left).value, (*left).left, right)
    }
}

/// `node` rebalanced when its children's heights differ by two.
///
/// # Safety
/// As for [`node`]; `node` is live.
unsafe fn balance(node: *mut MapNode) -> *mut MapNode {
    // SAFETY: the caller's contract; a side two taller than the other has
    // the child each rotation needs.
    unsafe {
        let (left, right) = ((*node).left, (*node).right);
        let difference = i32::from(height(left)) - i32::from(height(right));
        if difference > 1 {
            let node = if height((*left).right) > height((*left).left) {
                self::node((*node).key, (*node).value, rotate_left(left), right)
            } else {
                node
            };
            return rotate_right(node);
        }
        if difference < -1 {
            let node = if height((*right).left) > height((*right).right) {
                self::node((*node).key, (*node).value, left, rotate_right(right))
            } else {
                node
            };
            return rotate_left(node);
        }
        node
    }
}

/// `node` with `key` bound to `value`. An equal key already held stays;
/// only the value changes. The recursion is bounded by the tree height.
///
/// # Safety
/// As for [`node`]; `key` is of the tree's key type.
pub unsafe fn put(node: *mut MapNode, key: *mut Value, value: *mut Value) -> *mut MapNode {
    // SAFETY: the caller's contract.
    unsafe {
        if node.is_null() {
            return self::node(key, value, ptr::null_mut(), ptr::null_mut());
        }
        let order = compare(key, (*node).key);
        if order == 0 {
            return self::node((*node).key, value, (*node).left, (*node).right);
        }
        if order < 0 {
            let left = put((*node).left, key, value);
            return balance(self::node((*node).key, (*node).value, left, (*node).right));
        }
        let right = put((*node).right, key, value);
        balance(self::node((*node).key, (*node).value, (*node).left, right))
    }
}

/// `node` without its least node, which goes in `least`.
///
/// # Safety
/// As for [`node`]; `node` is live.
unsafe fn remove_least(node: *mut MapNode, least: &mut *mut MapNode) -> *mut MapNode {
    // SAFETY: the caller's contract.
    unsafe {
        if (*node).left.is_null() {
            *least = node;
            return (*node).right;
        }
        let left = remove_least((*node).left, least);
        balance(self::node((*node).key, (*node).value, left, (*node).right))
    }
}

/// `node` without `key`, rebalanced; `node` itself when it has no such key.
/// The recursion is bounded by the tree height.
///
/// # Safety
/// As for [`put`].
pub unsafe fn remove(node: *mut MapNode, key: *const Value) -> *mut MapNode {
    // SAFETY: the caller's contract.
    unsafe {
        if node.is_null() {
            return node;
        }
        let order = compare(key, (*node).key);
        if order < 0 {
            let left = remove((*node).left, key);
            return if left == (*node).left {
                node
            } else {
                balance(self::node((*node).key, (*node).value, left, (*node).right))
            };
        }
        if order > 0 {
            let right = remove((*node).right, key);
            return if right == (*node).right {
                node
            } else {
                balance(self::node((*node).key, (*node).value, (*node).left, right))
            };
        }
        if (*node).left.is_null() {
            return (*node).right;
        }
        if (*node).right.is_null() {
            return (*node).left;
        }
        let mut least = ptr::null_mut();
        let right = remove_least((*node).right, &mut least);
        balance(self::node((*least).key, (*least).value, (*node).left, right))
    }
}

/// The value bound to `key`, or null.
///
/// # Safety
/// As for [`put`], without allocating.
pub unsafe fn get(mut node: *const MapNode, key: *const Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        while !node.is_null() {
            let order = compare(key, (*node).key);
            if order == 0 {
                return (*node).value;
            }
            node = if order < 0 { (*node).left } else { (*node).right };
        }
        ptr::null_mut()
    }
}

/// The number of keys.
///
/// # Safety
/// `node` is null or a live tree.
pub unsafe fn size(node: *const MapNode) -> i64 {
    // SAFETY: the caller's contract; the recursion is bounded by the height.
    if node.is_null() { 0 } else { unsafe { 1 + size((*node).left) + size((*node).right) } }
}

/// The nodes in key order, so equal maps yield equal sequences whatever
/// their insertion history.
///
/// # Safety
/// `root` is null or a live tree.
pub unsafe fn entries(root: *const MapNode) -> Vec<*const MapNode> {
    let mut nodes = Vec::new();
    // The tree is balanced, so the pending nodes never outnumber its height.
    let mut pending: [*const MapNode; 64] = [ptr::null(); 64];
    let mut pending_length = 0;
    let mut node = root;
    // SAFETY: the caller's contract.
    unsafe {
        loop {
            while !node.is_null() {
                pending[pending_length] = node;
                pending_length += 1;
                node = (*node).left;
            }
            if pending_length == 0 {
                break;
            }
            pending_length -= 1;
            node = pending[pending_length];
            if nodes.len() == nodes.capacity() && nodes.try_reserve(nodes.len().max(16)).is_err() {
                // SAFETY: a function of runtime.c that does not return.
                chimi_runtime_exhausted_memory()
            }
            nodes.push(node);
            node = (*node).right;
        }
    }
    nodes
}
