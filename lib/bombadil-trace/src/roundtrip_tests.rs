//! Properties of reading back what [`TraceFileWriter`] wrote, whole or
//! cut short.

use std::{collections::HashMap, fs::File, io::Cursor, path::Path, sync::Arc};

use anyhow::Result;
use hegel::{
    Generator, TestCase,
    generators::{
        binary, booleans, integers, one_of, optional, sampled_from, text, vecs,
    },
};
use serde::Serialize;
use serde_json::value::RawValue;

use crate::{
    ENVELOPE_LEN, PAYLOAD_REF_LEN, RECORD_HEADER_LEN, State, StateReader,
    TRACE_FILE_NAME, TraceDriver, TraceFileWriter, open_trace, read_payload_at,
    read_payload_from, record::PAYLOAD_READ_CHUNK,
};

/// Metadata without snapshots, encoded by serde_json.
#[derive(Debug, Clone, Serialize)]
struct Metadata {
    label: String,
    number: u64,
}

/// A state's metadata, given to the writer as JSON text, as is.
#[derive(Debug, Clone)]
enum Shape {
    Plain(Metadata),
    /// An object with a `snapshots` member before or after a `label`
    /// member, and, if there is a violation, a `violations` member last,
    /// whose snapshots hold an entry drawn on its own, which isn't one of
    /// the state's entries.
    Snapshots {
        label: String,
        snapshots_first: bool,
        entries: Vec<Entry>,
        violation: Option<Entry>,
    },
}

/// A snapshot entry, its value given as JSON text.
#[derive(Debug, Clone)]
struct Entry {
    extractor: u64,
    value: String,
    style: Style,
}

/// How an entry is written.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Style {
    /// As the schema's `Snapshot` is.
    Schema,
    /// With its value before its index.
    ValueFirst,
    /// With a space before the colon after `"value"`: not an entry.
    Spaced,
    /// With an escape in its `"value"` key: not an entry.
    Escaped,
}

impl Style {
    fn is_entry(self) -> bool {
        matches!(self, Style::Schema | Style::ValueFirst)
    }
}

