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
    pub fn with_capacity(capacity: usize, value: T) -> Self {
        let mut nodes = Vec::with_capacity(capacity);
        nodes.push(Node {
            value,
            parent: None,
            children: Vec::new(),
        });
        Tree { nodes }
    }

    pub fn root(&self) -> NodeId {
        NodeId { index: 0 }
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
}

#[cfg(test)]
mod tests {}
