//! State hashes and action keys. Traces record each step's action vocabulary
//! (the available templates and the one taken) so that action weights can be
//! learned from them; the hashes are derived from the recorded state, here
//! and in training, by the same code.
//!
//! Each driver describes a state by named 64-bit hashes (e.g. `location`,
//! `snapshot.<name>`), equal exactly when the hashed value is equal. Training
//! decides which of them identify a state. Actions are identified by their
//! template's `category_hash`. Every hash here has a fixed algorithm (FNV-1a,
//! SplitMix64's finalizer), so recorded hashes and keys are the same across
//! Rust versions, unlike `DefaultHasher`'s.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hasher;

use serde::Serialize;

use crate::driver::{ActionTemplate, RunState};
use crate::specification::domain::Snapshot;
use crate::tree::Tree;

/// What a trace records about each step for training.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct StepModel {
    /// Key ([`action_key`]) of each available action template, with the
    /// probability the spec weights give it.
    pub actions_available: Vec<(String, f64)>,
    /// Key of the template, among the previous step's available ones, that
    /// generated the action leading to this state.
    pub action_taken: Option<String>,
}

impl StepModel {
    /// `previous` is the last step's available actions and the action taken
    /// from them.
    pub fn new<A, T: ActionTemplate<A> + Clone>(
        tree: &Tree<T>,
        previous: Option<(&Tree<T>, &A)>,
    ) -> Self {
        // Picking prunes empty branches first, so probabilities must too.
        let actions_available = tree
            .clone()
            .prune()
            .iter()
            .flat_map(|tree| tree.leaves_with_probabilities())
            .map(|(template, probability)| (action_key(template), probability))
            .collect();
        let action_taken = previous.and_then(|(tree, action)| {
            tree.values()
                .into_iter()
                .find(|template| template.accepts(action))
                .map(|template| action_key(template))
        });
        StepModel {
            actions_available,
            action_taken,
        }
    }
}

/// The template's `category_hash` under [`StableHasher`], as 16 hex digits.
pub fn action_key<A>(template: &impl ActionTemplate<A>) -> String {
    let mut hasher = StableHasher::default();
    template.category_hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// FNV-1a. Action keys are recorded in traces and weights files, so they must
/// not depend on the Rust version as `std::hash::DefaultHasher` does.
pub struct StableHasher(u64);

impl Default for StableHasher {
    fn default() -> Self {
        StableHasher(FNV_OFFSET)
    }
}

impl Hasher for StableHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0 = fnv1a64_from(self.0, bytes);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// The driver's hashes of the state, plus, for each named snapshot,
/// `snapshot.<name>`: the hash of its JSON leaves ([`feature_hash`] of the
/// `path=value` lines, see `json_leaves`).
pub fn state_hashes(
    state: &(impl RunState + ?Sized),
    snapshots: &[Snapshot],
) -> BTreeMap<String, u64> {
    let mut hashes = state.hashes();
    for snapshot in snapshots {
        let Some(name) = &snapshot.name else { continue };
        let mut leaves = Vec::new();
        json_leaves(&snapshot.value, "", &mut leaves);
        if let Some(hash) = feature_hash(leaves.iter().map(String::as_str)) {
            hashes.insert(format!("snapshot.{name}"), hash);
        }
    }
    hashes
}

/// The hash of a sequence of string features: FNV-1a over the features,
/// each followed by a newline, then SplitMix64's finalizer. Equal exactly
/// when the sequences are equal; `None` for no features.
pub fn feature_hash<'a>(
    features: impl IntoIterator<Item = &'a str>,
) -> Option<u64> {
    let mut hash = FNV_OFFSET;
    let mut any = false;
    for feature in features {
        any = true;
        hash = fnv1a64_from(hash, feature.as_bytes());
        hash = fnv1a64_from(hash, b"\n");
    }
    any.then(|| mix64(hash))
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_from(FNV_OFFSET, bytes)
}

fn fnv1a64_from(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn mix64(x: u64) -> u64 {
    let mut h = x.wrapping_add(0x9e3779b97f4a7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
    h ^ (h >> 31)
}

/// `path=value` for every scalar in a JSON value, e.g. `{"a": [1]}` gives
/// `a.0=1`. Object keys are visited in sorted order, as the order is part of
/// the hash.
fn json_leaves(value: &serde_json::Value, path: &str, out: &mut Vec<String>) {
    let child = |key: &str| {
        if path.is_empty() {
            key.to_string()
        } else {
            format!("{path}.{key}")
        }
    };
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            for (key, item) in entries {
                json_leaves(item, &child(key), out);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                json_leaves(item, &child(&index.to_string()), out);
            }
        }
        scalar => out.push(format!("{path}={scalar}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil_schema::Time;

    #[derive(Clone, Debug, PartialEq)]
    struct Key(&'static str);

    impl ActionTemplate<Key> for Key {
        fn generate<R: rand::TryRng + rand::RngExt>(&self, _: &mut R) -> Key {
            self.clone()
        }
        fn accepts(&self, original: &Key) -> bool {
            self == original
        }
        fn category_hash<H: Hasher>(&self, hasher: &mut H) {
            hasher.write(self.0.as_bytes());
        }
    }

    struct At(u64);

    impl RunState for At {
        fn timestamp(&self) -> Time {
            Time::from_system_time(std::time::SystemTime::UNIX_EPOCH)
        }
        fn hashes(&self) -> BTreeMap<String, u64> {
            BTreeMap::from([("place".to_string(), self.0)])
        }
    }

    fn tree() -> Tree<Key> {
        Tree::Branch {
            branches: vec![
                (1, Tree::Leaf { value: Key("a") }),
                (1, Tree::Leaf { value: Key("b") }),
                (1, Tree::Leaf { value: Key("c") }),
            ],
        }
    }

    #[test]
    fn action_taken_is_the_accepting_template() {
        let previous = tree();
        let model =
            StepModel::new(&tree(), Some((&previous, &Key("b"))));
        assert_eq!(model.action_taken, Some(action_key(&Key("b"))));
        assert_eq!(model.actions_available.len(), 3);
    }

    #[test]
    fn json_leaves_flatten_paths() {
        let mut leaves = Vec::new();
        json_leaves(
            &serde_json::json!({"b": [1, {"c": null}], "a": "x"}),
            "",
            &mut leaves,
        );
        // Object keys sorted ("a" before "b"), array order kept.
        assert_eq!(leaves, vec!["a=\"x\"", "b.0=1", "b.1.c=null"]);
    }

}
