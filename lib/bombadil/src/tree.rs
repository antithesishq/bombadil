use std::collections::VecDeque;

use anyhow::{Result, anyhow, bail};
use rand::{RngExt, TryRng};
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

    /// Replace each leaf with the weighted sub-leaves `f` expands it into. An
    /// empty expansion keeps the leaf unchanged.
    pub fn flat_map(self, f: &impl Fn(&T) -> Vec<(Weight, T)>) -> Self {
        match self {
            Tree::Leaf { value } => {
                let expansion = f(&value);
                if expansion.is_empty() {
                    Tree::Leaf { value }
                } else {
                    Tree::Branch {
                        branches: expansion
                            .into_iter()
                            .map(|(w, v)| (w, Tree::Leaf { value: v }))
                            .collect(),
                    }
                }
            }
            Tree::Branch { branches } => Tree::Branch {
                branches: branches
                    .into_iter()
                    .map(|(w, t)| (w, t.flat_map(f)))
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

    /// Picks a leaf with the probability `leaves_with_probabilities` gives
    /// it. Probability lost under branches whose weights are all zero is
    /// redistributed proportionally over the other leaves.
    pub fn pick<R: TryRng + RngExt>(&self, rng: &mut R) -> Result<&T> {
        let leaves = self.leaves_with_probabilities();
        let total: f64 = leaves.iter().map(|(_, p)| *p).sum();
        if total <= 0.0 {
            bail!("no leaf with a positive probability to pick")
        }
        let mut remaining = rng.random::<f64>() * total;
        let mut last_positive = None;
        for (value, probability) in leaves {
            if probability <= 0.0 {
                continue;
            }
            if remaining < probability {
                return Ok(value);
            }
            remaining -= probability;
            last_positive = Some(value);
        }
        // Rounding left a sliver past the last interval.
        last_positive.ok_or_else(|| anyhow!("BUG: no pick available"))
    }

    /// Every leaf with the probability that weighted picking selects it.
    /// Leaves under zero-weight branches get probability zero.
    pub fn leaves_with_probabilities(&self) -> Vec<(&T, f64)> {
        let mut result = vec![];
        let mut stack = vec![(self, 1.0)];
        while let Some((tree, probability)) = stack.pop() {
            match tree {
                Tree::Leaf { value } => result.push((value, probability)),
                Tree::Branch { branches } => {
                    let total: u64 =
                        branches.iter().map(|(w, _)| *w as u64).sum();
                    for (weight, subtree) in branches.iter().rev() {
                        let share = if total == 0 {
                            0.0
                        } else {
                            *weight as f64 / total as f64
                        };
                        stack.push((subtree, probability * share));
                    }
                }
            }
        }
        result
    }

    pub fn values(&self) -> Vec<&T> {
        let mut result: Vec<&T> = vec![];
        let mut queue = VecDeque::new();
        queue.push_front(self);
        while let Some(next) = queue.pop_front() {
            match next {
                Self::Leaf { value } => result.push(value),
                Self::Branch { branches } => {
                    queue.extend(branches.iter().map(|(_, tree)| tree))
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::Tree::*;
    use super::{Tree, Weight};
    use hegel::TestCase;
    use hegel::generators::{booleans, integers};
    use rand::{RngExt, SeedableRng, rngs::StdRng};

    /// A tree whose leaves are numbered in depth-first order, where every
    /// branch has at least one child and a positive total weight, so that
    /// picking always succeeds. Individual weights may be zero.
    fn trees(
        tc: &TestCase,
        depth: usize,
        next_leaf: &mut usize,
    ) -> Tree<usize> {
        if depth == 0 || tc.draw_silent(booleans()) {
            let value = *next_leaf;
            *next_leaf += 1;
            return Leaf { value };
        }
        let size =
            tc.draw_silent(integers::<usize>().min_value(1).max_value(4));
        let branches: Vec<(Weight, Tree<usize>)> = (0..size)
            .map(|_| {
                let weight = tc.draw_silent(integers::<Weight>());
                (weight, trees(tc, depth - 1, next_leaf))
            })
            .collect();
        tc.assume(branches.iter().any(|(weight, _)| *weight > 0));
        Branch { branches }
    }

    /// The reference: walks down the tree, at each branch drawing an integer
    /// below the total weight to choose the child.
    fn pick_by_walking<'a>(
        tree: &'a Tree<usize>,
        rng: &mut StdRng,
    ) -> &'a usize {
        match tree {
            Leaf { value } => value,
            Branch { branches } => {
                let total: u64 = branches.iter().map(|(w, _)| *w as u64).sum();
                let mut choice = rng.random_range(0..total);
                for (weight, subtree) in branches {
                    let weight = *weight as u64;
                    if choice < weight {
                        return pick_by_walking(subtree, rng);
                    }
                    choice -= weight;
                }
                unreachable!("choice is below the total weight")
            }
        }
    }

    #[hegel::test]
    fn pick_agrees_with_walking_the_tree(tc: TestCase) {
        const SAMPLES: usize = 1000;
        const TOLERANCE: f64 = 0.12;

        let mut leaf_count = 0;
        let tree = trees(&tc, 3, &mut leaf_count);
        let seed = tc.draw(integers::<u64>());
        tc.note(&format!("tree: {tree:?}"));

        let leaves = tree.leaves_with_probabilities();
        let values: Vec<usize> =
            leaves.iter().map(|(value, _)| **value).collect();
        assert_eq!(values, (0..leaf_count).collect::<Vec<_>>());
        let probabilities: Vec<f64> =
            leaves.iter().map(|(_, probability)| *probability).collect();
        assert!((probabilities.iter().sum::<f64>() - 1.0).abs() < 1e-9);

        let mut rng = StdRng::seed_from_u64(seed);
        let mut picked = vec![0usize; leaf_count];
        let mut expected = vec![0usize; leaf_count];
        for _ in 0..SAMPLES {
            picked[*tree.pick(&mut rng).unwrap()] += 1;
            expected[*pick_by_walking(&tree, &mut rng)] += 1;
        }

        for leaf in 0..leaf_count {
            if probabilities[leaf] == 0.0 {
                assert_eq!(picked[leaf], 0, "leaf {leaf} has probability zero");
            }
            let picked = picked[leaf] as f64 / SAMPLES as f64;
            let expected = expected[leaf] as f64 / SAMPLES as f64;
            assert!(
                (picked - expected).abs() < TOLERANCE,
                "leaf {leaf} (probability {}): pick chose it {picked} of the \
                 time, walking the tree {expected}",
                probabilities[leaf],
            );
        }
    }

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

    #[test]
    fn test_values() {
        let tree = Branch {
            branches: vec![
                (1, Leaf { value: 1 }),
                (2, Leaf { value: 2 }),
                (
                    3,
                    Branch {
                        branches: vec![(1, Leaf { value: 3 })],
                    },
                ),
            ],
        };
        assert_eq!(
            tree.values().iter().copied().copied().collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }
}
