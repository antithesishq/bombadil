#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NodeId {
    index: usize,
}

pub struct Node<T> {
    value: T,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
}

pub struct Tree<T> {
    nodes: Vec<Node<T>>,
}

impl<T> Tree<T> {
    pub fn new(value: T) -> Self {
        Self::with_capacity(64, value)
    }

    pub fn with_capacity(capacity: usize, value: T) -> Self {
        let mut nodes = Vec::with_capacity(capacity);
        nodes.push(Node {
            value,
            parent: None,
            children: Vec::new(),
        });
        Tree { nodes }
    }

    pub fn root_id(&self) -> NodeId {
        NodeId { index: 0 }
    }

    pub fn root(&self) -> &Node<T> {
        &self.nodes[0]
    }

    pub fn root_mut(&mut self) -> &mut Node<T> {
        &mut self.nodes[0]
    }

    pub fn insert(&mut self, value: T, parent: NodeId) -> NodeId {
        assert!(parent.index < self.nodes.len());
        let id = NodeId {
            index: self.nodes.len(),
        };
        self.nodes.push(Node {
            value,
            parent: Some(parent),
            children: Vec::new(),
        });
        self.nodes[parent.index].children.push(id);
        id
    }
}

impl<T> std::ops::Index<NodeId> for Tree<T> {
    type Output = Node<T>;
    fn index(&self, id: NodeId) -> &Self::Output {
        assert!(
            id.index < self.nodes.len(),
            "node {id:?} is not in the tree"
        );
        &self.nodes[id.index]
    }
}

impl<T> std::ops::IndexMut<NodeId> for Tree<T> {
    fn index_mut(&mut self, id: NodeId) -> &mut Self::Output {
        assert!(
            id.index < self.nodes.len(),
            "node {id:?} is not in the tree"
        );
        &mut self.nodes[id.index]
    }
}

impl<T> Node<T> {
    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    pub fn children(&self) -> &[NodeId] {
        &self.children
    }
}

#[cfg(test)]
mod tests {
    use crate::NodeId;

    use super::Tree;
    use hegel::{TestCase, generators::integers};

    /// This creates a series of (parent, child) ID pairs to insert into a
    /// tree. The children (second component of each pair) is a full ordered series
    /// starting at 1. It assumes the root will always be created with the value 0.
    /// The parent ID in the pairs are within [0, child).
    #[hegel::composite]
    fn insertions(tc: TestCase) -> Vec<(u8, u8)> {
        let mut result = vec![];
        for id in 1..tc.draw(integers().min_value(1)) {
            let parent = tc.draw(integers().min_value(0).max_value(id - 1));
            result.push((parent, id));
        }
        result
    }

    #[hegel::test]
    fn inserts_into_parents(tc: TestCase) {
        let insertions = tc.draw(insertions());
        let mut tree = Tree::new(0);
        let mut node_id_map: Vec<NodeId> = vec![tree.root_id()];

        // Fill the tree.
        for (parent, child) in &insertions {
            let parent_id = node_id_map[*parent as usize];
            let child_id = tree.insert(*child, parent_id);
            node_id_map.push(child_id);
        }

        // Check that all parent/child relations are there.
        for (parent, child) in &insertions {
            let child_id = node_id_map[*child as usize];

            let child_node = &tree[child_id];
            assert_eq!(child_node.value(), child);

            let parent_node = &tree[child_node.parent().unwrap()];
            assert_eq!(parent_node.value(), parent);
            assert!(parent_node.children().contains(&child_id));
        }
    }
}
