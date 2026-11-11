//! Persistent vector behind interpreter `List` values.
//!
//! A list is a window `[start, start + length)` over a `Vector`: a 32-way
//! radix trie of full leaves plus a tail holding the last 1..=32 elements,
//! matching the native runtime's `ListNode` layout. Every operation returns a
//! new list and shares all untouched nodes with the old one, so appending is
//! amortized O(1), indexing is O(log32 n), and slicing is O(1) amortized.
//! Elements sit behind `Arc` so copying a 32-slot node never deep-clones a value.
//!
//! Window retention: a slice shorter than a quarter of its vector copies its
//! elements into a fresh vector instead of sharing, so a small window never keeps
//! a large vector alive. Draining a list of n elements one at a time therefore
//! copies n/4 + n/16 + ... < n/3 elements in total: each copy of m elements is
//! paid for by the 3m or more elements dropped since the vector was built. The
//! source window is compacted, not just the slice: it adopts a vector of exactly
//! its own elements, which every later slice of it shares, so matching the same
//! window again is O(1) rather than another copy. The native runtime's
//! `list_slice` applies the same rule.

use std::fmt;
use std::sync::{Arc, OnceLock};

const BITS: u32 = 5;
const WIDTH: usize = 1 << BITS;
const MASK: usize = WIDTH - 1;
/// A slice shorter than `1 / RETAINED_FRACTION` of its vector is copied (see the module docs).
const RETAINED_FRACTION: usize = 4;

enum Node<T> {
    Branch(Vec<Arc<Node<T>>>),
    Leaf(Vec<Arc<T>>),
}

pub(super) struct PersistentList<T> {
    vector: Vector<T>,
    start: usize,
    length: usize,
    /// This window's elements in a vector of their own, built the first time a
    /// slice of it would copy. Clones share it, so the copy happens once per value.
    compacted: Arc<OnceLock<Vector<T>>>,
}

impl<T> Clone for PersistentList<T> {
    fn clone(&self) -> Self {
        Self {
            vector: self.vector.clone(),
            start: self.start,
            length: self.length,
            compacted: Arc::clone(&self.compacted),
        }
    }
}

impl<T> Default for PersistentList<T> {
    fn default() -> Self {
        Self::window(Vector::default(), 0, 0)
    }
}

struct Vector<T> {
    length: usize,
    /// Bit shift of the root's child index: `BITS * root height`.
    shift: u32,
    /// `None` until the list outgrows its tail.
    root: Option<Arc<Node<T>>>,
    tail: Arc<Vec<Arc<T>>>,
}

impl<T> Clone for Vector<T> {
    fn clone(&self) -> Self {
        Self {
            length: self.length,
            shift: self.shift,
            root: self.root.clone(),
            tail: Arc::clone(&self.tail),
        }
    }
}

impl<T> Default for Vector<T> {
    fn default() -> Self {
        Self {
            length: 0,
            shift: BITS,
            root: None,
            tail: Arc::new(Vec::new()),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for PersistentList<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.iter()).finish()
    }
}

/// Index of the first element stored in the tail rather than the trie.
fn tail_offset(length: usize) -> usize {
    if length == 0 { 0 } else { (length - 1) & !MASK }
}

/// A chain of single-child branches from `shift` down to `leaf`.
fn new_path<T>(shift: u32, leaf: Arc<Node<T>>) -> Arc<Node<T>> {
    if shift == 0 {
        leaf
    } else {
        Arc::new(Node::Branch(vec![new_path(shift - BITS, leaf)]))
    }
}

/// Copies the path to the leaf starting at element `first_index` and attaches `leaf`.
fn push_leaf<T>(
    node: &Node<T>,
    shift: u32,
    first_index: usize,
    leaf: Arc<Node<T>>,
) -> Arc<Node<T>> {
    let Node::Branch(children) = node else {
        unreachable!("persistent list leaves sit only at shift 0");
    };
    let slot = (first_index >> shift) & MASK;
    let mut children = children.clone();
    if shift == BITS {
        children.push(leaf);
    } else if let Some(child) = children.get(slot) {
        children[slot] = push_leaf(child, shift - BITS, first_index, leaf);
    } else {
        children.push(new_path(shift - BITS, leaf));
    }
    Arc::new(Node::Branch(children))
}

