use super::{
    node::{Key, Node, NodeType, Value},
    StableBTreeMap,
};
use crate::{types::NULL, Address, Memory};

// An indicator of the current position in the map.
enum Cursor {
    Address(Address),
    Node { node: Node, next: Index },
}

// An index into a node's child or entry.
enum Index {
    Child(usize),
    Entry(usize),
}

/// An iterator over the entries of a [`StableBTreeMap`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct Iter<'a, M: Memory> {
    // A reference to the map being iterated on.
    map: &'a StableBTreeMap<M>,

    // A stack of cursors indicating the current position in the tree.
    cursors: Vec<Cursor>,
}

impl<'a, M: Memory> Iter<'a, M> {
    pub(crate) fn new(map: &'a StableBTreeMap<M>) -> Self {
        Self {
            map,
            // Initialize the cursors with the address of the root of the map.
            cursors: vec![Cursor::Address(map.root_addr)],
        }
    }
}

impl<M: Memory + Clone> Iterator for Iter<'_, M> {
    type Item = (Key, Value);

    fn next(&mut self) -> Option<Self::Item> {
        match self.cursors.pop() {
            Some(Cursor::Address(address)) => {
                if address != NULL {
                    // Load the node at the given address, and add it to the cursors.
                    let node = self.map.load_node(address);
                    self.cursors.push(Cursor::Node {
                        next: match node.node_type {
                            // Iterate on internal nodes starting from the first child.
                            NodeType::Internal => Index::Child(0),
                            // Iterate on leaf nodes starting from the first entry.
                            NodeType::Leaf => Index::Entry(0),
                        },
                        node,
                    });
                }
                self.next()
            }

            Some(Cursor::Node {
                node,
                next: Index::Child(child_idx),
            }) => {
                let child_address = *node
                    .children
                    .get(child_idx)
                    .expect("Iterating over children went out of bounds.");

                // After iterating on the child, iterate on the next _entry_ in this node.
                // The entry immediately after the child has the same index as the child's.
                self.cursors.push(Cursor::Node {
                    node,
                    next: Index::Entry(child_idx),
                });

                // Add the child to the top of the cursors to be iterated on first.
                self.cursors.push(Cursor::Address(child_address));

                self.next()
            }

            Some(Cursor::Node {
                mut node,
                next: Index::Entry(entry_idx),
            }) => {
                if entry_idx >= node.entries.len() {
                    // No more entries to iterate on in this node.
                    return self.next();
                }

                // Take the entry from the node. It's swapped with an empty element to
                // avoid cloning.
                let entry = node.swap_entry(entry_idx, (vec![], vec![]));

                // Add to the cursors the next element to be traversed.
                self.cursors.push(Cursor::Node {
                    next: match node.node_type {
                        // If this is an internal node, add the next child to the cursors.
                        NodeType::Internal => Index::Child(entry_idx + 1),
                        // If this is a leaf node, add the next entry to the cursors.
                        NodeType::Leaf => Index::Entry(entry_idx + 1),
                    },
                    node,
                });
                Some(entry)
            }
            None => {
                // The cursors are empty. Iteration is complete.
                None
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::btreemap::node::CAPACITY;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn make_memory() -> Rc<RefCell<Vec<u8>>> {
        Rc::new(RefCell::new(Vec::new()))
    }

    #[test]
    fn iterate_leaf() {
        let mem = make_memory();
        let mut btree = StableBTreeMap::new(mem, 1, 1);

        for i in 0..CAPACITY as u8 {
            btree.insert(vec![i], vec![i + 1]).unwrap();
        }

        let mut i = 0;
        for (key, value) in btree.iter() {
            assert_eq!(key, vec![i]);
            assert_eq!(value, vec![i + 1]);
            i += 1;
        }

        assert_eq!(i, CAPACITY as u8);
    }

    #[test]
    fn iterate_children() {
        let mem = make_memory();
        let mut btree = StableBTreeMap::new(mem, 1, 1);

        // Insert the elements in reverse order.
        for i in (0..100).rev() {
            btree.insert(vec![i], vec![i + 1]).unwrap();
        }

        // Iteration should be in ascending order.
        let mut i = 0;
        for (key, value) in btree.iter() {
            assert_eq!(key, vec![i]);
            assert_eq!(value, vec![i + 1]);
            i += 1;
        }

        assert_eq!(i, 100);
    }
}
