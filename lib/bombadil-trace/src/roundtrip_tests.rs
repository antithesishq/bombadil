//! Properties of reading back what [`TraceFileWriter`] wrote, whole or
//! cut short.

use std::{fs::File, path::Path, sync::Arc};

use anyhow::Result;
use hegel::{
    Generator, TestCase,
    generators::{
        binary, integers, one_of, optional, sampled_from, text, vecs,
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

#[hegel::composite]
fn trace_states(tc: &TestCase) -> Vec<Written> {
    let count = tc.draw(one_of([
        integers::<usize>().max_value(65).boxed(),
        sampled_from(&[0usize, 1, 31, 32, 33, 63, 64, 65]).boxed(),
    ]));
    tc.draw(
        vecs(written().print_as_debug())
            .min_size(count)
            .max_size(count),
    )
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

fn layout(file: &[u8]) -> Vec<u64> {
    let mut position = ENVELOPE_LEN;
    let mut result = Vec::new();
    while position < file.len() - RECORD_HEADER_LEN {
        assert_eq!(
            u16::from_le_bytes(
                file[position..position + 2].try_into().unwrap()
            ),
            crate::tag::CHUNK
        );
        let length = u32::from_le_bytes(
            file[position + 4..position + 8].try_into().unwrap(),
        ) as usize;
        let end = position + RECORD_HEADER_LEN + length;
        let body = &file[position + RECORD_HEADER_LEN..end];
        let decoded_length =
            u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
        let decoded =
            zstd::bulk::decompress(&body[4..], decoded_length).unwrap();
        let mut offset = 0;
        while offset < decoded.len() {
            let tag = u16::from_le_bytes(
                decoded[offset..offset + 2].try_into().unwrap(),
            );
            let length = u32::from_le_bytes(
                decoded[offset + 4..offset + 8].try_into().unwrap(),
            ) as usize;
            offset += RECORD_HEADER_LEN + length;
            match tag {
                crate::tag::STATE | crate::tag::STATE_WITH_PAYLOAD => {
                    result.push(end as u64)
                }
                crate::tag::STATE_PAYLOAD => (),
                _ => panic!("unexpected chunk record tag {tag}"),
            }
        }
        position = end;
    }
    result
}

fn check_states(
    read: &[State<Metadata>],
    written: &[Written],
    without_payloads: bool,
) {
    for (index, (read, written)) in read.iter().zip(written).enumerate() {
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
        assert_eq!(
            read.payload_location.is_some(),
            written.payload.is_some(),
            "state {index}"
        );
    }
}

/// A finished trace reads back as the states written, with or without
/// their payloads, and each payload reads back at its offset.
#[hegel::test]
fn reads_back_what_was_written(tc: TestCase) {
    let states = tc.draw(trace_states().print_as_debug());
    let driver = tc.draw(
        sampled_from(&[TraceDriver::Terminal, TraceDriver::Browser])
            .print_as_debug(),
    );
    let directory = tempfile::tempdir().unwrap();
    write_trace(directory.path(), driver, &states);
    let path = directory.path().join(TRACE_FILE_NAME);

    let file = File::open(&path).unwrap();
    for without_payloads in [false, true] {
        let mut reader = open_trace::<Metadata>(&path).unwrap();
        if without_payloads {
            reader = reader.without_payloads();
        }
        assert_eq!(reader.driver, driver);
        let read = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
        assert!(reader.finished());
        assert_eq!(read.len(), states.len());
        check_states(&read, &states, without_payloads);
        for (state, written) in read.iter().zip(&states) {
            if let Some(payload) = &written.payload {
                let location = state.payload_location.unwrap();
                assert_eq!(
                    &Payload(read_payload_at(&path, location).unwrap()),
                    payload
                );
                assert_eq!(
                    &Payload(read_payload_from(&file, location).unwrap()),
                    payload
                );
            }
        }
    }
}

/// A trace cut anywhere after its envelope, as by a run killed while
/// writing, reads without an error as the states complete before the
/// cut, and is finished only if it wasn't cut. Cuts favour record and
/// payload chunk boundaries and the bytes around them.
#[hegel::test]
fn reads_the_complete_states_of_a_cut_trace(tc: TestCase) {
    let states = tc.draw(trace_states().print_as_debug());
    let directory = tempfile::tempdir().unwrap();
    write_trace(directory.path(), TraceDriver::Terminal, &states);
    let file = std::fs::read(directory.path().join(TRACE_FILE_NAME)).unwrap();
    let layout = layout(&file);
    let mut boundaries = vec![ENVELOPE_LEN];
    let mut start = ENVELOPE_LEN;
    while start < file.len() {
        let length =
            u32::from_le_bytes(file[start + 4..start + 8].try_into().unwrap())
                as usize;
        let end = start + RECORD_HEADER_LEN + length;
        boundaries.extend([start, start + RECORD_HEADER_LEN, end]);
        start = end;
    }
    boundaries.push(file.len());
    let boundary = || tc.draw(sampled_from(&boundaries));
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
    let complete = layout.iter().take_while(|&&end| end <= cut as u64).count();

    for without_payloads in [false, true] {
        let mut reader = StateReader::<_, Metadata>::new(&file[..cut]).unwrap();
        if without_payloads {
            reader = reader.without_payloads();
        }
        let read = reader.by_ref().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(reader.finished(), cut == file.len());
        assert_eq!(read.len(), complete);
        check_states(&read, &states, without_payloads);
    }
}