/// Copies the path to element `index` with `value` in its slot.
fn set_node<T>(node: &Node<T>, shift: u32, index: usize, value: Arc<T>) -> Arc<Node<T>> {
    Arc::new(match node {
        Node::Branch(children) => {
            let slot = (index >> shift) & MASK;
            let mut children = children.clone();
            children[slot] = set_node(&children[slot], shift - BITS, index, value);
            Node::Branch(children)
        }
        Node::Leaf(values) => {
            let mut values = values.clone();
            values[index & MASK] = value;
            Node::Leaf(values)
        }
    })
}

impl<T> PersistentList<T> {
    fn window(vector: Vector<T>, start: usize, length: usize) -> Self {
        Self {
            vector,
            start,
            length,
            compacted: Arc::default(),
        }
    }

    /// Elements `range` of this window in a fresh vector.
    fn copy_range(&self, range: std::ops::Range<usize>) -> Vector<T> {
        Vector::from_shared(
            range
                .map(|index| {
                    Arc::clone(
                        self.vector
                            .get(self.start + index)
                            .expect("index is inside the window"),
                    )
                })
                .collect(),
        )
    }

    pub(super) fn len(&self) -> usize {
        self.length
    }

    pub(super) fn first(&self) -> Option<&T> {
        self.get(0)
    }

    pub(super) fn get(&self, index: usize) -> Option<&T> {
        if index >= self.length {
            return None;
        }
        self.vector.get(self.start + index).map(Arc::as_ref)
    }

    /// A weak handle naming this list value: its clones share it, and no other live
    /// list has the same address. Holding it keeps the address from being reused.
    pub(super) fn identity(&self) -> std::sync::Weak<dyn std::any::Any>
    where
        T: 'static,
    {
        Arc::downgrade(&self.compacted) as std::sync::Weak<dyn std::any::Any>
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &T> {
        (0..self.length).map(|index| self.get(index).expect("index is inside the window"))
    }

    // `push` is reached from Chimi once the standard-library mechanism (G1)
    // exposes it; until then only tests call it.

    /// Returns a new list with `value` appended; `self` is unchanged. A window
    /// that ends before its vector does writes the next slot into a path copy,
    /// so the shared vector every other window sees is untouched.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn push(&self, value: T) -> Self {
        let end = self.start + self.length;
        let value = Arc::new(value);
        let vector = if end == self.vector.length {
            self.vector.push(value)
        } else {
            self.vector.set(end, value)
        };
        Self::window(vector, self.start, self.length + 1)
    }

    /// Window `range` of this list. `None` when the range is reversed or extends
    /// past the end. The window shares this list's vector in O(1), unless it is
    /// shorter than a quarter of that vector, so it does not retain the rest. Then
    /// it shares this list's compacted vector, built on first use, or copies its
    /// own elements when it is shorter than a quarter of this list too.
    pub(super) fn slice(&self, range: std::ops::Range<usize>) -> Option<Self> {
        if range.start > range.end || range.end > self.length {
            return None;
        }
        if range.is_empty() {
            return Some(Self::default());
        }
        let length = range.len();
        if length * RETAINED_FRACTION >= self.vector.length {
            return Some(Self::window(
                self.vector.clone(),
                self.start + range.start,
                length,
            ));
        }
        if length * RETAINED_FRACTION < self.length {
            return Some(Self::window(self.copy_range(range), 0, length));
        }
        let compacted = self
            .compacted
            .get_or_init(|| self.copy_range(0..self.length));
        Some(Self::window(compacted.clone(), range.start, length))
    }
}

impl<T> Vector<T> {
    fn get(&self, index: usize) -> Option<&Arc<T>> {
        if index >= self.length {
            return None;
        }
        let offset = tail_offset(self.length);
        if index >= offset {
            return Some(&self.tail[index - offset]);
        }
        let mut node = self.root.as_deref()?;
        let mut shift = self.shift;
        loop {
            match node {
                Node::Branch(children) => {
                    node = &children[(index >> shift) & MASK];
                    shift -= BITS;
                }
                Node::Leaf(values) => return Some(&values[index & MASK]),
            }
        }
    }

