pub mod html;
pub mod js;
pub mod source_id;
pub mod source_map;

use crate::match_pattern::MatchPattern;

/// Configuration for which types of JavaScript to instrument
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentationConfig {
    pub instrument_files: bool,
    pub instrument_inline: bool,
    /// The URLs under test. A script from elsewhere runs uninstrumented, and a
    /// document from elsewhere is blocked. Empty allows every URL.
    pub allow_urls: Vec<MatchPattern>,
}

impl InstrumentationConfig {
    pub fn all() -> Self {
        Self {
            instrument_files: true,
            instrument_inline: true,
            allow_urls: vec![],
        }
    }

    pub fn none() -> Self {
        Self {
            instrument_files: false,
            instrument_inline: false,
            allow_urls: vec![],
        }
    }

    pub fn is_url_allowed(&self, url: &str) -> bool {
        self.allow_urls.is_empty()
            || url::Url::parse(url)
                .map(|url| self.allow_urls.iter().any(|p| p.matches(&url)))
                .unwrap_or(false)
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
    pub script: String,
    /// Names of the enclosing classes and named functions, joined by `.`,
    /// or `<top>`. Anonymous functions belong to their enclosing name.
    pub function: String,
    /// 1-based line of the block in the script as served.
    pub line: u32,
    /// 0-based column (UTF-16 units) of the block on that line.
    pub column: u32,
    /// The original file the block came from, by the script's source map.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// 1-based line in `source`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_line: Option<u32>,
    /// `source` is on the map's ignore list: code the bundler marks as not
    /// the app's own (node_modules, generated runtime).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub vendor: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instrumented {
    pub text: String,
    pub blocks: Vec<Block>,
}

pub const BLOCK_ID_BITS: u32 = 31;
const BLOCK_ID_MASK: u64 = (1 << BLOCK_ID_BITS) - 1;

/// The ID of the nth (given by `ordinal`) block (in instrumentation order) of
/// `function` in `script`. Structural, so the same code gets the same ID on
/// every load, and 31 bits, so it is exact in JavaScript's int32 bit
/// operations.
pub fn block_id(script: &str, function: &str, ordinal: u32) -> u32 {
    let hash = fnv1a64(
        script
            .bytes()
            .chain([0])
            .chain(function.bytes())
            .chain([0])
            .chain(ordinal.to_le_bytes()),
    );
    ((hash ^ (hash >> BLOCK_ID_BITS) ^ (hash >> (2 * BLOCK_ID_BITS)))
        & BLOCK_ID_MASK) as u32
}

/// FNV-1a of `bytes`. A fixed algorithm instead of DefaultHasher, so IDs and tags
/// computed here are the same across Rust versions. See:
/// https://en.wikipedia.org/wiki/Fowler%E2%80%93Noll%E2%80%93Vo_hash_function
pub fn fnv1a64(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Every block instrumented so far, in registration order. Clones share the
/// same table. It has to outlive any one run, as a browser serves cached
/// scripts without instrumenting them again.
#[derive(Clone, Debug, Default)]
pub struct BlockRegistry(std::sync::Arc<std::sync::Mutex<BlockTable>>);

#[derive(Debug, Default)]
struct BlockTable {
    blocks: Vec<Block>,
    /// Each registered id's position in `blocks`.
    ids: std::collections::BTreeMap<u32, usize>,
}

impl BlockRegistry {
    /// Adds `blocks`, except those whose ID is already registered. The
    /// same script instrumented again yields the same IDs.
    pub fn register(&self, blocks: impl IntoIterator<Item = Block>) {
        let mut table = self.0.lock().expect("block registry poisoned");
        for block in blocks {
            if !table.ids.contains_key(&block.id) {
                let index = table.blocks.len();
                table.ids.insert(block.id, index);
                table.blocks.push(block);
            }
        }
    }

    /// Registered blocks from position `cursor` and on, together with the new
    /// cursor.
    pub fn since(&self, cursor: usize) -> (Vec<Block>, usize) {
        let table = self.0.lock().expect("block registry poisoned");
        let blocks = table.blocks.get(cursor..).unwrap_or_default().to_vec();
        (blocks, table.blocks.len())
    }

    /// Writes every registered block to `path`, one JSON object per line:
    /// the table that trace `branches` ids refer to (`--coverage-symbols`).
    pub fn write(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let (blocks, _) = self.since(0);
        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        for block in blocks {
            serde_json::to_writer(&mut file, &block)?;
            file.write_all(b"\n")?;
        }
        file.flush()
    }

    /// Per-function coverage components of a step: for every function of the
    /// app's own code whose blocks ran (`ids`: the step's `branches`), the entry
    /// `coverage.<script>#<function>` holds the hash of the ids that ran, as sorted
    /// decimal strings (the twin of state-model's `layout_search.py`
    /// `coverage.*` candidates). A function that did not run has no entry, so its
    /// slice of a state key is zeros: the component's default. Vendor blocks
    /// (the source map's ignore list, or a `node_modules` source) are left out,
    /// as in `query.py vendor_blocks`.
    pub fn function_hashes(
        &self,
        ids: &[u32],
    ) -> std::collections::BTreeMap<String, u64> {
        let table = self.0.lock().expect("block registry poisoned");
        let mut groups: std::collections::BTreeMap<String, Vec<u32>> =
            std::collections::BTreeMap::new();
        for id in ids {
            let Some(&index) = table.ids.get(id) else {
                continue;
            };
            let block = &table.blocks[index];
            let vendor = block.vendor
                || block
                    .source
                    .as_deref()
                    .is_some_and(|source| source.contains("/node_modules/"));
            if vendor {
                continue;
            }
            groups
                .entry(format!("coverage.{}#{}", block.script, block.function))
                .or_default()
                .push(*id);
        }
        drop(table);
        groups
            .into_iter()
            .filter_map(|(name, mut ids)| {
                ids.sort_unstable();
                let features: Vec<String> =
                    ids.iter().map(u32::to_string).collect();
                bombadil::model::feature_hash(
                    features.iter().map(String::as_str),
                )
                .map(|hash| (name, hash))
            })
            .collect()
    }
}

/// Whether every state carries the per-function coverage hashes of
/// [`BlockRegistry::function_hashes`] (`--coverage-components`).
static COVERAGE_COMPONENTS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_coverage_components(on: bool) {
    COVERAGE_COMPONENTS.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn coverage_components() -> bool {
    COVERAGE_COMPONENTS.load(std::sync::atomic::Ordering::Relaxed)
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

    /// Blocks with ids from a small range, so that batches overlap.
    #[hegel::composite]
    fn blocks(tc: &TestCase) -> Block {
        Block {
            id: tc.draw_silent(integers::<u32>().max_value(15)),
            script: tc.draw_silent(text().alphabet("/ab").max_size(2)),
            function: tc.draw_silent(text().alphabet("fg.").max_size(2)),
            line: tc.draw_silent(integers::<u32>().min_value(1).max_value(9)),
            column: tc.draw_silent(integers::<u32>().max_value(9)),
            source: None,
            source_line: None,
            vendor: false,
        }
    }

    #[hegel::test]
    fn registry_keeps_the_first_block_of_each_id_in_order(tc: TestCase) {
        let batches = tc.draw(
            vecs(vecs(blocks()).max_size(6))
                .max_size(6)
                .print_as_debug(),
        );

        let registry = BlockRegistry::default();
        let reader = registry.clone();
        let mut cursor = 0;
        let mut read = Vec::new();
        for batch in &batches {
            registry.register(batch.clone());
            let (blocks, cursor_new) = reader.since(cursor);
            assert_eq!(cursor_new, cursor + blocks.len());
            read.extend(blocks);
            cursor = cursor_new;
        }

        let mut ids = BTreeSet::new();
        let expected: Vec<Block> = batches
            .into_iter()
            .flatten()
            .filter(|block| ids.insert(block.id))
            .collect();
        assert_eq!(read, expected);
        assert_eq!(registry.since(0), (expected, cursor));
        assert_eq!(registry.since(cursor), (vec![], cursor));
        assert_eq!(registry.since(cursor + 1), (vec![], cursor));
    }

    #[hegel::test]
    fn registering_blocks_again_changes_nothing(tc: TestCase) {
        let batch = tc.draw(vecs(blocks()).max_size(8).print_as_debug());
        let registry = BlockRegistry::default();
        registry.register(batch.clone());
        let before = registry.since(0);
        registry.register(batch);
        assert_eq!(registry.since(0), before);
    }

    #[hegel::test]
    fn block_ids_fit_in_31_bits(tc: TestCase) {
        let script = tc.draw(text());
        let function = tc.draw(text());
        let ordinal = tc.draw(integers::<u32>());
        assert!(block_id(&script, &function, ordinal) < 1 << BLOCK_ID_BITS);
    }

    /// Pins the ID function. IDs are written into cached scripts and
    /// coverage tables, so changing it invalidates both.
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

    fn block(id: u32, function: &str, vendor: bool) -> Block {
        Block {
            id,
            script: "/app.js".into(),
            function: function.into(),
            line: 1,
            column: 0,
            source: None,
            source_line: None,
            vendor,
        }
    }

    #[test]
    fn function_hashes_group_a_steps_blocks_by_function() {
        let registry = BlockRegistry::default();
        registry.register([
            block(0x7000_0001, "badge", false),
            block(0x7000_0002, "badge", false),
            block(0x7000_0003, "noteStart", false),
            block(0x7000_0004, "render", true),
        ]);
        // Order of ids does not matter; a vendor block and an unknown id are left out.
        let hashes = registry.function_hashes(&[
            0x7000_0002,
            0x7000_0004,
            0x7000_0001,
            0x7000_0009,
        ]);
        let expected =
            bombadil::model::feature_hash(["1879048193", "1879048194"])
                .unwrap();
        assert_eq!(
            hashes,
            std::collections::BTreeMap::from([(
                "coverage./app.js#badge".to_string(),
                expected
            )])
        );
        // A function that did not run has no entry.
        assert!(registry.function_hashes(&[0x7000_0004]).is_empty());
        assert_eq!(registry.function_hashes(&[0x7000_0003]).len(), 1);
    }
}
