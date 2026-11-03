pub mod html;
pub mod js;
pub mod source_id;
pub mod source_map;

use std::io::{self, Write};
use std::iter;
use std::sync::{Arc, mpsc};

use bombadil::hash::{FNV_OFFSET, fnv1a64_from};
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
    /// The script's [`source_id::SourceId`]. With a `source`, the script
    /// the block was first seen in.
    pub script: Arc<str>,
    /// Enclosing class and named function names joined by `.`, or `<top>`.
    /// Anonymous functions count as their enclosing name. With a `source`,
    /// names are the original ones where the map has them.
    pub function: Arc<str>,
    /// 1-based line in the script as served. For inline scripts, relative
    /// to the script's own text.
    pub line: u32,
    /// 0-based column (UTF-16 units) of the block on that line.
    pub column: u32,
    /// The original file the block came from, by the script's source map,
    /// as its [`source_map::source_key`]. The ID of a block with a source is
    /// made from it instead of from `script`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Arc<str>>,
    /// 1-based line in `source`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_line: Option<u32>,
    /// `source` is on the map's ignore list: code the bundler marks as not
    /// the app's own (node_modules, generated runtime).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub vendor: bool,
}

impl Block {
    /// What the ID is made from, with the ordinal.
    fn id_key(&self) -> (&str, &str) {
        (
            self.source.as_deref().unwrap_or(&self.script),
            &self.function,
        )
    }
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
    /// Receives each new block, in registration order.
    new_blocks: Option<mpsc::Sender<Block>>,
}

impl BlockRegistry {
    /// A registry that sends each new block on the returned channel, in
    /// registration order, until [`BlockRegistry::close`].
    pub fn with_channel() -> (Self, mpsc::Receiver<Block>) {
        let (sender, receiver) = mpsc::channel();
        let registry = BlockRegistry::default();
        registry.lock().new_blocks = Some(sender);
        (registry, receiver)
    }

    /// Closes the channel of [`BlockRegistry::with_channel`].
    pub fn close(&self) {
        self.lock().new_blocks = None;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BlockTable> {
        self.0.lock().expect("block registry poisoned")
    }

    /// Adds the `blocks` whose IDs are not yet registered.
    pub fn register(&self, blocks: impl IntoIterator<Item = Block>) {
        let table = &mut *self.lock();
        let mut source_differs = false;
        for block in blocks {
            let Some(&index) = table.ids.get(&block.id) else {
                let index = table.blocks.len();
                table.ids.insert(block.id, index);
                if let Some(new_blocks) = &table.new_blocks {
                    // The receiver is gone if writing failed, which the
                    // writer reports.
                    let _ = new_blocks.send(block.clone());
                }
                table.blocks.push(block);
                continue;
            };
            let registered = &table.blocks[index];
            if registered.id_key() != block.id_key() {
                log::warn!(
                    "block ID {} of {} in {} is taken by {} in {}, which gets its coverage",
                    block.id,
                    block.function,
                    block.id_key().0,
                    registered.function,
                    registered.id_key().0,
                );
            } else if !source_differs
                && let Some(source) = &block.source
                && registered.source_line != block.source_line
                && registered.script != block.script
            {
                // Once per script: every block after the first that
                // differs would say the same. A script that loads again
                // after an edit has its own lines shifted, which is fine.
                source_differs = true;
                log::warn!(
                    "{source} is not the same in {} as in {} (two files with one source key, or one file bundled differently): their blocks share IDs and coverage",
                    block.script,
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
        self.lock().source_ids.claim(masked, exact)
    }

    pub fn contains(&self, id: u32) -> bool {
        self.lock().ids.contains_key(&id)
    }

    #[cfg(test)]
    fn blocks(&self) -> Vec<Block> {
        self.lock().blocks.clone()
    }
}

/// Writes the blocks of `new_blocks` as JSON lines until the channel closes,
/// flushing whenever it runs empty.
pub fn write_blocks(
    new_blocks: mpsc::Receiver<Block>,
    writer: impl io::Write,
) -> io::Result<()> {
    let mut writer = io::BufWriter::new(writer);
    while let Ok(block) = new_blocks.recv() {
        for block in iter::once(block).chain(new_blocks.try_iter()) {
            serde_json::to_writer(&mut writer, &block)?;
            writer.write_all(b"\n")?;
        }
        writer.flush()?;
    }
    Ok(())
}

#[cfg(test)]
impl Instrumented {
    pub(crate) fn snapshot(&self) -> String {
        let mut snapshot = format!("{}\n---- blocks ----\n", self.text);
        for block in &self.blocks {
            snapshot.push_str(&format!(
                "{:>10}  {}:{}:{}  {}",
                block.id,
                block.script,
                block.line,
                block.column,
                block.function
            ));
            if let (Some(source), Some(line)) =
                (&block.source, block.source_line)
            {
                let vendor = if block.vendor { " (vendor)" } else { "" };
                snapshot.push_str(&format!("  {source}:{line}{vendor}"));
            }
            snapshot.push('\n');
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::{
        Generator, TestCase,
        generators::{booleans, integers, optional, text, vecs},
    };
    use std::collections::BTreeSet;

    /// Blocks with IDs from a small range, so batches overlap.
    #[hegel::composite]
    fn blocks(tc: &TestCase) -> Block {
        Block {
            id: tc.draw_silent(integers::<u32>().max_value(15)),
            script: tc.draw_silent(text()).into(),
            function: tc.draw_silent(text()).into(),
            line: tc.draw_silent(integers::<u32>().min_value(1).max_value(9)),
            column: tc.draw_silent(integers::<u32>().max_value(9)),
            source: tc.draw_silent(optional(text())).map(Arc::from),
            source_line: tc.draw_silent(optional(integers::<u32>())),
            vendor: tc.draw_silent(booleans()),
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
    fn new_blocks_are_written_as_json_lines(tc: TestCase) {
        let batches = tc.draw(
            vecs(vecs(blocks()).max_size(6))
                .max_size(6)
                .print_as_debug(),
        );
        let (registry, new_blocks) = BlockRegistry::with_channel();
        for batch in batches {
            registry.register(batch);
        }
        registry.close();
        let mut written = Vec::new();
        write_blocks(new_blocks, &mut written).unwrap();

        let lines: Vec<serde_json::Value> = written
            .split_inclusive(|&byte| byte == b'\n')
            .map(|line| {
                assert_eq!(line.last(), Some(&b'\n'));
                serde_json::from_slice(line).unwrap()
            })
            .collect();
        let expected: Vec<serde_json::Value> = registry
            .blocks()
            .iter()
            .map(|block| {
                let mut expected = serde_json::json!({
                    "id": block.id,
                    "script": &*block.script,
                    "function": &*block.function,
                    "line": block.line,
                    "column": block.column,
                });
                if let Some(source) = &block.source {
                    expected["source"] = (&**source).into();
                }
                if let Some(source_line) = block.source_line {
                    expected["source_line"] = source_line.into();
                }
                if block.vendor {
                    expected["vendor"] = true.into();
                }
                expected
            })
            .collect();
        assert_eq!(lines, expected);
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
