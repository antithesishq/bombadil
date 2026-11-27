//! Properties of reading back what [`TraceFileWriter`] wrote, whole or
//! cut short.

use std::{
    fs::File,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use hegel::{
    Generator, TestCase,
    generators::{
        binary, booleans, integers, one_of, optional, sampled_from, text, vecs,
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    ENVELOPE_LEN, RECORD_HEADER_LEN, Record, State, StepError, TRACE_FILE_NAME,
    TraceDriver, TraceFileWriter, TraceReader,
    chunk::ChunkWriter,
    read_payload_at, read_payload_from,
    reader::{StateReader, open_states},
    record::PAYLOAD_READ_CHUNK,
    tag, write_envelope, write_record,
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
        let mut reader = open_states::<Metadata>(&path).unwrap();
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Action {
    action: String,
    action_taken: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StepMetadata {
    number: u64,
    action: Option<String>,
    state: StepState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StepState {
    action_taken: Option<String>,
}

/// A run as written: `actions[i]` was applied in `states[i]`.
#[derive(Debug, Default)]
struct Run {
    states: Vec<(StepMetadata, Option<Payload>)>,
    actions: Vec<Action>,
    error: Option<StepError>,
}

#[derive(Debug, PartialEq)]
enum Step {
    State {
        index: usize,
        action: Option<Action>,
        metadata: StepMetadata,
        payload: Option<Payload>,
    },
    Error {
        index: usize,
        actions: Vec<Action>,
        error: StepError,
    },
}

impl Run {
    fn steps(&self, states: usize, actions: usize, error: bool) -> Vec<Step> {
        let mut steps: Vec<Step> = self.states[..states]
            .iter()
            .enumerate()
            .map(|(index, (metadata, payload))| Step::State {
                index,
                action: index.checked_sub(1).map(|i| self.actions[i].clone()),
                metadata: metadata.clone(),
                payload: payload.clone(),
            })
            .collect();
        let first = states.saturating_sub(1);
        if first < actions {
            let written = self.error.clone().filter(|_| error);
            steps.push(Step::Error {
                index: states,
                actions: self.actions[first..actions].to_vec(),
                error: written.unwrap_or(StepError::Unknown),
            });
        }
        steps
    }
}

fn read_steps(file: &[u8]) -> (Vec<Step>, bool) {
    let mut reader = TraceReader::<_, StepMetadata, Action>::new(file).unwrap();
    let steps = reader
        .by_ref()
        .map(|record| match record.unwrap() {
            Record::State { action, state } => Step::State {
                index: state.index,
                action,
                metadata: state.metadata,
                payload: state.payload.map(Payload),
            },
            Record::Error {
                index,
                actions,
                error,
            } => Step::Error {
                index,
                actions,
                error,
            },
        })
        .collect();
    (steps, reader.finished())
}

#[derive(Debug, Default)]
struct Layout {
    states: Vec<usize>,
    actions: Vec<usize>,
    error: Option<usize>,
    records: Vec<usize>,
}

impl Layout {
    fn of(file: &[u8]) -> Self {
        let field = |bytes: &[u8], at: usize, length: usize| {
            let mut value = [0; 8];
            value[..length].copy_from_slice(&bytes[at..at + length]);
            u64::from_le_bytes(value) as usize
        };
        let mut layout = Layout::default();
        let mut start = ENVELOPE_LEN;
        while start < file.len() {
            let end = start + RECORD_HEADER_LEN + field(file, start + 4, 4);
            layout.records.push(start);
            match field(file, start, 2) as u16 {
                tag::ACTION => layout.actions.push(end),
                tag::ERROR => layout.error = Some(end),
                tag::CHUNK => {
                    let body = &file[start + RECORD_HEADER_LEN..end];
                    let decoded =
                        zstd::bulk::decompress(&body[4..], field(body, 0, 4))
                            .unwrap();
                    let mut offset = 0;
                    while offset < decoded.len() {
                        let tag = field(&decoded, offset, 2) as u16;
                        if matches!(tag, tag::STATE | tag::STATE_WITH_PAYLOAD) {
                            layout.states.push(end);
                        }
                        offset +=
                            RECORD_HEADER_LEN + field(&decoded, offset + 4, 4);
                    }
                }
                tag::END => {}
                tag => panic!("unexpected record tag {tag}"),
            }
            start = end;
        }
        layout
    }

    fn held(ends: &[usize], cut: usize) -> usize {
        ends.iter().take_while(|&&end| end <= cut).count()
    }
}

#[hegel::composite]
fn step_error(tc: &TestCase) -> StepError {
    let message = || tc.draw(text().max_size(32));
    match tc.draw(integers::<u8>().max_value(5)) {
        0 => StepError::Stalled,
        1 => StepError::Interrupted,
        2 => StepError::TimeLimit,
        3 => StepError::Driver { message: message() },
        4 => StepError::Failed { message: message() },
        _ => StepError::Other(
            serde_json::value::to_raw_value(&serde_json::json!({
                "kind": "from_the_future",
                "detail": message(),
                "code": tc.draw(integers::<u64>()),
            }))
            .unwrap(),
        ),
    }
}

#[hegel::composite]
fn run_length(tc: &TestCase) -> usize {
    tc.draw(one_of([
        integers::<usize>().min_value(1).max_value(70).boxed(),
        sampled_from(&[1usize, 2, 31, 32, 33, 34, 64, 65, 66]).boxed(),
    ]))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Ending {
    Finished,
    Dropped,
    Killed,
}

/// A trace reads back as the steps of its run, and cut anywhere as
/// the states it holds whole, then one error with every action after
/// them. The chunk writer is driven with a drawn clock and drawn
/// flushes, so the layout of the file follows from the draws.
#[hegel::test]
fn reads_back_the_steps_of_a_run(tc: TestCase) {
    let driver = tc.draw(
        sampled_from(&[TraceDriver::Terminal, TraceDriver::Browser])
            .print_as_debug(),
    );
    let action_records = tc.draw(booleans());
    let actions_in_states = !action_records || tc.draw(booleans());
    let length = tc.draw(run_length());
    let ends_with_action = action_records && tc.draw(booleans());

    let mut file = Vec::new();
    write_envelope(&mut file, driver).unwrap();
    let mut writer =
        ChunkWriter::new(file, std::sync::mpsc::channel::<()>().1).unwrap();
    let mut now = Instant::now();
    let mut run = Run::default();
    while run.states.len() < length {
        let action = run.actions.last().filter(|_| actions_in_states);
        let metadata = StepMetadata {
            number: tc.draw(integers()),
            action: action.map(|action| action.action.clone()),
            state: StepState {
                action_taken: action
                    .and_then(|action| action.action_taken.clone()),
            },
        };
        let payload = tc.draw(optional(one_of([
            binary().max_size(64).map(Payload).boxed(),
            patterned_payload(0, 4096).boxed(),
            patterned_payload(PAYLOAD_READ_CHUNK, 150 * 1024).boxed(),
        ])));
        writer
            .write_state(
                &serde_json::to_vec(&metadata).unwrap(),
                payload.as_ref().map(|payload| &payload.0[..]),
                now,
            )
            .unwrap();
        run.states.push((metadata, payload));
        if run.states.len() == length && !ends_with_action {
            break;
        }
        // The writer thread may flush before the action is appended.
        if tc.draw(booleans()) {
            writer.flush().unwrap();
        }
        let action = Action {
            action: tc.draw(text().max_size(64)),
            action_taken: tc.draw(optional(text().max_size(16))),
        };
        if action_records {
            let state = run.states.len() as u64 - 1;
            write_record(
                writer.writer_mut(),
                tag::ACTION,
                &[&state.to_le_bytes(), &serde_json::to_vec(&action).unwrap()],
            )
            .unwrap();
        }
        run.actions.push(action);
        now += tc.draw(sampled_from(&[
            Duration::ZERO,
            Duration::from_millis(1),
            Duration::from_millis(100),
        ]));
    }
    if ends_with_action && tc.draw(booleans()) {
        let error = tc.draw(step_error().print_as_debug());
        let state = run.states.len() as u64 - 1;
        writer
            .write_error(state, &serde_json::to_vec(&error).unwrap())
            .unwrap();
        run.error = Some(error);
    }
    let ending = tc.draw(
        sampled_from(&[Ending::Finished, Ending::Dropped, Ending::Killed])
            .print_as_debug(),
    );
    match ending {
        Ending::Finished => writer.finish().unwrap(),
        Ending::Dropped => writer.flush().unwrap(),
        Ending::Killed => {}
    }
    let file = std::mem::take(writer.writer_mut());

    let layout = Layout::of(&file);
    if ending != Ending::Killed {
        assert_eq!(layout.states.len(), run.states.len());
    }
    // At and around the bounds of every record, and anywhere.
    let mut cuts = vec![file.len()];
    for &record in &layout.records {
        let around = tc.draw(integers::<usize>().max_value(40));
        cuts.push(record);
        cuts.push((record + around).min(file.len()));
    }
    for _ in 0..4 {
        cuts.push(
            tc.draw(
                integers::<usize>()
                    .min_value(ENVELOPE_LEN)
                    .max_value(file.len()),
            ),
        );
    }
    for cut in cuts {
        let expected = run.steps(
            Layout::held(&layout.states, cut),
            Layout::held(&layout.actions, cut),
            layout.error.is_some_and(|end| end <= cut),
        );
        let errors = expected
            .iter()
            .filter(|step| matches!(step, Step::Error { .. }))
            .count();
        assert!(errors <= 1);
        assert_eq!(
            read_steps(&file[..cut]),
            (expected, ending == Ending::Finished && cut == file.len()),
            "cut at {cut} of {}",
            file.len()
        );
    }
}

fn step_metadata(number: u64) -> StepMetadata {
    StepMetadata {
        number,
        action: None,
        state: StepState { action_taken: None },
    }
}

/// An action is in the file once `write_action` returns.
#[test]
fn appends_actions_before_their_states_are_written() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(TRACE_FILE_NAME);
    for overwrite in [false, true] {
        let mut writer =
            TraceFileWriter::create(&path, TraceDriver::Browser, overwrite)
                .unwrap();
        let mut run = Run::default();
        for number in 0..40 {
            writer.write_state(step_metadata(number), None).unwrap();
            run.states.push((step_metadata(number), None));
            let action = Action {
                action: format!("action {number}"),
                action_taken: None,
            };
            writer.write_action(&action).unwrap();
            run.actions.push(action);
            // As a process killed now would leave the file.
            let file = std::fs::read(&path).unwrap();
            let (steps, _) = read_steps(&file);
            let Some(Step::Error { actions, error, .. }) = steps.last() else {
                panic!("no error last in {steps:?}");
            };
            assert_eq!(actions.last(), run.actions.last());
            assert_eq!(*error, StepError::Unknown);
        }
        writer.write_error(StepError::Stalled).unwrap();
        run.error = Some(StepError::Stalled);
        writer.finish().unwrap();
        let file = std::fs::read(&path).unwrap();
        assert_eq!(read_steps(&file), (run.steps(40, 40, true), true));

        // A torn last action record isn't an action.
        let layout = Layout::of(&file);
        let last = layout.actions[39];
        for cut in last - 30..last {
            let (steps, _) = read_steps(&file[..cut]);
            let Some(Step::Error { actions, .. }) = steps.last() else {
                panic!("no error last in {steps:?}");
            };
            assert_eq!(actions.last(), Some(&run.actions[38]), "cut {cut}");
        }
    }
}

/// The writer refuses calls out of order, and cuts long messages.
#[test]
fn refuses_actions_and_errors_out_of_place() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(TRACE_FILE_NAME);
    let mut writer =
        TraceFileWriter::create(&path, TraceDriver::Browser, false).unwrap();
    let refused = |result: Result<()>, message: &str| {
        let error = result.unwrap_err().to_string();
        assert!(error.contains(message), "{error}");
    };
    let other = serde_json::value::to_raw_value(&1).unwrap();
    refused(
        writer.write_action(&"action"),
        "no state was written before",
    );
    refused(writer.write_error(StepError::Stalled), "no action");
    writer.write_state(step_metadata(0), None).unwrap();
    refused(writer.write_error(StepError::Stalled), "no action");
    writer.write_action(&"first").unwrap();
    refused(writer.write_action(&"second"), "since the last one");
    refused(writer.write_error(StepError::Unknown), "for readers");
    refused(writer.write_error(StepError::Other(other)), "for readers");
    writer.write_state(step_metadata(1), None).unwrap();
    writer.write_action(&"last").unwrap();
    let message = "é".repeat(4000);
    writer.write_error(StepError::Failed { message }).unwrap();
    refused(writer.write_error(StepError::Stalled), "no action");
    refused(writer.write_action(&"after"), "since the last one");
    refused(
        writer.write_state(step_metadata(2), None),
        "ended with an error",
    );
    writer.finish().unwrap();

    let records = TraceReader::<_, StepMetadata, String>::new(
        &std::fs::read(&path).unwrap()[..],
    )
    .unwrap()
    .collect::<Result<Vec<_>>>()
    .unwrap();
    let [
        Record::State { action: None, .. },
        Record::State {
            action: Some(first),
            ..
        },
        Record::Error {
            index: 2,
            actions,
            error: StepError::Failed { message },
        },
    ] = &records[..]
    else {
        panic!("unexpected records: {records:?}");
    };
    assert_eq!(
        (first.as_str(), &actions[..]),
        ("first", &["last".to_string()][..])
    );
    assert!(
        message.len() <= 4096 + 3 && message.ends_with('…'),
        "{message}"
    );
}

/// An error kind this version doesn't know is kept as written.
#[test]
fn keeps_error_kinds_it_doesnt_know() {
    let written =
        r#"{"kind":"target_crashed","status":"crashed","error_code":5.0}"#;
    let error: StepError = serde_json::from_str(written).unwrap();
    assert!(matches!(error, StepError::Other(_)));
    assert_eq!(serde_json::to_string(&error).unwrap(), written);
    assert_eq!(error.to_string(), written);
    for known in [
        StepError::Unknown,
        StepError::Stalled,
        StepError::Interrupted,
        StepError::TimeLimit,
        StepError::Driver {
            message: "d".into(),
        },
        StepError::Failed {
            message: "f".into(),
        },
    ] {
        let written = serde_json::to_string(&known).unwrap();
        assert_eq!(serde_json::from_str::<StepError>(&written).unwrap(), known);
    }
    assert!(serde_json::from_str::<StepError>(r#"{"kind":"driver"}"#).is_err());
}