    /// Amortized O(1): copies at most the tail plus one root-to-leaf path.
    #[cfg_attr(not(test), allow(dead_code))]
    fn push(&self, value: Arc<T>) -> Self {
        let tail_length = self.length - tail_offset(self.length);
        if tail_length < WIDTH {
            let mut tail = Vec::with_capacity(tail_length + 1);
            tail.extend(self.tail.iter().cloned());
            tail.push(value);
            return Self {
                length: self.length + 1,
                shift: self.shift,
                root: self.root.clone(),
                tail: Arc::new(tail),
            };
        }
        let leaf = Arc::new(Node::Leaf(self.tail.as_ref().clone()));
        let first_index = self.length - WIDTH;
        let (root, shift) = match &self.root {
            None => (new_path(BITS, leaf), BITS),
            Some(root) if first_index >> BITS == 1 << self.shift => (
                Arc::new(Node::Branch(vec![
                    Arc::clone(root),
                    new_path(self.shift, leaf),
                ])),
                self.shift + BITS,
            ),
            Some(root) => (push_leaf(root, self.shift, first_index, leaf), self.shift),
        };
        Self {
            length: self.length + 1,
            shift,
            root: Some(root),
            tail: Arc::new(vec![value]),
        }
    }

    /// O(log32 n): copies the path to element `index`, which must exist.
    #[cfg_attr(not(test), allow(dead_code))]
    fn set(&self, index: usize, value: Arc<T>) -> Self {
        assert!(index < self.length, "set index {index} of {}", self.length);
        let offset = tail_offset(self.length);
        let mut next = self.clone();
        if index >= offset {
            let mut tail = self.tail.as_ref().clone();
            tail[index - offset] = value;
            next.tail = Arc::new(tail);
        } else {
            let root = self
                .root
                .as_ref()
                .expect("an index before the tail lies in the trie");
            next.root = Some(set_node(root, self.shift, index, value));
        }
        next
    }
}

impl<T> FromIterator<T> for PersistentList<T> {
    fn from_iter<I: IntoIterator<Item = T>>(values: I) -> Self {
        let vector = values.into_iter().collect::<Vector<T>>();
        let length = vector.length;
        Self::window(vector, 0, length)
    }
}

impl<T> FromIterator<T> for Vector<T> {
    fn from_iter<I: IntoIterator<Item = T>>(values: I) -> Self {
        Self::from_shared(values.into_iter().map(Arc::new).collect())
    }
}

