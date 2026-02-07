pub type Weight = u16;

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum Tree<T> {
    Leaf(T),
    Branch(Vec<Tree<T>>),
}

impl<T> Tree<T> {
    #[allow(unused)]
    pub fn map<U>(&self, f: fn(&T) -> U) -> Tree<U> {
        match self {
            Tree::Leaf(x) => Tree::Leaf(f(x)),
            Tree::Branch(branches) => Tree::Branch(
                branches.iter().map(|branch| branch.map(f)).collect(),
            ),
        }
    }

    fn prune_to_size(&mut self) -> usize {
        match self {
            Tree::Leaf(_) => 1,
            Tree::Branch(trees) => {
                let mut i = 0;
                while i < trees.len() {
                    if trees[i].prune_to_size() == 0 {
                        trees.remove(i);
                    } else {
                        i += 1;
                    }
                }

                trees.len()
            }
        }
    }

    pub fn prune(mut self) -> Option<Self> {
        if self.prune_to_size() == 0 {
            None
        } else {
            Some(self)
        }
    }
}

#[cfg(test)]
mod tests {

    use super::Tree::*;

    #[test]
    fn test_prune_non_empty() {
        let actual = Branch(vec![
            (Leaf(1)),
            (Branch(vec![(Leaf(2)), (Leaf(3)), (Branch(vec![]))])),
            (Branch(vec![])),
        ])
        .prune()
        .unwrap();
        let expected =
            Branch(vec![(Leaf(1)), (Branch(vec![(Leaf(2)), (Leaf(3))]))]);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_prune_empty() {
        let actual = Branch::<()>(vec![]).prune();
        assert_eq!(actual, None);
    }

    #[test]
    fn test_prune_empty_subtrees() {
        let actual = Branch::<()>(vec![Branch(vec![]), Branch(vec![])]).prune();
        assert_eq!(actual, None);
    }
}
