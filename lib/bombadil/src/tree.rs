use std::collections::VecDeque;

use anyhow::{Result, anyhow};
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

    /// Maps the leaves `f` accepts. A rejected leaf becomes an empty branch,
    /// so the weights of the others are untouched.
    pub fn filter_map<U>(self, f: &mut impl FnMut(T) -> Option<U>) -> Tree<U> {
        match self {
            Tree::Leaf { value } => match f(value) {
                Some(value) => Tree::Leaf { value },
                None => Tree::Branch { branches: vec![] },
            },
            Tree::Branch { branches } => Tree::Branch {
                branches: branches
                    .into_iter()
                    .map(|(w, t)| (w, t.filter_map(f)))
                    .collect(),
            },
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

    /// Walks the whole tree and allocates. Call once and share the result
    /// where possible.
    pub fn leaves(&self) -> Leaves<'_, T> {
        struct Frame<'a, T> {
            branches: &'a [(Weight, Tree<T>)],
            next: usize,
            leaves_start: usize,
            leaf_counts_start: usize,
            total: u64,
        }

        let branches = match self {
            Tree::Leaf { value } => {
                return Leaves {
                    leaves: vec![(value, 1.0)],
                };
            }
            Tree::Branch { branches } => branches,
        };
        let mut leaves: Vec<(&T, f64)> = Vec::new();
        // How many leaves each visited child of the open branches has.
        let mut leaf_counts: Vec<usize> = Vec::new();
        let mut stack = vec![Frame {
            branches,
            next: 0,
            leaves_start: 0,
            leaf_counts_start: 0,
            total: 0,
        }];

        while let Some(frame) = stack.last_mut() {
            if let Some((weight, subtree)) = frame.branches.get(frame.next) {
                frame.next += 1;
                match subtree {
                    Tree::Leaf { value } => {
                        leaves.push((value, 1.0));
                        leaf_counts.push(1);
                        frame.total += *weight as u64;
                    }
                    Tree::Branch { branches } => stack.push(Frame {
                        branches,
                        next: 0,
                        leaves_start: leaves.len(),
                        leaf_counts_start: leaf_counts.len(),
                        total: 0,
                    }),
                }
                continue;
            }

            // The leaves of a subtree that cannot be picked are already at
            // zero, so only the weights of the others are in `total`.
            let Frame {
                branches,
                leaves_start,
                leaf_counts_start,
                total,
                ..
            } = stack.pop().expect("should have a frame");
            let mut start = leaves_start;
            for ((weight, _), leaf_count) in
                branches.iter().zip(leaf_counts.drain(leaf_counts_start..))
            {
                let share = if total == 0 {
                    0.0
                } else {
                    *weight as f64 / total as f64
                };
                let end = start + leaf_count;
                for (_, probability) in &mut leaves[start..end] {
                    *probability *= share;
                }
                start = end;
            }
            if let Some(parent) = stack.last_mut() {
                leaf_counts.push(leaves.len() - leaves_start);
                if total > 0 {
                    parent.total += parent.branches[parent.next - 1].0 as u64;
                }
            }
        }
        Leaves { leaves }
    }

    /// Whether any leaf satisfies `predicate`, stopping at the first.
    pub fn any(&self, predicate: impl Fn(&T) -> bool) -> bool {
        let mut stack = vec![self];
        while let Some(next) = stack.pop() {
            match next {
                Self::Leaf { value } => {
                    if predicate(value) {
                        return true;
                    }
                }
                Self::Branch { branches } => {
                    stack.extend(branches.iter().map(|(_, tree)| tree))
                }
            }
        }
        false
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

/// Every leaf of a tree in depth-first order, with the probability that
/// picking selects it. A subtree with nothing to pick under it counts as
/// weight zero in its parent, as if it had been pruned, so the probabilities
/// sum to one unless nothing can be picked, and then they are all zero.
#[derive(Clone, Debug, PartialEq)]
pub struct Leaves<'a, T> {
    leaves: Vec<(&'a T, f64)>,
}

impl<'a, T> Leaves<'a, T> {
    /// Leaves with probabilities proportional to their weights, which are
    /// not negative. `None` when no weight is positive.
    pub fn from_weights(
        weights: impl IntoIterator<Item = (&'a T, f64)>,
    ) -> Option<Self> {
        let mut leaves: Vec<(&'a T, f64)> = weights.into_iter().collect();
        let total: f64 = leaves.iter().map(|(_, weight)| *weight).sum();
        if total <= 0.0 {
            return None;
        }
        for (_, weight) in &mut leaves {
            *weight /= total;
        }
        Some(Leaves { leaves })
    }

    pub fn as_slice(&self) -> &[(&'a T, f64)] {
        &self.leaves
    }

    pub fn pick<R: TryRng + RngExt>(&self, rng: &mut R) -> Result<&'a T> {
        let mut remaining = rng.random::<f64>();
        let mut last = None;
        for &(value, probability) in &self.leaves {
            if probability > 0.0 {
                if remaining < probability {
                    return Ok(value);
                }
                remaining -= probability;
                last = Some(value);
            }
        }
        last.ok_or_else(|| anyhow!("no leaf can be picked"))
    }
}

#[cfg(test)]
mod tests {
    use super::Tree::*;
    use super::{Leaves, Tree, Weight};
    use hegel::TestCase;
    use hegel::generators::{booleans, integers, vecs};
    use rand::{SeedableRng, rngs::StdRng};

    hegel::pretty_print_as_debug!(Tree<usize>);

    #[hegel::composite]
    fn weights(tc: &TestCase, minimum: Weight) -> Weight {
        if minimum == 0 && tc.draw_silent(booleans()) {
            0
        } else {
            tc.draw_silent(integers::<Weight>().min_value(minimum))
        }
    }

    /// A tree whose leaves are numbered in depth-first order. When `pickable`
    /// is false, every leaf sits below a zero weight or the tree has no leaf
    /// at all. `expected` receives each leaf's probability, computed on the
    /// way down from choices made before the subtrees are built; `probability`
    /// is that of reaching this subtree.
    fn subtree(
        tc: &TestCase,
        depth: usize,
        pickable: bool,
        probability: f64,
        expected: &mut Vec<f64>,
    ) -> Tree<usize> {
        if pickable && (depth == 0 || tc.draw_silent(booleans())) {
            expected.push(probability);
            return Leaf {
                value: expected.len() - 1,
            };
        }
        if depth == 0 {
            return Branch { branches: vec![] };
        }
        let mut children: Vec<(Weight, bool)> = if pickable {
            let size =
                tc.draw_silent(integers::<usize>().min_value(1).max_value(4));
            (0..size)
                .map(|_| {
                    (tc.draw_silent(weights(0)), tc.draw_silent(booleans()))
                })
                .collect()
        } else {
            let size = tc.draw_silent(integers::<usize>().max_value(3));
            (0..size)
                .map(|_| {
                    if tc.draw_silent(booleans()) {
                        (0, tc.draw_silent(booleans()))
                    } else {
                        (tc.draw_silent(weights(0)), false)
                    }
                })
                .collect()
        };
        if pickable {
            let index = tc
                .draw_silent(integers::<usize>().max_value(children.len() - 1));
            children[index] = (tc.draw_silent(weights(1)), true);
        }
        let total: u64 = children
            .iter()
            .filter(|(_, pickable)| *pickable)
            .map(|(weight, _)| *weight as u64)
            .sum();
        let branches = children
            .into_iter()
            .map(|(weight, pickable)| {
                let share = if pickable && total > 0 {
                    weight as f64 / total as f64
                } else {
                    0.0
                };
                let subtree = subtree(
                    tc,
                    depth - 1,
                    pickable,
                    probability * share,
                    expected,
                );
                (weight, subtree)
            })
            .collect();
        Branch { branches }
    }

    /// A tree and the probability of each of its leaves.
    #[hegel::composite]
    fn trees(tc: &TestCase, pickable: bool) -> (Tree<usize>, Vec<f64>) {
        let mut expected = vec![];
        let tree = subtree(tc, 3, pickable, 1.0, &mut expected);
        (tree, expected)
    }

    #[hegel::test]
    fn leaves_are_listed_depth_first_with_their_probabilities(tc: TestCase) {
        let pickable = tc.draw(booleans());
        let (tree, expected) = tc.draw(trees(pickable));

        let leaves = tree.leaves();
        let values: Vec<usize> =
            leaves.as_slice().iter().map(|(value, _)| **value).collect();
        assert_eq!(values, (0..expected.len()).collect::<Vec<_>>());
        for ((value, probability), expected) in
            leaves.as_slice().iter().zip(&expected)
        {
            if *expected == 0.0 {
                assert_eq!(*probability, 0.0, "leaf {value}");
            } else {
                assert!(
                    (probability - expected).abs() < 1e-9,
                    "leaf {value} has probability {probability}, expected \
                     {expected}"
                );
            }
        }
    }

    #[hegel::test]
    fn any_finds_exactly_the_leaves_there_are(tc: TestCase) {
        let pickable = tc.draw(booleans());
        let (tree, expected) = tc.draw(trees(pickable));
        let value = tc.draw(integers::<usize>().max_value(expected.len()));

        assert_eq!(tree.any(|leaf| *leaf == value), value < expected.len());
    }

    #[hegel::test]
    fn probabilities_sum_to_one_or_are_all_zero_when_pick_fails(tc: TestCase) {
        let pickable = tc.draw(booleans());
        let (tree, _) = tc.draw(trees(pickable));
        let seed = tc.draw(integers::<u64>());

        let leaves = tree.leaves();
        let total: f64 = leaves.as_slice().iter().map(|(_, p)| *p).sum();
        let picked = leaves.pick(&mut StdRng::seed_from_u64(seed));
        if pickable {
            assert!((total - 1.0).abs() < 1e-9, "total is {total}");
            let picked = *picked.unwrap();
            assert!(leaves.as_slice()[picked].1 > 0.0, "picked leaf {picked}");
        } else {
            assert!(leaves.as_slice().iter().all(|(_, p)| *p == 0.0));
            assert!(picked.is_err());
        }
    }

    #[hegel::test]
    fn pick_frequencies_match_the_probabilities(tc: TestCase) {
        const SAMPLES: usize = 1000;
        const TOLERANCE: f64 = 0.1;

        let (tree, _) = tc.draw(trees(true));
        let mut rng = StdRng::seed_from_u64(tc.draw(integers::<u64>()));

        let leaves = tree.leaves();
        let mut picked = vec![0usize; leaves.as_slice().len()];
        for _ in 0..SAMPLES {
            picked[*leaves.pick(&mut rng).unwrap()] += 1;
        }

        for (value, probability) in leaves.as_slice() {
            let count = picked[**value];
            if *probability == 0.0 {
                assert_eq!(count, 0, "leaf {value} has probability zero");
            }
            let frequency = count as f64 / SAMPLES as f64;
            assert!(
                (frequency - probability).abs() < TOLERANCE,
                "leaf {value} (probability {probability}) was picked \
                 {frequency} of the time",
            );
        }
    }

    #[hegel::test]
    fn leaves_from_weights_are_proportional_to_the_weights(tc: TestCase) {
        let weights: Vec<u32> = tc.draw(vecs(integers::<u32>()));
        let values: Vec<usize> = (0..weights.len()).collect();
        let total: f64 = weights.iter().map(|weight| *weight as f64).sum();

        let leaves = Leaves::from_weights(
            values
                .iter()
                .zip(weights.iter().map(|weight| *weight as f64)),
        );
        let Some(leaves) = leaves else {
            assert_eq!(total, 0.0);
            return;
        };
        let listed: Vec<usize> =
            leaves.as_slice().iter().map(|(value, _)| **value).collect();
        assert_eq!(listed, values);
        for (value, probability) in leaves.as_slice() {
            let expected = weights[**value] as f64 / total;
            assert!(
                (probability - expected).abs() < 1e-12,
                "leaf {value} has probability {probability}, expected \
                 {expected}"
            );
        }
    }

    #[hegel::test]
    fn filter_map_keeps_the_accepted_leaves_in_place(tc: TestCase) {
        let pickable = tc.draw(booleans());
        let (tree, expected) = tc.draw(trees(pickable));
        let accepted: Vec<bool> =
            expected.iter().map(|_| tc.draw(booleans())).collect();

        let mapped = tree.clone().filter_map(&mut |value| {
            accepted[value].then(|| value.to_string())
        });

        // The same shape and weights as the input, leaf by leaf.
        fn check(
            input: &Tree<usize>,
            output: &Tree<String>,
            accepted: &[bool],
        ) {
            match (input, output) {
                (Leaf { value }, Leaf { value: mapped }) => {
                    assert!(accepted[*value], "leaf {value} was rejected");
                    assert_eq!(*mapped, value.to_string());
                }
                (Leaf { value }, Branch { branches }) => {
                    assert!(!accepted[*value], "leaf {value} was accepted");
                    assert!(branches.is_empty());
                }
                (Branch { branches: inputs }, Branch { branches: outputs }) => {
                    assert_eq!(inputs.len(), outputs.len());
                    for ((weight, input), (mapped_weight, output)) in
                        inputs.iter().zip(outputs)
                    {
                        assert_eq!(weight, mapped_weight);
                        check(input, output, accepted);
                    }
                }
                (Branch { .. }, Leaf { value }) => {
                    panic!("a branch became the leaf {value}")
                }
            }
        }
        check(&tree, &mapped, &accepted);
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
