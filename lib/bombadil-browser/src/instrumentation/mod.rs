pub mod html;
pub mod js;
pub mod source_id;

use std::sync::Arc;

use source_id::{SourceId, SourceIds};

/// Configuration for which types of JavaScript to instrument
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentationConfig {
    pub instrument_files: bool,
    pub instrument_inline: bool,
}

impl InstrumentationConfig {
    pub fn all() -> Self {
        Self {
            instrument_files: true,
            instrument_inline: true,
        }
    }

    pub fn none() -> Self {
        Self {
            instrument_files: false,
            instrument_inline: false,
        }
    }
}

impl Default for InstrumentationConfig {
    fn default() -> Self {
        Self::all()
    }
}

/// A coverage point (a branch or loop body) in an instrumented script.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Block {
    pub id: u32,
    /// The script's [`source_id::SourceId`].
    pub script: Arc<str>,
    /// Enclosing class and named function names joined by `.`, or `<top>`.
    /// Anonymous functions count as their enclosing name.
    pub function: Arc<str>,
    /// 1-based line in the script. For inline scripts, relative to the
    /// script's own text.
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instrumented {
    pub text: String,
    pub blocks: Vec<Block>,
}

pub const BLOCK_ID_BITS: u32 = 31;
const BLOCK_ID_MASK: u64 = (1 << BLOCK_ID_BITS) - 1;

/// The ID of the `ordinal`th block of `function` in `script`. Structural,
/// so stable across loads; 31 bits, so exact in JavaScript's int32
/// operations.
pub fn block_id(script: &str, function: &str, ordinal: u32) -> u32 {
    BlockIdPrefix::new(script, function).id(ordinal)
}

/// The script and function part of a block ID, hashed once.
#[derive(Clone, Copy, Debug)]
pub struct BlockIdPrefix(u64);

impl BlockIdPrefix {
    pub fn new(script: &str, function: &str) -> Self {
        let bytes =
            script.bytes().chain([0]).chain(function.bytes()).chain([0]);
        BlockIdPrefix(fnv1a64_from(FNV_OFFSET, bytes))
    }

    pub fn id(self, ordinal: u32) -> u32 {
        let hash = fnv1a64_from(self.0, ordinal.to_le_bytes());
        ((hash ^ (hash >> BLOCK_ID_BITS) ^ (hash >> (2 * BLOCK_ID_BITS)))
            & BLOCK_ID_MASK) as u32
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a rather than `DefaultHasher`, so IDs are stable across Rust
/// versions.
pub fn fnv1a64(bytes: impl IntoIterator<Item = u8>) -> u64 {
    fnv1a64_from(FNV_OFFSET, bytes)
}

fn fnv1a64_from(mut hash: u64, bytes: impl IntoIterator<Item = u8>) -> u64 {
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Every block instrumented so far, in registration order, shared between
/// clones. Outlives a run, since browsers serve cached scripts without
/// instrumenting them again.
#[derive(Clone, Debug, Default)]
pub struct BlockRegistry(std::sync::Arc<std::sync::Mutex<BlockTable>>);

#[derive(Debug, Default)]
struct BlockTable {
    blocks: Vec<Block>,
    /// Position in `blocks` by ID.
    ids: std::collections::BTreeMap<u32, usize>,
    source_ids: SourceIds,
}

impl BlockRegistry {
    /// Adds the `blocks` whose IDs are not yet registered.
    pub fn register(&self, blocks: impl IntoIterator<Item = Block>) {
        let mut table = self.0.lock().expect("block registry poisoned");
        for block in blocks {
            let Some(&index) = table.ids.get(&block.id) else {
                let index = table.blocks.len();
                table.ids.insert(block.id, index);
                table.blocks.push(block);
                continue;
            };
            let registered = &table.blocks[index];
            if (&registered.script, &registered.function)
                != (&block.script, &block.function)
            {
                log::warn!(
                    "block ID {} of {} in {} is taken by {} in {}, which gets its coverage",
                    block.id,
                    block.function,
                    block.script,
                    registered.function,
                    registered.script,
                );
            }
        }
    }

    /// [`SourceId::from_url`], or the unmasked path and query if another URL
    /// already has that ID.
    pub fn source_id(&self, url: &url::Url, origin: &url::Url) -> SourceId {
        let masked = SourceId::from_url(url, origin);
        let exact = SourceId::exact(url, origin);
        if masked == exact {
            return masked;
        }
        let mut table = self.0.lock().expect("block registry poisoned");
        table.source_ids.claim(masked, exact)
    }

    #[cfg(test)]
    fn blocks(&self) -> Vec<Block> {
        let table = self.0.lock().expect("block registry poisoned");
        table.blocks.clone()
    }
}

#[cfg(test)]
impl Instrumented {
    pub(crate) fn snapshot(&self) -> String {
        let mut snapshot = format!("{}\n---- blocks ----\n", self.text);
        for block in &self.blocks {
            snapshot.push_str(&format!(
                "{:>10}  {}:{}  {}\n",
                block.id, block.script, block.line, block.function
            ));
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::{
        Generator, TestCase,
        generators::{integers, text, vecs},
    };
    use std::collections::BTreeSet;

    /// Blocks with IDs from a small range, so batches overlap.
    #[hegel::composite]
    fn blocks(tc: &TestCase) -> Block {
        Block {
            id: tc.draw_silent(integers::<u32>().max_value(15)),
            script: tc.draw_silent(text().alphabet("/ab").max_size(2)).into(),
            function: tc.draw_silent(text().alphabet("fg.").max_size(2)).into(),
            line: tc.draw_silent(integers::<u32>().min_value(1).max_value(9)),
        }
    }

    #[hegel::test]
    fn registry_keeps_the_first_block_of_each_id_in_order(tc: TestCase) {
        let blocks = tc.draw(vecs(blocks()).max_size(24).print_as_debug());
        let registry = BlockRegistry::default();
        registry.register(blocks.clone());
        let registered = registry.blocks();

        let ids: Vec<u32> = registered.iter().map(|block| block.id).collect();
        let unique: BTreeSet<u32> = ids.iter().copied().collect();
        assert_eq!(ids.len(), unique.len());
        assert_eq!(unique, blocks.iter().map(|block| block.id).collect());

        // Each block is the first of its ID, and they come in that order.
        let positions: Vec<usize> = registered
            .iter()
            .map(|block| {
                let first =
                    blocks.iter().position(|b| b.id == block.id).unwrap();
                assert_eq!(block, &blocks[first]);
                first
            })
            .collect();
        assert!(positions.is_sorted());
    }

    #[hegel::test]
    fn block_ids_fit_in_31_bits(tc: TestCase) {
        let script = tc.draw(text());
        let function = tc.draw(text());
        let ordinal = tc.draw(integers::<u32>());
        assert!(block_id(&script, &function, ordinal) < 1 << BLOCK_ID_BITS);
    }

    /// Pins the ID function: IDs live in cached scripts and coverage tables.
    #[test]
    fn block_ids_are_pinned() {
        assert_eq!(fnv1a64([]), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(*b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(*b"foobar"), 0x8594_4171_f739_67e8);
        insta::assert_snapshot!(
            [
                block_id("/app.js", "<top>", 0),
                block_id("/app.js", "<top>", 1),
                block_id("/app.js", "Store.add", 0),
                block_id("/index.html#inline0", "<top>", 0),
            ]
            .map(|id| id.to_string())
            .join("\n")
        );
    }
}
