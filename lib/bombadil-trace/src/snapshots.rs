//! Extractor snapshot values that repeat their extractor's previous value
//! are written once, as the [crate] docs say.
//!
//! Both sides find the snapshot entries of a state's metadata the same
//! way ([`snapshot_values`]) and splice bytes, so whatever isn't such an
//! entry's value (or reference) is passed through as is, and a restored
//! value is the bytes it was written from.

use std::{borrow::Cow, collections::HashMap, fmt};

use anyhow::{Result, anyhow, bail};
use serde::de::{self, Deserialize, IgnoredAny, MapAccess, Visitor};
use serde_json::value::RawValue;

use crate::RECORD_PAYLOAD_BYTES_MAX;

/// A snapshot entry's value written in full: its key, as serde_json
/// writes it, then the value.
const VALUE_KEY: &[u8] = br#""value":"#;
/// A snapshot entry's value written as the index of an earlier state with
/// that value: this key, then the index.
const VALUE_FROM_KEY: &[u8] = br#""value_from":"#;

/// The object keys that matter here.
#[derive(Clone, Copy, PartialEq)]
enum Key {
    Snapshots,
    Index,
    Value,
    ValueFrom,
    Other,
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: de::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct KeyVisitor;

        impl Visitor<'_> for KeyVisitor {
            type Value = Key;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an object key")
            }

            // Keys with escapes too, decoded.
            fn visit_str<E>(self, key: &str) -> Result<Key, E> {
                Ok(match key {
                    "snapshots" => Key::Snapshots,
                    "index" => Key::Index,
                    "value" => Key::Value,
                    "value_from" => Key::ValueFrom,
                    _ => Key::Other,
                })
            }
        }

        deserializer.deserialize_str(KeyVisitor)
    }
}

/// The members of a JSON object with a key that matters, in order, as
/// written: duplicates too.
struct Members<'a>(Vec<(Key, &'a RawValue)>);

impl<'de> Deserialize<'de> for Members<'de> {
    fn deserialize<D: de::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct MembersVisitor;

        impl<'de> Visitor<'de> for MembersVisitor {
            type Value = Members<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Members<'de>, A::Error> {
                let mut members = Vec::new();
                while let Some(key) = map.next_key::<Key>()? {
                    if key == Key::Other {
                        map.next_value::<IgnoredAny>()?;
                    } else {
                        members.push((key, map.next_value::<&RawValue>()?));
                    }
                }
                Ok(Members(members))
            }
        }

        deserializer.deserialize_map(MembersVisitor)
    }
}

