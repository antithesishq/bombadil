//! Source maps for instrumented scripts: a block records the original file and
//! line it came from, so that coverage can be read per source file and app code
//! told from bundled vendor code, and its ID is made from that file and the
//! original function names, so that it survives a change of bundle. A script
//! without a map records blocks as before.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use serde::Deserialize;
use serde::de::{DeserializeSeed, SeqAccess, Visitor};
use url::Url;

#[derive(Deserialize)]
struct RawSourceMap<'a> {
    #[serde(default)]
    sources: Vec<Option<String>>,
    #[serde(rename = "sourceRoot", default)]
    source_root: Option<String>,
    #[serde(default)]
    names: Names,
    #[serde(borrow, default)]
    mappings: Cow<'a, str>,
    /// Sources that are not the app's own code (node_modules, generated
    /// runtime).
    #[serde(rename = "ignoreList", default)]
    ignore_list: Vec<usize>,
    /// What bundlers wrote before `ignoreList` was standardised.
    #[serde(rename = "x_google_ignoreList", default)]
    ignore_list_google: Vec<usize>,
    #[serde(default)]
    sections: Option<serde::de::IgnoredAny>,
}

/// The map's `names`, end to end.
#[derive(Default)]
struct Names {
    text: String,
    /// Where each name ends in `text`.
    ends: Vec<u32>,
}

impl Names {
    fn get(&self, index: u32) -> Option<&str> {
        let index = index as usize;
        let end = *self.ends.get(index)? as usize;
        let start = match index.checked_sub(1) {
            Some(previous) => self.ends[previous] as usize,
            None => 0,
        };
        Some(&self.text[start..end])
    }
}

impl<'de> Deserialize<'de> for Names {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        /// Appends a name to the text. A `null` is an empty name.
        struct Name<'n>(&'n mut String);

        impl<'de> DeserializeSeed<'de> for Name<'_> {
            type Value = ();

            fn deserialize<D: serde::Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> Result<(), D::Error> {
                deserializer.deserialize_any(self)
            }
        }

        impl<'de> Visitor<'de> for Name<'_> {
            type Value = ();

            fn expecting(
                &self,
                f: &mut std::fmt::Formatter,
            ) -> std::fmt::Result {
                f.write_str("a name or null")
            }

            fn visit_str<E>(self, name: &str) -> Result<(), E> {
                self.0.push_str(name);
                Ok(())
            }

            fn visit_unit<E>(self) -> Result<(), E> {
                Ok(())
            }
        }

        struct NamesVisitor;

        impl<'de> Visitor<'de> for NamesVisitor {
            type Value = Names;

            fn expecting(
                &self,
                f: &mut std::fmt::Formatter,
            ) -> std::fmt::Result {
                f.write_str("a list of names")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Names, A::Error> {
                let mut names = Names::default();
                while seq.next_element_seed(Name(&mut names.text))?.is_some() {
                    let end = u32::try_from(names.text.len())
                        .map_err(serde::de::Error::custom)?;
                    names.ends.push(end);
                }
                Ok(names)
            }
        }

        deserializer.deserialize_seq(NamesVisitor)
    }
}

/// The `source` of a segment of generated code that has no source.
const SOURCE_NONE: u32 = u32::MAX;
/// The `name` of a segment that has none.
const NAME_NONE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug)]
struct Segment {
    column: u32,
    source: u32,
    /// 0-based line in the source.
    line: u32,
    /// Index into the map's names.
    name: u32,
}

pub struct SourceMap {
    /// The [`source_key`] of each source, or `None` for one the map lists
    /// as `null`.
    sources: Vec<Option<Arc<str>>>,
    /// The first source with the same key as each source.
    sources_first: Vec<u32>,
    vendors: Vec<bool>,
    names: Names,
    /// The segments of all generated lines, each line's sorted by column.
    segments: Vec<Segment>,
    /// Where each 0-based generated line's segments end in `segments`.
    line_ends: Vec<u32>,
}

/// Where a generated position came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Original<'a> {
    /// The [`source_key`] of the source.
    pub source: &'a Arc<str>,
    /// The same for every source of the map that has this key.
    pub source_index: u32,
    /// 1-based line in the source.
    pub line: u32,
    /// The source is on the map's ignore list.
    pub vendor: bool,
}

