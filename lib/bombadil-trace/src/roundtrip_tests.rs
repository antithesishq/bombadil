//! Properties of reading back what [`TraceFileWriter`] wrote, whole or
//! cut short.

use std::{fs::File, io::Cursor, path::Path, sync::Arc};

use anyhow::Result;
use hegel::{
    Generator, TestCase,
    generators::{
        binary, booleans, integers, one_of, optional, sampled_from, text, vecs,
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    ENVELOPE_LEN, PAYLOAD_REF_LEN, RECORD_HEADER_LEN, State, StateReader,
    TRACE_FILE_NAME, TraceDriver, TraceFileWriter, open_trace, read_payload_at,
    read_payload_from, record::PAYLOAD_READ_CHUNK,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Metadata {
    label: String,
    number: u64,
}

/// A state as written: its metadata and payload.
#[derive(Debug)]
struct Written {
    metadata: Metadata,
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

#[hegel::composite]
fn written(tc: &TestCase) -> Written {
    Written {
        metadata: Metadata {
            // Any unicode, including quotes, backslashes and control
            // characters that JSON escapes.
            label: tc.draw(text().max_size(32)),
            number: tc.draw(integers()),
        },
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
/// of its bytes.
#[hegel::composite]
fn written_states(tc: &TestCase) -> Vec<Written> {
    let mut states = tc.draw(vecs(written().print_as_debug()).max_size(12));
    for index in 1..states.len() {
        let earlier: Vec<Payload> = states[..index]
            .iter()
            .filter_map(|state| state.payload.clone())
            .collect();
        if earlier.is_empty() || !tc.draw(booleans()) {
            continue;
        }
        let source = tc.draw(sampled_from(&earlier));
        states[index].payload = Some(if tc.draw(booleans()) {
            source
        } else {
            Payload(Arc::new(source.0.to_vec()))
        });
    }
    states
}

/// Writes `states` with a [`TraceFileWriter`] for `driver`, finished, to
/// `trace.bin` in `directory`.
fn write_trace(directory: &Path, driver: TraceDriver, states: &[Written]) {
    let mut writer = TraceFileWriter::create(
        &directory.join(TRACE_FILE_NAME),
        driver,
        false,
    )
    .unwrap();
    for state in states {
        let payload = state.payload.clone().map(|payload| payload.0);
        writer.write_state(state.metadata.clone(), payload).unwrap();
    }
    writer.finish().unwrap();
    let metadata = Metadata {
        label: String::new(),
        number: 0,
    };
    let error = writer.write_state(metadata, None).unwrap_err();
    assert!(error.to_string().contains("already finished"), "{error}");
}

/// Where a state's records are in the file, by the documented layout.
#[derive(Clone, Copy, Debug)]
struct Placed {
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
/// and what the writer refers to.
fn layout(states: &[Written]) -> Vec<Placed> {
    let mut position = ENVELOPE_LEN as u64;
    let mut placed: Vec<Placed> = Vec::new();
    for (index, state) in states.iter().enumerate() {
        let json = serde_json::to_vec(&state.metadata).unwrap();
        position += (RECORD_HEADER_LEN + json.len()) as u64;
        let mut state_placed = Placed {
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
/// `layout`, with their payloads unless `without_payloads`.
fn check_states(
    read: &[State<Metadata>],
    written: &[Written],
    layout: &[Placed],
    without_payloads: bool,
) {
    for (index, ((read, written), placed)) in
        read.iter().zip(written).zip(layout).enumerate()
    {
        assert_eq!(read.index, index);
        assert_eq!(read.metadata, written.metadata, "state {index}");
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

/// A finished trace reads back as the states written, with or without
/// their payloads, those shared with an earlier state written once, and
/// each payload reads back at its offset.
#[hegel::test]
fn reads_back_what_was_written(tc: TestCase) {
    let states = tc.draw(written_states().print_as_debug());
    let driver = tc.draw(
        sampled_from(&[TraceDriver::Terminal, TraceDriver::Browser])
            .print_as_debug(),
    );
    let directory = tempfile::tempdir().unwrap();
    write_trace(directory.path(), driver, &states);
    let path = directory.path().join(TRACE_FILE_NAME);
    let layout = layout(&states);
    let end = layout
        .last()
        .map_or(ENVELOPE_LEN as u64, |placed| placed.end);
    let file_length = std::fs::metadata(&path).unwrap().len();
    assert_eq!(file_length, end + RECORD_HEADER_LEN as u64);

    for without_payloads in [false, true] {
        let mut reader = open_trace::<Metadata>(&path).unwrap();
        if without_payloads {
            reader = reader.without_payloads();
        }
        assert_eq!(reader.driver, driver);
        let read = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
        assert!(reader.finished());
        assert_eq!(read.len(), states.len());
        check_states(&read, &states, &layout, without_payloads);
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
/// cut, and is finished only if it wasn't cut. Cuts favour record and
/// payload chunk boundaries and the bytes around them, including those
/// of payload reference records.
#[hegel::test]
fn reads_the_complete_states_of_a_cut_trace(tc: TestCase) {
    let states = tc.draw(written_states().print_as_debug());
    let directory = tempfile::tempdir().unwrap();
    write_trace(directory.path(), TraceDriver::Terminal, &states);
    let file = std::fs::read(directory.path().join(TRACE_FILE_NAME)).unwrap();
    let layout = layout(&states);
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

    for without_payloads in [false, true] {
        let mut reader =
            StateReader::<_, Metadata>::new(Cursor::new(&file[..cut])).unwrap();
        if without_payloads {
            reader = reader.without_payloads();
        }
        let read = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(reader.finished(), cut == file.len());
        assert_eq!(read.len(), complete);
        check_states(&read, &states, &layout, without_payloads);
    }
}