impl<'a> Members<'a> {
    /// The values of the members named `keys`.
    fn named(&self, keys: &[Key]) -> Vec<(Key, &'a RawValue)> {
        self.0
            .iter()
            .filter(|(key, _)| keys.contains(key))
            .copied()
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Written {
    /// `"value":<value>`
    InFull,
    /// `"value_from":<state index>`
    From,
}

/// Where a snapshot entry's value, or its reference to an earlier
/// state's, is in a state's metadata.
#[derive(Debug, PartialEq)]
struct SnapshotValue {
    extractor: u64,
    written: Written,
    // Where the member starts: its key.
    start: usize,
    // Where its value (or the state index) starts and ends.
    value_start: usize,
    end: usize,
}

/// The snapshot entries of `metadata`, a state's encoded metadata, that
/// hold their extractor's value or refer to an earlier state's.
///
/// Those are the entries of the `snapshots` member of the metadata (an
/// object with one such member, an array), which are objects with one
/// `index` member, an integer (the extractor), and one member that is
/// either `value` or `value_from`, written as serde_json writes it: the
/// key without escapes, directly followed by a colon and then the value.
/// Anything else is left alone.
fn snapshot_values(metadata: &[u8]) -> serde_json::Result<Vec<SnapshotValue>> {
    let mut found = Vec::new();
    let first = metadata
        .iter()
        .find(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'));
    if first != Some(&b'{') {
        return Ok(found);
    }
    let top: Members = serde_json::from_slice(metadata)?;
    let [(_, snapshots)] = top.named(&[Key::Snapshots])[..] else {
        return Ok(found);
    };
    if !snapshots.get().starts_with('[') {
        return Ok(found);
    }
    let entries: Vec<&RawValue> = serde_json::from_str(snapshots.get())?;
    for entry in entries {
        if !entry.get().starts_with('{') {
            continue;
        }
        let members: Members = serde_json::from_str(entry.get())?;
        let [(_, index)] = members.named(&[Key::Index])[..] else {
            continue;
        };
        let Ok(extractor) = serde_json::from_str::<u64>(index.get()) else {
            continue;
        };
        let [(key, value)] = members.named(&[Key::Value, Key::ValueFrom])[..]
        else {
            continue;
        };
        let (written, key) = match key {
            Key::Value => (Written::InFull, VALUE_KEY),
            _ => (Written::From, VALUE_FROM_KEY),
        };
        let value_start = offset_in(metadata, value.get());
        let Some(start) = value_start
            .checked_sub(key.len())
            .filter(|&start| &metadata[start..value_start] == key)
        else {
            continue;
        };
        found.push(SnapshotValue {
            extractor,
            written,
            start,
            value_start,
            end: value_start + value.get().len(),
        });
    }
    Ok(found)
}

/// The offset of `part`, borrowed from `whole`, in `whole`.
fn offset_in(whole: &[u8], part: &str) -> usize {
    (part.as_ptr() as usize)
        .checked_sub(whole.as_ptr() as usize)
        .filter(|offset| offset + part.len() <= whole.len())
        .expect("a value borrowed from the metadata")
}

/// Writes snapshot values that repeat their extractor's previous value
/// as references to the state that holds it.
///
/// Keeps each extractor's last value written in full, and the state it
/// was written in.
#[derive(Default)]
pub(crate) struct SnapshotDeduplicator {
    last: HashMap<u64, (u64, Vec<u8>)>,
}

impl SnapshotDeduplicator {
    /// Replaces the snapshot values of `metadata`, the encoded metadata
    /// of state `state`, that repeat their extractor's last value written
    /// in full with references to the state holding it, if that is
    /// shorter.
    pub fn deduplicate(
        &mut self,
        metadata: &mut Vec<u8>,
        state: u64,
    ) -> Result<()> {
        let mut deduplicated = Vec::new();
        // How much of `metadata` is in `deduplicated`.
        let mut copied = 0;
        for found in snapshot_values(metadata)? {
            if found.written == Written::From {
                bail!(
                    "the snapshot of extractor {} has a `value_from` \
                     member, which trace metadata can't have",
                    found.extractor
                );
            }
            let value = &metadata[found.value_start..found.end];
            let reference = match self.last.get(&found.extractor) {
                Some((from, last)) if *from < state && last == value => {
                    let reference = format!("{from}");
                    (VALUE_FROM_KEY.len() + reference.len()
                        < found.end - found.start)
                        .then_some(reference)
                }
                _ => None,
            };
            match reference {
                Some(reference) => {
                    deduplicated
                        .extend_from_slice(&metadata[copied..found.start]);
                    deduplicated.extend_from_slice(VALUE_FROM_KEY);
                    deduplicated.extend_from_slice(reference.as_bytes());
                    copied = found.end;
                }
                None => {
                    let last = self.last.entry(found.extractor).or_default();
                    last.0 = state;
                    last.1.clear();
                    last.1.extend_from_slice(value);
                }
            }
        }
        if copied > 0 {
            deduplicated.extend_from_slice(&metadata[copied..]);
            *metadata = deduplicated;
        }
        Ok(())
    }
}

/// Puts back the snapshot values written as references.
///
/// Keeps each extractor's last value written in full, and the state it
/// was written in, which is the only one a reference may name.
pub(crate) struct SnapshotRestorer {
    last: HashMap<u64, (usize, Vec<u8>)>,
    // The size of metadata with its values put back, at most: that of
    // the metadata the writer encoded, before writing values once.
    bytes_max: usize,
}

impl Default for SnapshotRestorer {
    fn default() -> Self {
        SnapshotRestorer {
            last: HashMap::new(),
            bytes_max: RECORD_PAYLOAD_BYTES_MAX,
        }
    }
}

impl SnapshotRestorer {
    /// `metadata`, the encoded metadata of state `state`, read from
    /// trace record `record`, with the values written as references put
    /// back: as written if it has none.
    pub fn restore<'a>(
        &mut self,
        metadata: &'a [u8],
        state: usize,
        record: usize,
    ) -> Result<Cow<'a, [u8]>> {
        let found = snapshot_values(metadata).map_err(|error| {
            anyhow!(error)
                .context(format!("invalid state in trace record {record}"))
        })?;
        let mut restored = Vec::new();
        // How much of `metadata` is in `restored`.
        let mut copied = 0;
        // The size of the metadata with the values so far put back.
        let mut length = metadata.len();
        for found in found {
            let extractor = found.extractor;
            let value = &metadata[found.value_start..found.end];
            if found.written == Written::InFull {
                let last = self.last.entry(extractor).or_default();
                last.0 = state;
                last.1.clear();
                last.1.extend_from_slice(value);
                continue;
            }
            let corrupt = |reason: String| {
                anyhow!(
                    "corrupt snapshot value reference in trace record \
                     {record} (of extractor {extractor}, {reason})"
                )
            };
            let Ok(from) = serde_json::from_slice::<u64>(value) else {
                return Err(corrupt("not to a state index".to_string()));
            };
            if from >= state as u64 {
                return Err(corrupt(format!(
                    "to state {from}, which isn't before state {state}"
                )));
            }
            let last = match self.last.get(&extractor) {
                Some((last_from, last)) if *last_from as u64 == from => last,
                Some((last_from, _)) => {
                    return Err(corrupt(format!(
                        "to state {from}, but its last value in full is \
                         state {last_from}'s"
                    )));
                }
                None => {
                    return Err(corrupt(format!(
                        "to state {from}, but it has no earlier value in \
                         full"
                    )));
                }
            };
            // Checked before putting it back, so that a record can't be
            // made to take any amount of memory.
            length = length - (found.end - found.start)
                + VALUE_KEY.len()
                + last.len();
            if length > self.bytes_max {
                bail!(
                    "corrupt snapshot value references in trace record \
                     {record} (with its values put back, the state's \
                     metadata would be more than {} bytes)",
                    self.bytes_max
                );
            }
            restored.extend_from_slice(&metadata[copied..found.start]);
            restored.extend_from_slice(VALUE_KEY);
            restored.extend_from_slice(last);
            copied = found.end;
        }
        if copied == 0 {
            return Ok(Cow::Borrowed(metadata));
        }
        restored.extend_from_slice(&metadata[copied..]);
        Ok(Cow::Owned(restored))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(index: u64, value: &str) -> String {
        format!(r#"{{"index":{index},"name":"e","value":{value},"time":0}}"#)
    }

    fn state(entries: &[String]) -> String {
        format!(
            r#"{{"action":null,"snapshots":[{}],"violations":[]}}"#,
            entries.join(",")
        )
    }

    /// Each state of `originals` as written, after the ones before it.
    fn deduplicated(originals: &[String]) -> Vec<String> {
        let mut deduplicator = SnapshotDeduplicator::default();
        originals
            .iter()
            .enumerate()
            .map(|(index, original)| {
                let mut metadata = original.as_bytes().to_vec();
                deduplicator
                    .deduplicate(&mut metadata, index as u64)
                    .unwrap();
                String::from_utf8(metadata).unwrap()
            })
            .collect()
    }

    /// Each state of `written` as read, after the ones before it.
    fn restored(written: &[String]) -> Result<Vec<String>> {
        let mut restorer = SnapshotRestorer::default();
        written
            .iter()
            .enumerate()
            .map(|(index, written)| {
                let restored =
                    restorer.restore(written.as_bytes(), index, index)?;
                Ok(String::from_utf8(restored.into_owned()).unwrap())
            })
            .collect()
    }

    #[test]
    fn writes_repeated_values_once_and_restores_them() {
        let list = "[1,2,3,4,5,6,7,8,9]";
        let originals = [
            state(&[entry(0, list), entry(1, r#""a""#)]),
            state(&[entry(0, list), entry(1, r#""b""#)]),
            state(&[entry(0, list), entry(1, r#""a""#)]),
            state(&[entry(0, "[4]"), entry(1, r#""a""#)]),
            state(&[entry(0, list)]),
            // Twice in a state: in full both times.
            state(&[
                entry(0, "[2,2,2,2,2,2,2,2]"),
                entry(0, "[2,2,2,2,2,2,2,2]"),
            ]),
        ];
        let written = deduplicated(&originals);
        assert_eq!(
            written[1],
            state(&[
                r#"{"index":0,"name":"e","value_from":0,"time":0}"#.into(),
                entry(1, r#""b""#),
            ])
        );
        // Still state 0's, which holds it; short values in full.
        assert_eq!(
            written[2],
            state(&[
                r#"{"index":0,"name":"e","value_from":0,"time":0}"#.into(),
                entry(1, r#""a""#),
            ])
        );
        // A reference to state 2 would be longer than `"a"`.
        assert_eq!(written[3], originals[3]);
        // Only the previous value: after `[4]`, the list is written again.
        assert_eq!(written[4], originals[4]);
        assert_eq!(written[5], originals[5]);
        assert_eq!(restored(&written).unwrap(), originals);
    }

    /// Only the values of snapshot entries, as the `Snapshot` type writes
    /// them, are written once: not those of other members, nor members
    /// named `value_from` in a value.
    #[test]
    fn leaves_everything_else_as_written() {
        let value = r#"{"value_from":0,"value":"long enough"}"#;
        let originals = [
            // Elsewhere, or not in an object, or written otherwise.
            format!(
                r#"{{"snapshots":[{},{{"index":1,"value" :{value}}},{{"index":"2","value":{value}}},{{"index":3,"value":{value},"value":{value}}},{{"index":4,"index":4,"value":{value}}},5],"state":{{"snapshots":[{}]}},"violations":[{{"snapshots":[{}]}}]}}"#,
                entry(0, value),
                entry(0, value),
                entry(0, value),
            ),
            format!(
                r#"{{"snapshots":[{},{{"index":1,"value" :{value}}},{{"index":"2","value":{value}}},{{"index":3,"value":{value},"value":{value}}},{{"index":4,"index":4,"value":{value}}},5],"state":{{"snapshots":[{}]}},"violations":[{{"snapshots":[{}]}}]}}"#,
                entry(0, value),
                entry(0, value),
                entry(0, value),
            ),
            r#"[{"snapshots":[]}]"#.into(),
            r#""snapshots""#.into(),
            format!(r#"{{"snapshots":{{"0":{}}}}}"#, entry(0, value)),
            // Two lists of snapshots: neither is the list.
            format!(
                r#"{{"snapshots":[{}],"snapshots":[{}]}}"#,
                entry(0, value),
                entry(0, value)
            ),
        ];
        let written = deduplicated(&originals);
        let expected = originals[0].replacen(
            &entry(0, value),
            r#"{"index":0,"name":"e","value_from":0,"time":0}"#,
            1,
        );
        assert_eq!(written[0], originals[0]);
        assert_eq!(written[1], expected);
        assert_eq!(written[2..], originals[2..]);
        assert_eq!(restored(&written).unwrap(), originals);
    }

    /// Metadata with its values put back is at most as large as the
    /// bound, or else it is a corrupt record, found before it is put back.
    #[test]
    fn bounds_the_size_of_metadata_put_back() {
        let list = entry(0, "[1,2,3,4,5,6,7,8,9]");
        let originals = [
            state(std::slice::from_ref(&list)),
            state(&[list.clone(), list]),
        ];
        let written = deduplicated(&originals);
        assert_eq!(written[1].matches("value_from").count(), 2);
        let size = originals[1].len();
        for (bytes_max, fits) in [(size, true), (size - 1, false)] {
            let mut restorer = SnapshotRestorer {
                bytes_max,
                ..SnapshotRestorer::default()
            };
            restorer.restore(written[0].as_bytes(), 0, 0).unwrap();
            let restored = restorer.restore(written[1].as_bytes(), 1, 1);
            match restored {
                Ok(restored) => {
                    assert!(fits, "{bytes_max}");
                    assert_eq!(&*restored, originals[1].as_bytes());
                }
                Err(error) => {
                    assert!(!fits, "{bytes_max}: {error}");
                    assert!(
                        error.to_string().contains(&format!(
                            "corrupt snapshot value references in trace \
                             record 1 (with its values put back, the \
                             state's metadata would be more than \
                             {bytes_max} bytes)"
                        )),
                        "{error}"
                    );
                }
            }
        }
        assert_eq!(
            SnapshotRestorer::default().bytes_max,
            RECORD_PAYLOAD_BYTES_MAX
        );
    }

    #[test]
    fn refuses_metadata_that_reads_as_a_reference() {
        let mut metadata =
            br#"{"snapshots":[{"index":0,"value_from":0}]}"#.to_vec();
        let error = SnapshotDeduplicator::default()
            .deduplicate(&mut metadata, 1)
            .unwrap_err();
        assert!(error.to_string().contains("`value_from` member"), "{error}");
    }

    #[test]
    fn rejects_references_that_cannot_be_resolved() {
        let reference = |index: u64, from: &str| {
            state(&[format!(
                r#"{{"index":{index},"name":"e","value_from":{from},"time":0}}"#
            )])
        };
        let full = state(&[entry(0, "[1,2,3,4,5,6,7,8,9]")]);
        for (states, reason) in [
            (vec![reference(0, "0")], "isn't before state 0"),
            (
                vec![full.clone(), reference(0, "1")],
                "to state 1, which isn't before state 1",
            ),
            (vec![full.clone(), reference(0, "-1")], "state index"),
            (vec![full.clone(), reference(0, "0.0")], "state index"),
            (vec![full.clone(), reference(0, r#""0""#)], "state index"),
            (
                vec![full.clone(), reference(1, "0")],
                "no earlier value in full",
            ),
            // Not to an older value, nor to a reference.
            (
                vec![
                    full.clone(),
                    state(&[entry(0, "[2]")]),
                    reference(0, "0"),
                ],
                "last value in full is state 1's",
            ),
            (
                vec![full.clone(), reference(0, "0"), reference(0, "1")],
                "last value in full is state 0's",
            ),
        ] {
            let error = restored(&states).unwrap_err();
            let record = states.len() - 1;
            let error = error.to_string();
            assert!(
                error.contains(&format!(
                    "corrupt snapshot value reference in trace record \
                     {record} (of extractor"
                )) && error.contains(reason),
                "{error}"
            );
        }
    }
}