const VLQ_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const VLQ_INVALID: u8 = u8::MAX;
const VLQ_DIGITS: [u8; 256] = {
    let mut digits = [VLQ_INVALID; 256];
    let mut digit = 0;
    while digit < VLQ_ALPHABET.len() {
        digits[VLQ_ALPHABET[digit] as usize] = digit as u8;
        digit += 1;
    }
    digits
};
/// Positions are 32-bit, so a value with more digits than this is not one.
const VLQ_SHIFT_MAX: u32 = 55;

fn advance(position: &mut i64, delta: i64) -> Result<u32> {
    *position = position
        .checked_add(delta)
        .context("mapping position out of range")?;
    u32::try_from(*position).context("mapping position out of range")
}

fn parse_mappings(mappings: &str) -> Result<(Vec<Segment>, Vec<u32>)> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut line_ends = Vec::new();
    let mut line_start = 0;
    let mut line_sorted = true;
    let mut fields = [0i64; 5];
    let mut fields_count = 0;
    let (mut value, mut shift) = (0u64, 0u32);
    let (mut column, mut source, mut line, mut name) = (0i64, 0i64, 0i64, 0i64);
    for byte in mappings.bytes().chain(std::iter::once(b';')) {
        if byte != b',' && byte != b';' {
            let digit = VLQ_DIGITS[byte as usize];
            if digit == VLQ_INVALID {
                bail!("invalid VLQ character {:?}", byte as char);
            }
            if shift > VLQ_SHIFT_MAX || fields_count == fields.len() {
                bail!("mapping segment is too long");
            }
            value |= u64::from(digit & 31) << shift;
            if digit & 32 != 0 {
                shift += 5;
                continue;
            }
            let magnitude = (value >> 1) as i64;
            fields[fields_count] = if value & 1 == 1 {
                -magnitude
            } else {
                magnitude
            };
            fields_count += 1;
            (value, shift) = (0, 0);
            continue;
        }
        if shift != 0 {
            bail!("mapping segment ends inside a VLQ value");
        }
        match fields_count {
            0 => {}
            // The source column (field 3) is not kept.
            1 | 4 | 5 => {
                let mut segment = Segment {
                    column: advance(&mut column, fields[0])?,
                    source: SOURCE_NONE,
                    line: 0,
                    name: NAME_NONE,
                };
                if fields_count > 1 {
                    segment.source = advance(&mut source, fields[1])?;
                    segment.line = advance(&mut line, fields[2])?;
                }
                if fields_count == 5 {
                    segment.name = advance(&mut name, fields[4])?;
                }
                if segments[line_start..]
                    .last()
                    .is_some_and(|last| last.column > segment.column)
                {
                    line_sorted = false;
                }
                segments.push(segment);
            }
            count => bail!("mapping segment with {count} fields"),
        }
        fields_count = 0;
        if byte == b';' {
            if !line_sorted {
                segments[line_start..].sort_by_key(|segment| segment.column);
                line_sorted = true;
            }
            line_start = segments.len();
            line_ends
                .push(u32::try_from(line_start).context("too many mappings")?);
            column = 0;
        }
    }
    Ok((segments, line_ends))
}

/// What follows the scheme of `source` and its `:`, if it starts with one.
fn after_scheme(source: &str) -> Option<&str> {
    let (scheme, rest) = source.split_once(':')?;
    let mut bytes = scheme.bytes();
    let is_scheme = bytes.next()?.is_ascii_alphabetic()
        && bytes
            .all(|byte| byte.is_ascii_alphanumeric() || b"+-.".contains(&byte));
    is_scheme.then_some(rest)
}

/// Whether `source` is a URL or an absolute path, and not a path relative
/// to where the map is.
fn stands_alone(source: &str) -> bool {
    source.starts_with('/') || after_scheme(source).is_some()
}

/// The directory that the map's relative sources are in, ending in `/`
/// unless it is empty: the map's `sourceRoot`, which is itself under the
/// directory of `base` unless it stands alone. A root that is a URL is left
/// without its scheme and authority, which a key does not have either.
fn sources_directory(base: Option<&Url>, root: &str) -> String {
    let mut directory = String::new();
    if let Some(base) = base
        && !base.cannot_be_a_base()
        && !stands_alone(root)
    {
        let path = base.path();
        let end = path.rfind('/').map_or(0, |slash| slash + 1);
        directory.push_str(&path[..end]);
    }
    let root = strip_scheme_and_authority(root).unwrap_or(root);
    push_resolved(&mut directory, root);
    if !directory.is_empty() && !directory.ends_with('/') {
        directory.push('/');
    }
    directory
}