impl<T> Vector<T> {
    /// Builds the trie bottom-up in O(n) without intermediate versions.
    fn from_shared(mut values: Vec<Arc<T>>) -> Self {
        let length = values.len();
        let tail = Arc::new(values.split_off(tail_offset(length)));
        if values.is_empty() {
            return Self {
                length,
                tail,
                ..Self::default()
            };
        }
        let mut level = Vec::with_capacity(values.len() / WIDTH);
        let mut values = values.into_iter();
        while values.len() != 0 {
            level.push(Arc::new(Node::Leaf(values.by_ref().take(WIDTH).collect())));
        }
        let mut shift = 0;
        while level.len() > 1 || shift == 0 {
            shift += BITS;
            let mut children = level.into_iter();
            level = Vec::with_capacity(children.len().div_ceil(WIDTH));
            while children.len() != 0 {
                level.push(Arc::new(Node::Branch(
                    children.by_ref().take(WIDTH).collect(),
                )));
            }
        }
        Self {
            length,
            shift,
            root: level.pop(),
            tail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pushed(length: usize) -> PersistentList<usize> {
        (0..length).fold(PersistentList::default(), |list, value| list.push(value))
    }

    fn assert_sequence(list: &PersistentList<usize>, length: usize, offset: usize) {
        assert_eq!(list.len(), length);
        for index in 0..length {
            assert_eq!(list.get(index), Some(&(offset + index)), "index {index}");
        }
        assert_eq!(list.get(length), None);
    }

    #[test]
    fn push_and_get_across_trie_heights() {
        // 1,100,000 elements need a root of shift 20 (above 32^4 trie slots).
        let length = 1_100_000;
        let list = pushed(length);
        assert_sequence(&list, length, 0);
        assert_eq!(
            list.iter().copied().sum::<usize>(),
            length * (length - 1) / 2
        );
    }

    #[test]
    fn old_versions_are_unchanged_after_append() {
        let lengths = [0, 1, 31, 32, 33, 64, 1024, 1056, 1057, 32_800, 33_824];
        let mut list = PersistentList::default();
        let mut versions = Vec::new();
        for &length in &lengths {
            while list.len() < length {
                list = list.push(list.len());
            }
            versions.push(list.clone());
        }
        for (version, &length) in versions.iter().zip(&lengths) {
            let left = (0..40).fold(version.clone(), |list, value| list.push(1_000_000 + value));
            let right = version.push(7);
            assert_sequence(version, length, 0);
            assert_eq!(left.get(length), Some(&1_000_000));
            assert_eq!(left.len(), length + 40);
            assert_eq!(right.get(length), Some(&7));
            assert_eq!(right.len(), length + 1);
        }
    }

    #[test]
    fn append_shares_every_full_leaf_with_the_old_version() {
        let old = pushed(32 * 20);
        let new = old.push(0);
        let (Some(Node::Branch(old_leaves)), Some(Node::Branch(new_leaves))) =
            (old.vector.root.as_deref(), new.vector.root.as_deref())
        else {
            panic!("both versions have a trie");
        };
        assert_eq!(new_leaves.len(), old_leaves.len() + 1);
        for (old_leaf, new_leaf) in old_leaves.iter().zip(new_leaves) {
            assert!(Arc::ptr_eq(old_leaf, new_leaf));
        }
    }

    #[test]
    fn bulk_construction_matches_push() {
        for length in [
            0, 1, 2, 31, 32, 33, 63, 64, 65, 1023, 1024, 1025, 1056, 1057, 1088, 32_768, 32_800,
            32_801, 33_824, 33_825, 65_536, 65_568, 70_000,
        ] {
            let bulk = (0..length).collect::<PersistentList<_>>();
            assert_sequence(&bulk, length, 0);
            // Appending relies on the bulk trie having the shape push builds.
            let extended = (length..length + 2000).fold(bulk, |list, value| list.push(value));
            assert_sequence(&extended, length + 2000, 0);
        }
    }

    // Guards: repeated append stays linear; the flat Vec it replaced copied the
    // whole list per append (about 5 * 10^11 element copies at this length).
    #[test]
    fn appending_a_million_elements_is_linear() {
        let started = std::time::Instant::now();
        let list = pushed(1_000_000);
        let elapsed = started.elapsed();
        assert_eq!(list.len(), 1_000_000);
        assert!(elapsed.as_secs() < 10, "a million appends took {elapsed:?}");
    }

    fn assert_window(list: &PersistentList<usize>, from: usize, length: usize) {
        assert_eq!(list.len(), length);
        assert_eq!(
            list.iter().copied().collect::<Vec<_>>(),
            (from..from + length).collect::<Vec<_>>()
        );
        assert_eq!(list.get(length), None);
    }

    #[test]
    fn slices_are_windows_over_the_original() {
        let length = 33_900;
        let list = pushed(length);
        for (from, to) in [
            (0, 0),
            (0, 1),
            (5, 5),
            (0, 32),
            (31, 33),
            (32, 64),
            (1000, 1100),
            (1, length),
            (0, length - 1),
            (length - 33, length),
        ] {
            let window = list.slice(from..to).unwrap();
            assert_window(&window, from, to - from);
            if to - from >= 2 {
                let inner = window.slice(1..to - from - 1).unwrap();
                assert_window(&inner, from + 1, to - from - 2);
            }
        }
        assert!(list.slice(std::ops::Range { start: 3, end: 2 }).is_none());
        assert!(list.slice(0..length + 1).is_none());
        assert_sequence(&list, length, 0);
    }

    // Guards: `[..init, last]` followed by appending to `init` must not
    // overwrite `last` in the original list.
    #[test]
    fn append_to_a_prefix_leaves_the_original_unchanged() {
        for length in [1, 2, 32, 33, 64, 65, 1057, 33_825] {
            let list = pushed(length);
            let init = list.slice(0..length - 1).unwrap();
            let left = init.push(1_000_000);
            let right = (0..70).fold(init.clone(), |list, value| list.push(2_000_000 + value));
            assert_sequence(&list, length, 0);
            assert_window(&init, 0, length - 1);
            assert_eq!(
                (left.len(), left.get(length - 1)),
                (length, Some(&1_000_000))
            );
            assert_eq!(right.len(), length + 69);
            assert_eq!(right.get(length - 1), Some(&2_000_000));
            assert_eq!(right.get(length + 68), Some(&2_000_069));
            let rest = list.slice(1..length).unwrap().push(length);
            assert_window(&rest, 1, length);
            assert_sequence(&list, length, 0);
        }
    }

    /// A list of `length` elements whose liveness the caller can observe: element
    /// `index` holds `index`, and the returned `Weak` dies once no list holds it.
    fn tracked(length: usize) -> (PersistentList<Arc<usize>>, Vec<std::sync::Weak<usize>>) {
        let elements = (0..length).map(Arc::new).collect::<Vec<_>>();
        let weak = elements.iter().map(Arc::downgrade).collect();
        (elements.into_iter().collect(), weak)
    }

    fn alive(weak: &[std::sync::Weak<usize>], range: std::ops::Range<usize>) -> Vec<usize> {
        range
            .filter(|&index| weak[index].upgrade().is_some())
            .collect()
    }

    // Guards: `[first, ..rest]` recursion stays linear, and a drained window keeps
    // alive at most four times its own length of elements, so the dropped
    // front of a long list is freed. A copy per view would make this quadratic
    // (about 2 * 10^10 element copies at this length).
    #[test]
    fn a_drained_window_frees_the_dropped_elements() {
        let length = 200_000;
        let (mut rest, weak) = tracked(length);
        let started = std::time::Instant::now();
        for expected in 0..length {
            assert_eq!(rest.first().map(|value| **value), Some(expected));
            rest = rest.slice(1..rest.len()).unwrap();
            let remaining = rest.len();
            if remaining % 1_000 == 0 && remaining > 0 {
                let alive = alive(&weak, 0..length);
                assert!(
                    alive.len() <= 4 * remaining && alive.ends_with(&[length - 1]),
                    "{} elements alive for a window of {remaining}",
                    alive.len()
                );
                assert_eq!(alive.first(), Some(&(length - alive.len())));
            }
        }
        let elapsed = started.elapsed();
        assert!(elapsed.as_secs() < 10, "draining took {elapsed:?}");
        assert!(alive(&weak, 0..length).is_empty());
    }

    #[test]
    fn a_small_window_does_not_retain_its_source() {
        let (list, weak) = tracked(10_000);
        let small = list.slice(5_000..5_100).unwrap();
        drop(list);
        assert_eq!(alive(&weak, 0..10_000), (5_000..5_100).collect::<Vec<_>>());
        assert_eq!(
            small.iter().map(|value| **value).collect::<Vec<_>>(),
            (5_000..5_100).collect::<Vec<_>>()
        );
    }

    // Guards: a window of at least a quarter of its vector shares that vector
    // instead of copying, so the elements outside it stay alive with it.
    #[test]
    fn a_window_of_at_least_a_quarter_shares_its_source() {
        for (from, to) in [(1_000, 9_000), (5_000, 7_500)] {
            let (list, weak) = tracked(10_000);
            let window = list.slice(from..to).unwrap();
            drop(list);
            assert_eq!(alive(&weak, 0..10_000), (0..10_000).collect::<Vec<_>>());
            assert_eq!(
                window.iter().map(|value| **value).collect::<Vec<_>>(),
                (from..to).collect::<Vec<_>>()
            );
            drop(window);
            assert_eq!(alive(&weak, 0..10_000), Vec::<usize>::new());
        }
    }

    // LP-10: `window` holds exactly a quarter of its vector, so each rest view must
    // not share that vector. The window copies its elements once and every later
    // view shares the copy; copying per view would clone 16,383 elements 50,000
    // times (about 8 * 10^8 reference-count updates).
    #[test]
    fn repeated_rest_views_of_a_boundary_window_copy_it_once() {
        let list = pushed(65_536);
        let window = list.slice(49_152..65_536).unwrap();
        drop(list);
        let started = std::time::Instant::now();
        for _ in 0..50_000 {
            let rest = window.clone().slice(1..window.len()).unwrap();
            assert_eq!(rest.first(), Some(&49_153));
        }
        let elapsed = started.elapsed();
        assert!(elapsed.as_secs() < 5, "50,000 rest views took {elapsed:?}");
        assert_window(&window, 49_152, 16_384);
    }

    // Guards: pushing onto a prefix window whose end lies inside the trie (not
    // the tail) writes a path copy, so the shared vector is untouched.
    #[test]
    fn append_to_a_prefix_ending_in_the_trie_leaves_the_original_unchanged() {
        for end in [31, 32, 33, 1000, 1024, 1025, 32_768] {
            // `end + 64` keeps the window above a quarter of its vector, so it
            // shares that vector, and puts `end` before the 32..=64-element tail.
            let length = end + 64;
            let list = pushed(length);
            let prefix = list.slice(0..end).unwrap();
            assert!(end < tail_offset(length));
            let extended = (0..40).fold(prefix.clone(), |list, value| list.push(1_000_000 + value));
            assert_sequence(&list, length, 0);
            assert_window(&prefix, 0, end);
            assert_eq!(extended.len(), end + 40);
            for index in 0..end {
                assert_eq!(extended.get(index), Some(&index));
            }
            for offset in 0..40 {
                assert_eq!(extended.get(end + offset), Some(&(1_000_000 + offset)));
            }
        }
    }
}