impl Entry {
    /// The entry with `member`, `"value":<value>` or a reference.
    fn render(&self, member: &str) -> String {
        let index = self.extractor;
        match self.style {
            Style::Schema => format!(
                r#"{{"index":{index},"name":"e","{member},"time":{index}}}"#
            ),
            Style::ValueFirst => format!(r#"{{"{member},"index":{index}}}"#),
            Style::Spaced => {
                format!(r#"{{"index":{index},"value" :{}}}"#, self.value)
            }
            Style::Escaped => {
                format!(r#"{{"index":{index},"\u0076alue":{}}}"#, self.value)
            }
        }
    }

    fn in_full(&self) -> String {
        self.render(&format!(r#"value":{}"#, self.value))
    }
}

impl Shape {
    /// The metadata with each entry's value in full, or where `from` says,
    /// as a reference to that state.
    fn render(&self, from: &[Option<usize>]) -> String {
        match self {
            Shape::Plain(metadata) => serde_json::to_string(metadata).unwrap(),
            Shape::Snapshots {
                label,
                snapshots_first,
                entries,
                violation,
            } => {
                let entries: Vec<String> = entries
                    .iter()
                    .zip(from)
                    .map(|(entry, from)| match from {
                        Some(from) => {
                            entry.render(&format!(r#"value_from":{from}"#))
                        }
                        None => entry.in_full(),
                    })
                    .collect();
                let snapshots =
                    format!(r#""snapshots":[{}]"#, entries.join(","));
                let label = format!(
                    r#""label":{}"#,
                    serde_json::to_string(label).unwrap()
                );
                let mut members = if *snapshots_first {
                    vec![snapshots, label]
                } else {
                    vec![label, snapshots]
                };
                if let Some(violation) = violation {
                    members.push(format!(
                        r#""violations":[{{"snapshots":[{}]}}]"#,
                        violation.in_full()
                    ));
                }
                format!("{{{}}}", members.join(","))
            }
        }
    }

    fn entries_mut(&mut self) -> &mut [Entry] {
        match self {
            Shape::Plain(_) => &mut [],
            Shape::Snapshots { entries, .. } => entries,
        }
    }
}

/// A state as written: its metadata and payload.
#[derive(Debug)]
struct Written {
    metadata: Shape,
    payload: Option<Payload>,
}

/// A payload, printed by its length, checksum and first bytes rather
/// than all of them, so a failing case with a large payload, or an
/// assertion on one, prints briefly. Shared, so that a state can be
/// written with the same allocation as an earlier one, as a driver does
/// when its screen didn't change.
#[derive(Clone, PartialEq)]
struct Payload(Arc<Vec<u8>>);

impl std::fmt::Debug for Payload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let start = &self.0[..self.0.len().min(8)];
        let crc = crc32fast::hash(&self.0);
        write!(
            f,
            "Payload({} bytes, CRC-32 {crc:08x}, starting {start:02x?})",
            self.0.len()
        )
    }
}

hegel::pretty_print_as_debug!(Payload);
hegel::pretty_print_as_debug!(Shape);
hegel::pretty_print_as_debug!(Entry);
hegel::pretty_print_as_debug!(Style);

/// A payload of `min..=max` bytes that don't repeat with a short period,
/// so a misplaced read can't match.
#[hegel::composite]
fn patterned_payload(tc: &TestCase, min: usize, max: usize) -> Payload {
    let length = tc.draw(integers::<usize>().min_value(min).max_value(max));
    let seed = tc.draw(integers::<u32>());
    Payload(Arc::new(
        (0..length as u32)
            .map(|i| {
                (i.wrapping_add(seed).wrapping_mul(0x9e37_79b1) >> 24) as u8
            })
            .collect(),
    ))
}

/// A string, as serde_json encodes it: any unicode, including quotes,
/// backslashes and control characters that JSON escapes.
#[hegel::composite]
fn json_string(tc: &TestCase, max: usize) -> String {
    serde_json::to_string(&tc.draw(text().max_size(max))).unwrap()
}

/// Values in pairs that differ in one byte: `SIBLINGS[n ^ 1]` is
/// `SIBLINGS[n]`'s. With its key, the first pair is exactly as long as a
/// reference to a state below 10, and the others as one to a state from
/// 10 on, so whether a reference is written depends on the state.
const SIBLINGS: [&str; 6] = [
    r#""abcd""#,
    r#""abce""#,
    r#""abcde""#,
    r#""abcdf""#,
    "1234567",
    "1234568",
];

/// A JSON value as text, in forms serde_json wouldn't write itself, or
/// holding `value_from` and `value` members, or long, or often one of
/// the [`SIBLINGS`].
#[hegel::composite]
fn json_value(tc: &TestCase) -> String {
    match tc.draw(integers::<u8>().max_value(6)) {
        0 => tc.draw(integers::<i64>()).to_string(),
        1 => tc
            .draw(sampled_from(&[
                "1.50",
                "1e300",
                "-0",
                "0.1E-5",
                "18446744073709551616",
                "true",
                "null",
                "[]",
                "{ }",
            ]))
            .to_string(),
        2 => tc.draw(json_string(24)),
        3 => format!(
            r#"{{"value_from":{},"value":{}}}"#,
            tc.draw(integers::<u8>().max_value(3)),
            tc.draw(json_string(8))
        ),
        4 => format!(
            "[{}]",
            tc.draw(vecs(json_string(16)).max_size(8)).join(",")
        ),
        _ => tc.draw(sampled_from(&SIBLINGS)).to_string(),
    }
}

#[hegel::composite]
fn entry(tc: &TestCase) -> Entry {
    Entry {
        // Few extractors, so their values repeat, and an extractor may
        // have more than one entry in a state.
        extractor: tc.draw(integers::<u64>().max_value(1)),
        value: tc.draw(json_value()),
        style: tc.draw(sampled_from(&[
            Style::Schema,
            Style::Schema,
            Style::ValueFirst,
            Style::Spaced,
            Style::Escaped,
        ])),
    }
}

#[hegel::composite]
fn shape(tc: &TestCase) -> Shape {
    if tc.draw(booleans()) {
        Shape::Plain(Metadata {
            label: tc.draw(text().max_size(32)),
            number: tc.draw(integers()),
        })
    } else {
        Shape::Snapshots {
            label: tc.draw(text().max_size(8)),
            snapshots_first: tc.draw(booleans()),
            entries: tc.draw(vecs(entry()).max_size(4)),
            violation: tc.draw(optional(entry())),
        }
    }
}

#[hegel::composite]
fn written(tc: &TestCase) -> Written {
    Written {
        metadata: tc.draw(shape()),
        // Small arbitrary bytes, up to one chunk, or several chunks.
        payload: tc.draw(optional(one_of([
            binary()
                .max_size(64)
                .map(|bytes| Payload(Arc::new(bytes)))
                .boxed(),
            patterned_payload(0, PAYLOAD_READ_CHUNK).boxed(),
            patterned_payload(PAYLOAD_READ_CHUNK + 1, 150 * 1024).boxed(),
        ]))),
    }
}

/// States to write, some of them with the payload of an earlier state
/// again: the same allocation (as the writer compares first), or a copy
/// of its bytes. And some of their snapshots with a value their
/// extractor had before: its last one, or an older one, or with the
/// sibling of its last one, or that of an earlier entry of the same
/// extractor in the state.
#[hegel::composite]
fn written_states(tc: &TestCase) -> Vec<Written> {
    let mut states = tc.draw(vecs(written().print_as_debug()).max_size(12));
    // Each extractor's values so far, last last.
    let mut values: HashMap<u64, Vec<String>> = HashMap::new();
    for index in 0..states.len() {
        let earlier: Vec<Payload> = states[..index]
            .iter()
            .filter_map(|state| state.payload.clone())
            .collect();
        if !earlier.is_empty() && tc.draw(booleans()) {
            let source = tc.draw(sampled_from(&earlier));
            states[index].payload = Some(if tc.draw(booleans()) {
                source
            } else {
                Payload(Arc::new(source.0.to_vec()))
            });
        }
        let mut seen: Vec<(u64, String)> = Vec::new();
        for entry in states[index].metadata.entries_mut() {
            if let Some(history) = values.get(&entry.extractor) {
                let last = history.last().unwrap();
                match tc.draw(integers::<u8>().max_value(3)) {
                    0 => {}
                    1 => entry.value = last.clone(),
                    2 => entry.value = tc.draw(sampled_from(history)),
                    // Its last value's sibling, if it has one.
                    _ => {
                        if let Some(n) =
                            SIBLINGS.iter().position(|sibling| sibling == last)
                        {
                            entry.value = SIBLINGS[n ^ 1].to_string();
                        }
                    }
                }
            }
            // Or the value of an earlier entry of its extractor in this
            // state, which the writer writes in full both times.
            if let Some((_, value)) = seen
                .iter()
                .rfind(|(extractor, _)| *extractor == entry.extractor)
                && tc.draw(booleans())
            {
                entry.value = value.clone();
            }
            seen.push((entry.extractor, entry.value.clone()));
        }
        for (extractor, value) in seen {
            values.entry(extractor).or_default().push(value);
        }
    }
    states
}

/// What the writer is given for each state, and what it writes for it,
/// by the documented rule: an entry's value as a reference to the last
/// earlier state holding its extractor's value in full, if that is the
/// same and the reference is shorter.
fn metadata(states: &[Written]) -> Vec<(String, String)> {
    let mut last: HashMap<u64, (usize, String)> = HashMap::new();
    states
        .iter()
        .enumerate()
        .map(|(state, written)| {
            let entries = match &written.metadata {
                Shape::Plain(_) => &[][..],
                Shape::Snapshots { entries, .. } => entries,
            };
            let from: Vec<Option<usize>> = entries
                .iter()
                .map(|entry| {
                    if !entry.style.is_entry() {
                        return None;
                    }
                    if let Some((from, value)) = last.get(&entry.extractor)
                        && *from < state
                        && *value == entry.value
                        && format!(r#""value_from":{from}"#).len()
                            < format!(r#""value":{value}"#).len()
                    {
                        return Some(*from);
                    }
                    last.insert(entry.extractor, (state, entry.value.clone()));
                    None
                })
                .collect();
            let in_full = vec![None; entries.len()];
            (
                written.metadata.render(&in_full),
                written.metadata.render(&from),
            )
        })
        .collect()
}

/// Writes `states`, with the metadata in `metadata`, with a
/// [`TraceFileWriter`] for `driver`, finished, to `trace.bin` in
/// `directory`.
fn write_trace(
    directory: &Path,
    driver: TraceDriver,
    states: &[Written],
    metadata: &[(String, String)],
) {
    let mut writer = TraceFileWriter::create(
        &directory.join(TRACE_FILE_NAME),
        driver,
        false,
    )
    .unwrap();
    for (state, (metadata, _)) in states.iter().zip(metadata) {
        let payload = state.payload.clone().map(|payload| payload.0);
        let metadata = RawValue::from_string(metadata.clone()).unwrap();
        writer.write_state(metadata, payload).unwrap();
    }
    writer.finish().unwrap();
    let metadata = RawValue::from_string("{}".to_string()).unwrap();
    let error = writer.write_state(metadata, None).unwrap_err();
    assert!(error.to_string().contains("already finished"), "{error}");
}

/// Where a state's records are in the file, by the documented layout.
#[derive(Clone, Copy, Debug)]
struct Placed {
    /// Where its metadata is.
    metadata_offset: u64,
    /// Where its records end. The END record follows the last state.
    end: u64,
    /// Where its payload's record starts: a state payload record, or a
    /// payload reference record.
    payload_record: Option<u64>,
    /// Where the bytes of its payload are: in its own record, or in the
    /// record of the state it shares them with.
    payload_offset: Option<u64>,
    /// The state whose record holds its payload: the first state with
    /// those bytes, if they are larger than a reference, else itself.
    payload_from: Option<usize>,
}

/// Where each state's records are in the file, by the documented layout
/// and what the writer refers to, its metadata as in `metadata`.
fn layout(states: &[Written], metadata: &[(String, String)]) -> Vec<Placed> {
    let mut position = ENVELOPE_LEN as u64;
    let mut placed: Vec<Placed> = Vec::new();
    for (index, (state, (_, written))) in
        states.iter().zip(metadata).enumerate()
    {
        let metadata_offset = position + RECORD_HEADER_LEN as u64;
        position = metadata_offset + written.len() as u64;
        let mut state_placed = Placed {
            metadata_offset,
            end: position,
            payload_record: None,
            payload_offset: None,
            payload_from: None,
        };
        if let Some(payload) = &state.payload {
            let shared = (payload.0.len() > PAYLOAD_REF_LEN)
                .then(|| {
                    states[..index].iter().position(|earlier| {
                        earlier.payload.as_ref().is_some_and(|earlier| {
                            earlier.0[..] == payload.0[..]
                        })
                    })
                })
                .flatten();
            state_placed.payload_record = Some(position);
            let body_length = match shared {
                Some(from) => {
                    state_placed.payload_offset = placed[from].payload_offset;
                    state_placed.payload_from = Some(from);
                    PAYLOAD_REF_LEN
                }
                None => {
                    let offset = position + RECORD_HEADER_LEN as u64;
                    state_placed.payload_offset = Some(offset);
                    state_placed.payload_from = Some(index);
                    payload.0.len()
                }
            };
            position += (RECORD_HEADER_LEN + body_length) as u64;
            state_placed.end = position;
        }
        placed.push(state_placed);
    }
    placed
}

/// Checks that `read` are the first states of `written`, placed as in
/// `layout`, with their payloads unless `without_payloads`, and with the
/// very metadata the writer was given (`metadata`).
fn check_states(
    read: &[State<Box<RawValue>>],
    written: &[Written],
    metadata: &[(String, String)],
    layout: &[Placed],
    without_payloads: bool,
) {
    for (index, (((read, written), (metadata, _)), placed)) in read
        .iter()
        .zip(written)
        .zip(metadata)
        .zip(layout)
        .enumerate()
    {
        assert_eq!(read.index, index);
        assert_eq!(read.metadata.get(), metadata, "state {index}");
        if without_payloads {
            assert_eq!(read.payload, None, "state {index}");
        } else {
            assert_eq!(
                read.payload.as_deref(),
                written.payload.as_ref().map(|payload| &payload.0[..]),
                "state {index}"
            );
        }
        assert_eq!(
            read.payload_length,
            written.payload.as_ref().map(|payload| payload.0.len()),
            "state {index}"
        );
        assert_eq!(read.payload_offset, placed.payload_offset, "state {index}");
        assert_eq!(read.payload_from, placed.payload_from, "state {index}");
    }
}

/// Checks that the trace `file` holds the metadata the writer writes for
/// each state (`metadata`) where `layout` places it.
fn check_metadata_written(
    file: &[u8],
    metadata: &[(String, String)],
    layout: &[Placed],
) {
    for (index, ((_, written), placed)) in
        metadata.iter().zip(layout).enumerate()
    {
        let start = placed.metadata_offset as usize;
        let bytes = &file[start..start + written.len()];
        assert_eq!(bytes, written.as_bytes(), "state {index}");
    }
}

/// A finished trace reads back as the states written, with or without
/// their payloads, those shared with an earlier state written once, and
/// each payload reads back at its offset. Snapshot values repeating
/// their extractor's last value in full are written once, and the
/// metadata reads back byte for byte as the writer was given it.
#[hegel::test]
fn reads_back_what_was_written(tc: TestCase) {
    let states = tc.draw(written_states().print_as_debug());
    let driver = tc.draw(
        sampled_from(&[TraceDriver::Terminal, TraceDriver::Browser])
            .print_as_debug(),
    );
    let metadata = metadata(&states);
    let directory = tempfile::tempdir().unwrap();
    write_trace(directory.path(), driver, &states, &metadata);
    let path = directory.path().join(TRACE_FILE_NAME);
    let layout = layout(&states, &metadata);
    let end = layout
        .last()
        .map_or(ENVELOPE_LEN as u64, |placed| placed.end);
    let file = std::fs::read(&path).unwrap();
    assert_eq!(file.len() as u64, end + RECORD_HEADER_LEN as u64);
    check_metadata_written(&file, &metadata, &layout);

    for without_payloads in [false, true] {
        let mut reader = open_trace::<Box<RawValue>>(&path).unwrap();
        if without_payloads {
            reader = reader.without_payloads();
        }
        assert_eq!(reader.driver, driver);
        let read = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
        assert!(reader.finished());
        assert_eq!(read.len(), states.len());
        check_states(&read, &states, &metadata, &layout, without_payloads);
    }

    // Shared payloads too, at the offset of the state holding them.
    let file = File::open(&path).unwrap();
    for (state, placed) in states.iter().zip(&layout) {
        if let (Some(payload), Some(offset)) =
            (&state.payload, placed.payload_offset)
        {
            assert_eq!(read_payload_at(&path, offset).unwrap(), *payload.0);
            assert_eq!(read_payload_from(&file, offset).unwrap(), *payload.0);
        }
    }
}

/// A trace cut anywhere after its envelope, as by a run killed while
/// writing, reads without an error as the states complete before the
/// cut, with the metadata the writer was given (snapshot values written
/// once put back), and is finished only if it wasn't cut. Before it is
/// cut, the trace holds the metadata as the writer writes it. Cuts
/// favour record and payload chunk boundaries and the bytes around them,
/// including those of payload reference records.
#[hegel::test]
fn reads_the_complete_states_of_a_cut_trace(tc: TestCase) {
    let states = tc.draw(written_states().print_as_debug());
    let metadata = metadata(&states);
    let directory = tempfile::tempdir().unwrap();
    write_trace(directory.path(), TraceDriver::Terminal, &states, &metadata);
    let file = std::fs::read(directory.path().join(TRACE_FILE_NAME)).unwrap();
    let layout = layout(&states, &metadata);
    // Where a record starts or ends: the end of the envelope, each state
    // record's body start and end (the start of its payload or reference
    // record), each payload or reference record's body start, and the end
    // of the file. And where the reader starts a payload's later chunks,
    // and where reference records start, kept apart as there are few of
    // them.
    let mut boundaries = vec![ENVELOPE_LEN];
    let mut chunk_boundaries = Vec::new();
    let mut references = Vec::new();
    let mut start = ENVELOPE_LEN;
    for (index, placed) in layout.iter().enumerate() {
        let end = placed.end as usize;
        boundaries.push(start + RECORD_HEADER_LEN);
        if let Some(record) = placed.payload_record {
            let record = record as usize;
            boundaries.extend([record, record + RECORD_HEADER_LEN]);
            if placed.payload_from == Some(index) {
                chunk_boundaries.extend(
                    (record + RECORD_HEADER_LEN + PAYLOAD_READ_CHUNK..end)
                        .step_by(PAYLOAD_READ_CHUNK),
                );
            } else {
                references.push(record);
            }
        }
        boundaries.push(end);
        start = end;
    }
    boundaries.push(file.len());
    let boundary = || {
        if !chunk_boundaries.is_empty() && tc.draw(booleans()) {
            tc.draw(sampled_from(&chunk_boundaries))
        } else {
            tc.draw(sampled_from(&boundaries))
        }
    };
    // Anywhere, often deep inside a large payload; exactly at a boundary;
    // within a header's length of one; or, half the time if there are
    // references, inside a reference record's header or body. So cuts at a
    // state boundary, before a payload's or a reference's record, inside a
    // header or a reference, at the start of a body, at a chunk boundary
    // or just before END are common.
    let cut = if !references.is_empty() && tc.draw(booleans()) {
        let record = tc.draw(sampled_from(&references));
        let (first, last) = if tc.draw(booleans()) {
            (record + 1, record + RECORD_HEADER_LEN - 1)
        } else {
            let body = record + RECORD_HEADER_LEN;
            (body, body + PAYLOAD_REF_LEN - 1)
        };
        tc.draw(integers::<usize>().min_value(first).max_value(last))
    } else {
        match tc.draw(integers::<u8>().max_value(2)) {
            0 => tc.draw(
                integers::<usize>()
                    .min_value(ENVELOPE_LEN)
                    .max_value(file.len()),
            ),
            1 => boundary(),
            _ => {
                let boundary = boundary();
                tc.draw(
                    integers::<usize>()
                        .min_value(
                            boundary
                                .saturating_sub(RECORD_HEADER_LEN)
                                .max(ENVELOPE_LEN),
                        )
                        .max_value(
                            (boundary + RECORD_HEADER_LEN).min(file.len()),
                        ),
                )
            }
        }
    };
    let complete = layout
        .iter()
        .take_while(|placed| placed.end <= cut as u64)
        .count();
    check_metadata_written(&file, &metadata, &layout);

    for without_payloads in [false, true] {
        let mut reader =
            StateReader::<_, Box<RawValue>>::new(Cursor::new(&file[..cut]))
                .unwrap();
        if without_payloads {
            reader = reader.without_payloads();
        }
        let read = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(reader.finished(), cut == file.len());
        assert_eq!(read.len(), complete);
        check_states(&read, &states, &metadata, &layout, without_payloads);
    }
}