/// Appends the relative `path` to the directory `resolved`, without its `.`
/// and `..` segments.
fn push_resolved(resolved: &mut String, path: &str) {
    let mut segments = path.split('/').peekable();
    while let Some(segment) = segments.next() {
        match segment {
            "." => {}
            ".." => {
                let parent = resolved
                    .trim_end_matches('/')
                    .rfind('/')
                    .map_or(0, |slash| slash + 1);
                resolved.truncate(parent);
            }
            _ => {
                resolved.push_str(segment);
                if segments.peek().is_some() {
                    resolved.push('/');
                }
            }
        }
    }
}

/// The [`source_key`] of `source` as the map means it: under `directory`
/// (see [`sources_directory`]), unless it stands alone. `resolved` is
/// scratch space.
fn source_key_under<'a>(
    directory: &str,
    source: &'a str,
    resolved: &'a mut String,
) -> &'a str {
    if directory.is_empty() || stands_alone(source) {
        return source_key(source);
    }
    resolved.clear();
    resolved.push_str(directory);
    push_resolved(resolved, source);
    source_key(resolved)
}

/// The part of a source's path that does not depend on the bundler or on
/// where it wrote the bundle: without the scheme and authority
/// (`webpack://app/`, `file://`) and without leading `/`, `./` and `../`.
/// It never starts with `/`, as the path of a script's URL does.
pub fn source_key(source: &str) -> &str {
    let mut key = source;
    loop {
        let rest = key.trim_start_matches('/');
        let rest = rest
            .strip_prefix("./")
            .or_else(|| rest.strip_prefix("../"))
            .or_else(|| strip_scheme_and_authority(rest))
            .unwrap_or(rest);
        if rest.len() == key.len() {
            return key;
        }
        key = rest;
    }
}

fn strip_scheme_and_authority(source: &str) -> Option<&str> {
    let rest = after_scheme(source)?.strip_prefix("//")?;
    Some(rest.find('/').map_or("", |slash| &rest[slash..]))
}

impl SourceMap {
    /// `base` is the URL of the map, or of the script for a map inside it:
    /// what the map's relative sources are relative to.
    pub fn parse(json: &str, base: Option<&Url>) -> Result<Self> {
        let raw: RawSourceMap =
            serde_json::from_str(json).context("source map is not JSON")?;
        if raw.sections.is_some() {
            bail!("index source maps (`sections`) are not supported");
        }
        let (segments, line_ends) = parse_mappings(&raw.mappings)?;
        let directory = sources_directory(
            base,
            raw.source_root.as_deref().unwrap_or_default(),
        );
        let mut resolved = String::new();
        let mut vendors = vec![false; raw.sources.len()];
        for index in raw.ignore_list.into_iter().chain(raw.ignore_list_google) {
            if let Some(vendor) = vendors.get_mut(index) {
                *vendor = true;
            }
        }
        let mut firsts: HashMap<Arc<str>, u32> = HashMap::new();
        let mut sources_first = Vec::with_capacity(raw.sources.len());
        let mut sources = Vec::with_capacity(raw.sources.len());
        for (index, source) in raw.sources.into_iter().enumerate() {
            let index = u32::try_from(index).context("too many sources")?;
            let key = source.map(|source| {
                Arc::<str>::from(source_key_under(
                    &directory,
                    &source,
                    &mut resolved,
                ))
            });
            sources_first.push(match &key {
                Some(key) => *firsts.entry(key.clone()).or_insert(index),
                None => index,
            });
            sources.push(key);
        }
        Ok(Self {
            sources,
            sources_first,
            vendors,
            names: raw.names,
            segments,
            line_ends,
        })
    }

    /// The last segment at or before `column` (0-based, UTF-16 units) of
    /// the generated `line` (1-based).
    fn segment(&self, line: u32, column: u32) -> Option<&Segment> {
        let line = line.checked_sub(1)? as usize;
        let end = *self.line_ends.get(line)? as usize;
        let start = match line.checked_sub(1) {
            Some(previous) => self.line_ends[previous] as usize,
            None => 0,
        };
        let segments = &self.segments[start..end];
        let after = segments.partition_point(|s| s.column <= column);
        segments.get(after.checked_sub(1)?)
    }

