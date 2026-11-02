//! Properties of reading back what [`TraceFileWriter`] wrote, whole or
//! cut short.

use std::{fs::File, path::Path, sync::Arc};

use anyhow::Result;
use hegel::{
    Generator, TestCase,
    generators::{
        binary, booleans, integers, one_of, optional, sampled_from, text, vecs,
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    ENVELOPE_LEN, RECORD_HEADER_LEN, State, StateReader, TRACE_FILE_NAME,
    TraceDriver, TraceFileWriter, open_trace, read_payload_at,
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
/// assertion on one, prints briefly.
#[derive(Clone, PartialEq)]
struct Payload(Vec<u8>);

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
    Payload(
        (0..length as u32)
            .map(|i| {
                (i.wrapping_add(seed).wrapping_mul(0x9e37_79b1) >> 24) as u8
            })
            .collect(),
    )
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
            binary().max_size(64).map(Payload).boxed(),
            patterned_payload(0, PAYLOAD_READ_CHUNK).boxed(),
            patterned_payload(PAYLOAD_READ_CHUNK + 1, 150 * 1024).boxed(),
        ]))),
    }
}

/// Writes `states` with a [`TraceFileWriter`] for `driver`, finished, to
/// `trace.bin` in `directory`.
fn write_trace(directory: &Path, driver: TraceDriver, states: &[Written]) {
    let mut writer = TraceFileWriter::create(directory, driver, false).unwrap();
    for state in states {
        let payload = state.payload.clone().map(|payload| Arc::new(payload.0));
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

/// Where each state's records end in the file, and where its payload
/// starts if it has one, by the documented layout. The END record
/// follows the last state.
fn layout(states: &[Written]) -> Vec<(u64, Option<u64>)> {
    let mut position = ENVELOPE_LEN as u64;
    states
        .iter()
        .map(|state| {
            let json = serde_json::to_vec(&state.metadata).unwrap();
            position += (RECORD_HEADER_LEN + json.len()) as u64;
            let payload_offset = state.payload.as_ref().map(|payload| {
                let offset = position + RECORD_HEADER_LEN as u64;
                position = offset + payload.0.len() as u64;
                offset
            });
            (position, payload_offset)
        })
        .collect()
}

/// Checks that `read` are the first states of `written`, at the offsets
/// of `layout`, with their payloads unless `without_payloads`.
fn check_states(
    read: &[State<Metadata>],
    written: &[Written],
    layout: &[(u64, Option<u64>)],
    without_payloads: bool,
) {
    for (index, ((read, written), (_, offset))) in
        read.iter().zip(written).zip(layout).enumerate()
    {
        assert_eq!(read.index, index);
        assert_eq!(read.metadata, written.metadata, "state {index}");
        if without_payloads {
            assert_eq!(read.payload, None, "state {index}");
        } else {
            assert_eq!(
                read.payload.clone().map(Payload),
                written.payload,
                "state {index}"
            );
        }
        assert_eq!(
            read.payload_length,
            written.payload.as_ref().map(|payload| payload.0.len()),
            "state {index}"
        );
        assert_eq!(read.payload_offset, *offset, "state {index}");
    }
}

/// A finished trace reads back as the states written, with or without
/// their payloads, and each payload reads back at its offset.
#[hegel::test]
fn reads_back_what_was_written(tc: TestCase) {
    let states = tc.draw(vecs(written().print_as_debug()).max_size(12));
    let driver = tc.draw(
        sampled_from(&[TraceDriver::Terminal, TraceDriver::Browser])
            .print_as_debug(),
    );
    let directory = tempfile::tempdir().unwrap();
    write_trace(directory.path(), driver, &states);
    let path = directory.path().join(TRACE_FILE_NAME);
    let layout = layout(&states);
    let end = layout.last().map_or(ENVELOPE_LEN as u64, |&(end, _)| end);
    let file_length = std::fs::metadata(&path).unwrap().len();
    assert_eq!(file_length, end + RECORD_HEADER_LEN as u64);

    for without_payloads in [false, true] {
        let mut reader = open_trace::<Metadata>(directory.path()).unwrap();
        if without_payloads {
            reader = reader.without_payloads();
        }
        assert_eq!(reader.driver, driver);
        let read = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
        assert!(reader.finished());
        assert_eq!(read.len(), states.len());
        check_states(&read, &states, &layout, without_payloads);
    }

    let file = File::open(&path).unwrap();
    for (state, (_, offset)) in states.iter().zip(&layout) {
        if let (Some(payload), Some(offset)) = (&state.payload, offset) {
            assert_eq!(
                &Payload(read_payload_at(&path, *offset).unwrap()),
                payload
            );
            assert_eq!(
                &Payload(read_payload_from(&file, *offset).unwrap()),
                payload
            );
        }
    }
}

/// A trace cut anywhere after its envelope, as by a run killed while
/// writing, reads without an error as the states complete before the
/// cut, and is finished only if it wasn't cut. Cuts favour record and
/// payload chunk boundaries and the bytes around them.
#[hegel::test]
fn reads_the_complete_states_of_a_cut_trace(tc: TestCase) {
    let states = tc.draw(vecs(written().print_as_debug()).max_size(12));
    let directory = tempfile::tempdir().unwrap();
    write_trace(directory.path(), TraceDriver::Terminal, &states);
    let file = std::fs::read(directory.path().join(TRACE_FILE_NAME)).unwrap();
    let layout = layout(&states);
    // Where a record starts or ends: the end of the envelope, each state
    // record's body start and end, each payload record's header and body
    // start, and the end of the file. And where the reader starts a
    // payload's later chunks, kept apart as there are few of them.
    let mut boundaries = vec![ENVELOPE_LEN];
    let mut chunk_boundaries = Vec::new();
    let mut start = ENVELOPE_LEN;
    for &(end, payload) in &layout {
        let end = end as usize;
        boundaries.push(start + RECORD_HEADER_LEN);
        if let Some(offset) = payload {
            let offset = offset as usize;
            boundaries.extend([offset - RECORD_HEADER_LEN, offset]);
            chunk_boundaries.extend(
                (offset + PAYLOAD_READ_CHUNK..end).step_by(PAYLOAD_READ_CHUNK),
            );
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
    // or within a header's length of one, so cuts at a state boundary,
    // before a payload's record, inside a header, at the start of a body,
    // at a chunk boundary or just before END are common.
    let cut = match tc.draw(integers::<u8>().max_value(2)) {
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
                    .max_value((boundary + RECORD_HEADER_LEN).min(file.len())),
            )
        }
    };
    let complete = layout
        .iter()
        .take_while(|&&(end, _)| end <= cut as u64)
        .count();

    for without_payloads in [false, true] {
        let mut reader = StateReader::<_, Metadata>::new(&file[..cut]).unwrap();
        if without_payloads {
            reader = reader.without_payloads();
        }
        let read = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(reader.finished(), cut == file.len());
        assert_eq!(read.len(), complete);
        check_states(&read, &states, &layout, without_payloads);
    }
}
