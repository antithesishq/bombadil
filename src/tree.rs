use anyhow::{Result, bail};
use rand::Rng;
use serde::{Deserialize, Serialize};

pub type Weight = u16;

#[derive(PartialEq, Eq, Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Tree<T> {
    Leaf { value: T },
    Branch { branches: Vec<(Weight, Tree<T>)> },
}

impl<T> Tree<T> {
    pub fn try_map<U, E>(
        self,
        f: &mut impl FnMut(T) -> Result<U, E>,
    ) -> Result<Tree<U>, E> {
        match self {
            Tree::Leaf { value } => Ok(Tree::Leaf { value: f(value)? }),
            Tree::Branch { branches } => Ok(Tree::Branch {
                branches: branches
                    .into_iter()
                    .map(|(w, t)| Ok((w, t.try_map(f)?)))
                    .collect::<Result<_, E>>()?,
            }),
        }
    }

    pub fn filter(self, predicate: &impl Fn(&T) -> bool) -> Self {
        match self {
            Tree::Leaf { value } => {
                if predicate(&value) {
                    Tree::Leaf { value }
                } else {
                    Tree::Branch { branches: vec![] }
                }
            }
            Tree::Branch { branches } => Tree::Branch {
                branches: branches
                    .into_iter()
                    .map(|(w, t)| (w, t.filter(predicate)))
                    .collect(),
            },
        }
    }

    fn prune_to_size(&mut self) -> usize {
        match self {
            Tree::Leaf { .. } => 1,
            Tree::Branch { branches } => {
                let mut i = 0;
                while i < branches.len() {
                    if branches[i].1.prune_to_size() == 0 {
                        branches.remove(i);
                    } else {
                        i += 1;
                    }
                }
                branches.len()
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

    pub fn pick(&self, rng: &mut impl Rng) -> Result<&T> {
        match self {
            Tree::Leaf { value } => Ok(value),
            Tree::Branch { branches } => {
                let total: u64 = branches.iter().map(|(w, _)| *w as u64).sum();
                if total == 0 {
                    bail!("total of weights is zero")
                }
                let mut choice = rng.random_range(0..total);
                for (weight, subtree) in branches {
                    let w = *weight as u64;
                    if choice < w {
                        return subtree.pick(rng);
                    }
                    choice -= w;
                }
                bail!("BUG: no pick available")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Tree::*;

    #[test]
    fn test_prune_non_empty() {
        let actual = Branch {
            branches: vec![
                (1, Leaf { value: 1 }),
                (
                    1,
                    Branch {
                        branches: vec![
                            (1, Leaf { value: 2 }),
                            (1, Leaf { value: 3 }),
                            (1, Branch { branches: vec![] }),
                        ],
                    },
                ),
                (1, Branch { branches: vec![] }),
            ],
        }
        .prune()
        .unwrap();
        let expected = Branch {
            branches: vec![
                (1, Leaf { value: 1 }),
                (
                    1,
                    Branch {
                        branches: vec![
                            (1, Leaf { value: 2 }),
                            (1, Leaf { value: 3 }),
                        ],
                    },
                ),
            ],
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_prune_empty() {
        let actual = Branch::<()> { branches: vec![] }.prune();
        assert_eq!(actual, None);
    }

    #[test]
    fn test_prune_empty_subtrees() {
        let actual = Branch::<()> {
            branches: vec![
                (1, Branch { branches: vec![] }),
                (1, Branch { branches: vec![] }),
            ],
        }
        .prune();
        assert_eq!(actual, None);
    }

    #[test]
    fn test_filter() {
        let tree = Branch {
            branches: vec![
                (1, Leaf { value: 1 }),
                (1, Leaf { value: 2 }),
                (1, Leaf { value: 3 }),
            ],
        };
        let filtered = tree.filter(&|x| *x > 1).prune().unwrap();
        let expected = Branch {
            branches: vec![(1, Leaf { value: 2 }), (1, Leaf { value: 3 })],
        };
        assert_eq!(filtered, expected);
    }

    #[test]
    fn test_try_map_ok() {
        let tree = Branch {
            branches: vec![(1, Leaf { value: 1 }), (2, Leaf { value: 2 })],
        };
        let mapped = tree
            .try_map::<String, ()>(&mut |x| Ok(x.to_string()))
            .unwrap();
        let expected = Branch {
            branches: vec![
                (
                    1,
                    Leaf {
                        value: "1".to_string(),
                    },
                ),
                (
                    2,
                    Leaf {
                        value: "2".to_string(),
                    },
                ),
            ],
        };
        assert_eq!(mapped, expected);
    }

    #[test]
    fn test_try_map_err() {
        let tree = Branch {
            branches: vec![(1, Leaf { value: 1 }), (2, Leaf { value: 2 })],
        };
        let result = tree.try_map::<String, &str>(&mut |x| {
            if x == 2 {
                Err("bad")
            } else {
                Ok(x.to_string())
            }
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_pick_single_leaf() {
        let tree = Leaf { value: 42 };
        let mut rng = rand::rng();
        assert_eq!(*tree.pick(&mut rng).unwrap(), 42);
    }

    #[test]
    fn test_pick_weighted() {
        // With weight 0 for "a" and weight 1 for "b", should always pick "b"
        // Actually weight 0 would cause issues, use weight 1000 vs 1
        let tree = Branch {
            branches: vec![
                (1000, Leaf { value: "heavy" }),
                (1, Leaf { value: "light" }),
            ],
        };
        let mut rng = rand::rng();
        let mut heavy_count = 0;
        for _ in 0..100 {
            if *tree.pick(&mut rng).unwrap() == "heavy" {
                heavy_count += 1;
            }
        }
        // With 1000:1 ratio, heavy should dominate
        assert!(heavy_count > 80);
    }
}