    /// The original position of a generated `line` (1-based) and `column`
    /// (0-based, UTF-16 units): that of the last segment at or before the
    /// column, if that segment has a source.
    pub fn lookup(&self, line: u32, column: u32) -> Option<Original<'_>> {
        let segment = self.segment(line, column)?;
        let source = segment.source as usize;
        Some(Original {
            source: self.sources.get(source)?.as_ref()?,
            source_index: self.sources_first[source],
            line: segment.line.checked_add(1)?,
            vendor: self.vendors[source],
        })
    }

    /// The original name of the identifier that starts at a generated
    /// `line` and `column`, if the map has a segment with a name there.
    pub fn name(&self, line: u32, column: u32) -> Option<&str> {
        let segment = self.segment(line, column)?;
        if segment.column != column {
            return None;
        }
        self.names.get(segment.name).filter(|name| !name.is_empty())
    }

    pub fn sources_count(&self) -> usize {
        self.sources.len()
    }

    pub fn has_names(&self) -> bool {
        !self.names.ends.is_empty()
    }

    /// About how much memory the map holds.
    pub fn size_bytes(&self) -> usize {
        let sources: usize = self
            .sources
            .iter()
            .flatten()
            .map(|source| source.len())
            .sum();
        sources
            + self.sources.capacity() * size_of::<Option<Arc<str>>>()
            + self.sources_first.capacity() * size_of::<u32>()
            + self.vendors.capacity()
            + self.names.text.capacity()
            + self.names.ends.capacity() * size_of::<u32>()
            + self.segments.capacity() * size_of::<Segment>()
            + self.line_ends.capacity() * size_of::<u32>()
    }
}

/// The `//# sourceMappingURL=` among the comment lines that end a script,
/// if there is one (the last such comment wins, as in browsers).
pub fn source_mapping_url(source_text: &str) -> Option<&str> {
    for line in source_text.rsplit('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let comment = line.strip_prefix("//")?;
        if let Some(url) = comment
            .strip_prefix("# sourceMappingURL=")
            .or_else(|| comment.strip_prefix("@ sourceMappingURL="))
        {
            let url = url.trim_start();
            return (!url.is_empty()).then_some(url);
        }
    }
    None
}

/// Where a script's source map is.
#[derive(Debug, PartialEq, Eq)]
pub enum Location {
    /// In the script itself, as a `data:` URL: the map's JSON.
    Inline(String),
    Remote(Url),
}

/// Where the source map that the script at `script_url` refers to as
/// `map_url` is.
pub fn locate(script_url: &Url, map_url: &str) -> Result<Location> {
    let Some(data) = map_url.strip_prefix("data:") else {
        return Ok(Location::Remote(script_url.join(map_url)?));
    };
    let (header, payload) = data
        .split_once(',')
        .ok_or_else(|| anyhow!("malformed data URL"))?;
    let is_base64 = header
        .len()
        .checked_sub(";base64".len())
        .and_then(|at| header.as_bytes().get(at..))
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(b";base64"));
    let json = if is_base64 {
        String::from_utf8(BASE64_STANDARD.decode(payload)?)?
    } else {
        percent_encoding::percent_decode_str(payload)
            .decode_utf8()?
            .into_owned()
    };
    Ok(Location::Inline(json))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use hegel::{
        Generator, TestCase,
        generators::{
            booleans, integers, optional, sampled_from, text, tuples2, vecs,
        },
    };

    fn vlq_encode(value: i64, out: &mut String) {
        let mut rest = (value.unsigned_abs() << 1) | u64::from(value < 0);
        loop {
            let digit = (rest & 31) as usize;
            rest >>= 5;
            let continues = if rest > 0 { 32 } else { 0 };
            out.push(VLQ_ALPHABET[digit | continues] as char);
            if rest == 0 {
                break;
            }
        }
    }

    #[derive(Clone, Debug)]
    pub(crate) struct SegmentModel {
        pub(crate) column: u32,
        /// Index into the sources and 0-based line, or generated code
        /// without a source.
        pub(crate) original: Option<(u32, u32)>,
        pub(crate) column_original: u32,
        /// Index into the names.
        pub(crate) name: Option<u32>,
    }

    #[derive(Debug, Default)]
    pub(crate) struct MapModel {
        pub(crate) sources: Vec<Option<String>>,
        pub(crate) vendors: Vec<bool>,
        /// Whether a vendor source is listed under the standard name.
        pub(crate) vendors_standard: Vec<bool>,
        pub(crate) names: Vec<Option<String>>,
        /// In the order the map lists them, which need not be by column.
        pub(crate) lines: Vec<Vec<SegmentModel>>,
    }

    #[hegel::composite]
    fn map_models(tc: &TestCase) -> MapModel {
        let sources =
            tc.draw_silent(vecs(optional(text())).min_size(1).max_size(4));
        let source_last = sources.len() as u32 - 1;
        let names = tc.draw_silent(vecs(optional(text())).max_size(4));
        let name_last = (names.len() as u32).saturating_sub(1);
        let segments = hegel::compose!(|tc| {
            // A name of the map, or any index: most are past the last name.
            let name_max = match tc.draw_silent(booleans()) {
                true => name_last,
                false => u32::MAX,
            };
            SegmentModel {
                column: tc.draw_silent(integers::<u32>()),
                original: tc.draw_silent(optional(tuples2(
                    integers::<u32>().max_value(source_last),
                    integers::<u32>(),
                ))),
                column_original: tc.draw_silent(integers::<u32>()),
                name: tc.draw_silent(optional(
                    integers::<u32>().max_value(name_max),
                )),
            }
        });
        MapModel {
            vendors: tc.draw_silent(
                vecs(booleans())
                    .min_size(sources.len())
                    .max_size(sources.len()),
            ),
            vendors_standard: tc.draw_silent(
                vecs(booleans())
                    .min_size(sources.len())
                    .max_size(sources.len()),
            ),
            lines: tc.draw_silent(vecs(vecs(segments).max_size(6)).max_size(6)),
            sources,
            names,
        }
    }

    impl MapModel {
        pub(crate) fn json(&self) -> String {
            let mut mappings = String::new();
            let (mut source, mut line, mut column_original, mut name) =
                (0i64, 0i64, 0i64, 0i64);
            for (index, segments) in self.lines.iter().enumerate() {
                if index > 0 {
                    mappings.push(';');
                }
                let mut column = 0i64;
                for (index, segment) in segments.iter().enumerate() {
                    if index > 0 {
                        mappings.push(',');
                    }
                    let mut field = |previous: &mut i64, value: u32| {
                        vlq_encode(i64::from(value) - *previous, &mut mappings);
                        *previous = i64::from(value);
                    };
                    field(&mut column, segment.column);
                    let Some((source_index, line_original)) = segment.original
                    else {
                        continue;
                    };
                    field(&mut source, source_index);
                    field(&mut line, line_original);
                    field(&mut column_original, segment.column_original);
                    if let Some(name_index) = segment.name {
                        field(&mut name, name_index);
                    }
                }
            }
            let ignore_list = |standard: bool| -> Vec<usize> {
                (0..self.sources.len())
                    .filter(|&index| {
                        self.vendors[index]
                            && self.vendors_standard[index] == standard
                    })
                    .collect()
            };
            serde_json::json!({
                "version": 3,
                "sources": self.sources,
                "names": self.names,
                "mappings": mappings,
                "ignoreList": ignore_list(true),
                "x_google_ignoreList": ignore_list(false),
            })
            .to_string()
        }

        /// The last segment at or before the column.
        fn segment(&self, line: u32, column: u32) -> Option<&SegmentModel> {
            let segments = self.lines.get(line.checked_sub(1)? as usize)?;
            let mut sorted: Vec<&SegmentModel> = segments.iter().collect();
            sorted.sort_by_key(|segment| segment.column);
            sorted.into_iter().rfind(|segment| segment.column <= column)
        }

        /// The source's key, the first source with that key, the line and
        /// whether the source is a vendor's.
        fn lookup(
            &self,
            line: u32,
            column: u32,
        ) -> Option<(&str, u32, u32, bool)> {
            let (source, line) = self.segment(line, column)?.original?;
            let key = source_key(self.sources[source as usize].as_deref()?);
            let first = self.sources.iter().position(|source| {
                source.as_deref().map(source_key) == Some(key)
            })?;
            Some((
                key,
                first as u32,
                line.checked_add(1)?,
                self.vendors[source as usize],
            ))
        }

        fn name(&self, line: u32, column: u32) -> Option<&str> {
            let segment = self.segment(line, column)?;
            segment.original?;
            let name = self.names.get(segment.name? as usize)?.as_deref()?;
            (segment.column == column && !name.is_empty()).then_some(name)
        }

        /// A position at or next to a segment, or any column of a line from
        /// one before the first to two past the last.
        fn position(&self, tc: &TestCase) -> (u32, u32) {
            let near: Vec<(u32, u32)> = self
                .lines
                .iter()
                .enumerate()
                .flat_map(|(line, segments)| {
                    segments.iter().flat_map(move |segment| {
                        [
                            segment.column.saturating_sub(1),
                            segment.column,
                            segment.column.saturating_add(1),
                        ]
                        .map(|column| (line as u32 + 1, column))
                    })
                })
                .collect();
            if near.is_empty() || tc.draw(booleans()) {
                let line = tc.draw(
                    integers::<u32>().max_value(self.lines.len() as u32 + 2),
                );
                (line, tc.draw(integers::<u32>()))
            } else {
                tc.draw(sampled_from(near))
            }
        }
    }

    #[hegel::test]
    fn lookup_finds_the_segment_a_position_falls_in(tc: TestCase) {
        let model = tc.draw(map_models().print_as_debug());
        let map = SourceMap::parse(&model.json(), None).unwrap();
        let (line, column) = model.position(&tc);
        let found = map.lookup(line, column).map(|original| {
            (
                &**original.source,
                original.source_index,
                original.line,
                original.vendor,
            )
        });
        assert_eq!(found, model.lookup(line, column));
    }

    #[hegel::test(test_cases = 2000)]
    fn name_is_that_of_the_segment_that_starts_at_a_position(tc: TestCase) {
        let model = tc.draw(map_models().print_as_debug());
        let map = SourceMap::parse(&model.json(), None).unwrap();
        let (line, column) = model.position(&tc);
        assert_eq!(map.name(line, column), model.name(line, column));
    }

    const SOURCE_PREFIXES: [&str; 8] = [
        "/",
        "./",
        "../",
        "webpack://app/",
        "webpack:///",
        "webpack://_N_E/",
        "file:///",
        "ng+x.y-z://host/",
    ];

    #[hegel::test]
    fn source_keys_are_the_path_below_what_bundlers_put_before_it(
        tc: TestCase,
    ) {
        // A path whose first segment ends in a character that keeps it from
        // being `.`, `..`, empty or a scheme.
        let segments =
            tc.draw(vecs(text().exclude_characters("/")).min_size(1));
        let end =
            tc.draw(text().exclude_characters("/.:").min_size(1).max_size(1));
        let (first, rest) = segments.split_first().unwrap();
        let mut path = format!("{first}{end}");
        for segment in rest {
            path.push('/');
            path.push_str(segment);
        }
        let prefixes =
            tc.draw(vecs(sampled_from(SOURCE_PREFIXES.as_slice())).max_size(4));
        let source = format!("{}{path}", prefixes.concat());
        assert_eq!(source_key(&source), path);
    }

    #[hegel::test]
    fn source_keys_are_what_is_left_when_nothing_leads(tc: TestCase) {
        let source = if tc.draw(booleans()) {
            tc.draw(text())
        } else {
            tc.draw(text().alphabet("ab:/."))
        };
        let key = source_key(&source);
        assert!(source.ends_with(key));
        for prefix in ["/", "./", "../"] {
            assert!(!key.starts_with(prefix), "{key:?}");
        }
        assert_eq!(source_key(key), key);
    }

    #[hegel::test]
    fn mappings_that_are_not_mappings_are_errors_not_panics(tc: TestCase) {
        let mappings = if tc.draw(booleans()) {
            tc.draw(text())
        } else {
            // Close to valid, to get past the first character.
            tc.draw(text().alphabet("ACDgkpz+/,;"))
        };
        let json = serde_json::json!({
            "sources": ["a.ts"],
            "mappings": mappings,
        })
        .to_string();
        if let Ok(map) = SourceMap::parse(&json, None) {
            let line = tc.draw(integers::<u32>());
            let column = tc.draw(integers::<u32>());
            if let Some(original) = map.lookup(line, column) {
                assert_eq!(&**original.source, "a.ts");
            }
        }
    }

    #[test]
    fn index_maps_and_other_json_are_rejected() {
        assert!(SourceMap::parse(r#"{"sections":[]}"#, None).is_err());
        assert!(SourceMap::parse("", None).is_err());
    }

    /// A path whose segments are not `.`, `..` or empty and that is not a
    /// URL.
    #[hegel::composite]
    fn source_paths(tc: &TestCase) -> String {
        let segments = tc.draw_silent(
            vecs(tuples2(
                text().exclude_characters("/:"),
                text().exclude_characters("/.:").min_size(1).max_size(1),
            ))
            .min_size(1)
            .max_size(3),
        );
        let mut path = String::new();
        for (index, (start, end)) in segments.iter().enumerate() {
            if index > 0 {
                path.push('/');
            }
            path.push_str(start);
            path.push_str(end);
        }
        path
    }

    /// The URL of a map in the `directories`, one in the other.
    fn map_url(directories: &[&str]) -> Url {
        let mut url = "https://example.com/".to_string();
        for directory in directories {
            url.push_str(directory);
            url.push('/');
        }
        url.push_str("bundle.js.map?v=1/2");
        Url::parse(&url).unwrap()
    }

    const DIRECTORIES: [&str; 4] = ["assets", "static", "js", "a-b_c"];

    #[hegel::composite]
    fn directories(tc: &TestCase) -> Vec<&'static str> {
        tc.draw_silent(vecs(sampled_from(DIRECTORIES.as_slice())).max_size(3))
    }

    /// The `sourceRoot` of a map.
    #[derive(Debug)]
    struct Root {
        /// What is before the directories: nothing for a relative root, `/`
        /// for an absolute path, or the scheme and authority of a URL.
        prefix: &'static str,
        directories: Vec<&'static str>,
        slash_last: bool,
    }

    const ROOT_PREFIXES: [&str; 6] = [
        "",
        "/",
        "webpack://app/",
        "webpack:///",
        "file:///",
        "https://example.com/",
    ];

    #[hegel::composite]
    fn roots(tc: &TestCase) -> Root {
        Root {
            prefix: tc.draw_silent(sampled_from(ROOT_PREFIXES.as_slice())),
            directories: tc.draw_silent(directories()),
            slash_last: tc.draw_silent(booleans()),
        }
    }

    impl Root {
        fn text(&self) -> String {
            let mut text =
                format!("{}{}", self.prefix, self.directories.join("/"));
            if self.slash_last && !self.directories.is_empty() {
                text.push('/');
            }
            text
        }
    }

    /// The key that a map with this root, at the URL of `base`, has for
    /// its `source`.
    fn key_in_map(
        base: Option<&[&str]>,
        root: Option<&Root>,
        source: &str,
    ) -> String {
        let json = serde_json::json!({
            "sources": [source],
            "sourceRoot": root.map(Root::text),
            "mappings": "AAAA",
        })
        .to_string();
        let base = base.map(map_url);
        let map = SourceMap::parse(&json, base.as_ref()).unwrap();
        map.lookup(1, 0).unwrap().source.to_string()
    }

    #[hegel::test]
    fn relative_sources_are_keyed_from_the_source_root_and_the_map(
        tc: TestCase,
    ) {
        let base = tc.draw(optional(directories()));
        let root = tc.draw(optional(roots()).print_as_debug());
        let steps =
            tc.draw(vecs(sampled_from(["./", "../"].as_slice())).max_size(5));
        let path = tc.draw(source_paths());
        let source = format!("{}{path}", steps.concat());
        let key = key_in_map(base.as_deref(), root.as_ref(), &source);

        // The directories of the map, unless the root is not relative to
        // them, then those of the root.
        let mut above = match &root {
            Some(root) if !root.prefix.is_empty() => Vec::new(),
            _ => base.unwrap_or_default(),
        };
        above.extend(root.iter().flat_map(|root| &root.directories));
        let ups = steps.iter().filter(|step| **step == "../").count();
        above.truncate(above.len().saturating_sub(ups));
        let mut expected = String::new();
        for directory in above {
            expected.push_str(directory);
            expected.push('/');
        }
        expected.push_str(&path);
        assert_eq!(key, expected);
    }

    #[hegel::test]
    fn sources_that_stand_alone_are_keyed_the_same_in_every_map(tc: TestCase) {
        let base = tc.draw(optional(directories()));
        let root = tc.draw(optional(roots()).print_as_debug());
        // An authority need not be a host that a URL could have.
        let prefix = tc.draw(sampled_from(
            [
                "/",
                "webpack://app/",
                "webpack:///",
                "webpack://my app/",
                "webpack://[name]/",
                "file:///",
            ]
            .as_slice(),
        ));
        let path = tc.draw(source_paths());
        let source = format!("{prefix}{path}");
        assert_eq!(key_in_map(base.as_deref(), root.as_ref(), &source), path);
    }

    const URL_ALPHABET: &str = "abcXYZ019./:-_?=&%#@";

    #[derive(Clone, Debug)]
    enum Line {
        Code,
        Blank,
        Comment,
        MappingUrl { at_sign: bool, url: String },
    }

    impl Line {
        fn render(&self, out: &mut String) {
            match self {
                Line::Code => out.push_str("x(); // y"),
                Line::Blank => out.push_str(" \t"),
                Line::Comment => out.push_str("// sourceMappingURL"),
                Line::MappingUrl { at_sign, url } => {
                    out.push_str(if *at_sign { "//@" } else { "//#" });
                    out.push_str(" sourceMappingURL=");
                    out.push_str(url);
                }
            }
        }
    }

    #[hegel::composite]
    fn lines(tc: &TestCase, code: bool) -> Line {
        let kind = tc.draw_silent(integers::<u8>().max_value(3));
        match kind {
            0 if code => Line::Code,
            0 | 1 => Line::Blank,
            2 => Line::Comment,
            _ => Line::MappingUrl {
                at_sign: tc.draw_silent(booleans()),
                url: tc.draw_silent(text().alphabet(URL_ALPHABET).min_size(1)),
            },
        }
    }

    #[hegel::test]
    fn the_last_trailing_comment_gives_the_mapping_url(tc: TestCase) {
        // Any lines, then a line of code if there is one at all, then
        // lines that are not code.
        let mut script =
            tc.draw(vecs(lines(true)).max_size(4).print_as_debug());
        if !script.is_empty() {
            script.push(Line::Code);
        }
        let trailing = tc.draw(vecs(lines(false)).max_size(5).print_as_debug());
        script.extend(trailing.iter().cloned());

        let mut text = String::new();
        for (index, line) in script.iter().enumerate() {
            line.render(&mut text);
            if index + 1 < script.len() || tc.draw(booleans()) {
                text.push_str(tc.draw(sampled_from(vec!["\n", "\r\n"])));
            }
        }
        let expected = trailing.iter().rev().find_map(|line| match line {
            Line::MappingUrl { url, .. } => Some(url.as_str()),
            _ => None,
        });
        assert_eq!(source_mapping_url(&text), expected);
    }

    #[hegel::test]
    fn data_urls_locate_the_map_inline(tc: TestCase) {
        let json = tc.draw(text());
        let script_url = Url::parse("https://example.com/app.js").unwrap();
        let media_type = tc.draw(sampled_from(vec![
            "",
            "application/json",
            "application/json;charset=utf-8",
        ]));
        let map_url = if tc.draw(booleans()) {
            let base64 = tc.draw(sampled_from(vec![";base64", ";BASE64"]));
            format!(
                "data:{media_type}{base64},{}",
                BASE64_STANDARD.encode(&json)
            )
        } else {
            format!(
                "data:{media_type},{}",
                percent_encoding::utf8_percent_encode(
                    &json,
                    percent_encoding::NON_ALPHANUMERIC
                )
            )
        };
        assert_eq!(
            locate(&script_url, &map_url).unwrap(),
            Location::Inline(json)
        );
    }

    #[test]
    fn other_urls_locate_the_map_relative_to_the_script() {
        let script_url =
            Url::parse("https://example.com/static/app.js?v=1").unwrap();
        let located = |map_url: &str| match locate(&script_url, map_url) {
            Ok(Location::Remote(url)) => String::from(url),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            located("app.js.map"),
            "https://example.com/static/app.js.map"
        );
        assert_eq!(
            located("/maps/app.map"),
            "https://example.com/maps/app.map"
        );
        assert_eq!(
            located("https://maps.example.com/app.map"),
            "https://maps.example.com/app.map"
        );
        assert!(locate(&script_url, "data:application/json").is_err());
    }
}
