use std::{
    fs::File,
    io::{BufReader, Read},
    marker::PhantomData,
    path::Path,
};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;

use crate::{
    RawRecord, RecordReader, TRACE_FILE_NAME, TraceDriver, read_envelope, tag,
};

/// One state read back from a trace.
#[derive(Debug)]
pub struct State<M> {
    /// Position of the state in the trace, from 0.
    pub index: usize,
    pub metadata: M,
    /// The state's driver-defined bytes, if it has any.
    pub payload: Option<Vec<u8>>,
    /// Byte offset of the payload in the trace file, for reading it again
    /// later without holding it in memory.
    pub payload_offset: Option<u64>,
}

/// Reads the states of a trace, in the order they were written, with
/// their metadata deserialized as `M`.
pub struct StateReader<R, M> {
    pub driver: TraceDriver,
    records: RecordReader<R>,
    states_read: usize,
    // A state waiting to see whether a payload follows it.
    pending: Option<M>,
    failed: bool,
    _metadata: PhantomData<M>,
}

/// Opens a trace file, or the trace in a run directory.
pub fn open_trace<M: DeserializeOwned>(
    path: &Path,
) -> Result<StateReader<BufReader<File>, M>> {
    let path = if path.is_dir() {
        path.join(TRACE_FILE_NAME)
    } else {
        path.to_path_buf()
    };
    let file = File::open(&path).with_context(|| {
        format!("failed to open trace file {}", path.display())
    })?;
    StateReader::new(BufReader::new(file))
        .with_context(|| format!("failed to read {}", path.display()))
}

impl<R: Read, M: DeserializeOwned> StateReader<R, M> {
    /// Reads the envelope from `reader`; states are read on iteration.
    pub fn new(mut reader: R) -> Result<Self> {
        let driver = read_envelope(&mut reader)?;
        Ok(StateReader {
            driver,
            records: RecordReader::new(reader),
            states_read: 0,
            pending: None,
            failed: false,
            _metadata: PhantomData,
        })
    }

    /// Fails unless the trace was written by `driver`.
    pub fn expect_driver(self, driver: TraceDriver) -> Result<Self> {
        if self.driver != driver {
            bail!(
                "not a {driver} trace (written by the {} driver)",
                self.driver
            );
        }
        Ok(self)
    }

    fn state(&mut self, metadata: M, payload: Option<RawRecord>) -> State<M> {
        let index = self.states_read;
        self.states_read += 1;
        let (payload_offset, payload) = match payload {
            Some(record) => (Some(record.payload_offset), Some(record.payload)),
            None => (None, None),
        };
        State {
            index,
            metadata,
            payload,
            payload_offset,
        }
    }

    fn read_state(&mut self) -> Result<Option<State<M>>> {
        loop {
            let Some(record) = self.records.next().transpose()? else {
                return Ok(self
                    .pending
                    .take()
                    .map(|metadata| self.state(metadata, None)));
            };
            match record.tag {
                tag::STATE => {
                    let metadata = serde_json::from_slice(&record.payload)
                        .with_context(|| {
                            format!(
                                "invalid state in trace record {}",
                                record.index
                            )
                        })?;
                    if let Some(previous) = self.pending.replace(metadata) {
                        return Ok(Some(self.state(previous, None)));
                    }
                }
                tag::STATE_PAYLOAD => {
                    let Some(metadata) = self.pending.take() else {
                        bail!(
                            "state payload without a state in trace record {}",
                            record.index
                        );
                    };
                    return Ok(Some(self.state(metadata, Some(record))));
                }
                tag if tag & tag::OPTIONAL != 0 => {}
                tag => bail!(
                    "unsupported tag {tag} in trace record {}",
                    record.index
                ),
            }
        }
    }
}

impl<R: Read, M: DeserializeOwned> Iterator for StateReader<R, M> {
    type Item = Result<State<M>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        let result = self.read_state().transpose()?;
        if result.is_err() {
            self.failed = true;
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{write_envelope, write_record};
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Metadata {
        n: u32,
    }

    fn trace(records: &[(u16, &[u8])]) -> Vec<u8> {
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Terminal).unwrap();
        for (tag, payload) in records {
            write_record(&mut file, *tag, &[payload]).unwrap();
        }
        file
    }

    fn states(file: &[u8]) -> Result<Vec<State<Metadata>>> {
        StateReader::new(file)?.collect()
    }

    #[test]
    fn pairs_states_with_their_payloads() {
        let file = trace(&[
            (tag::STATE, br#"{"n":0}"#),
            (tag::STATE_PAYLOAD, b"zero"),
            (tag::STATE, br#"{"n":1}"#),
            (tag::STATE, br#"{"n":2}"#),
            (tag::STATE_PAYLOAD, b"two"),
        ]);
        let states = states(&file).unwrap();
        assert_eq!(states.len(), 3);
        assert_eq!(states[0].metadata, Metadata { n: 0 });
        assert_eq!(states[0].payload.as_deref(), Some(&b"zero"[..]));
        assert_eq!(states[1].payload, None);
        assert_eq!(states[2].index, 2);
        assert_eq!(states[2].payload.as_deref(), Some(&b"two"[..]));

        let offset = states[2].payload_offset.unwrap() as usize;
        assert_eq!(&file[offset..offset + 3], b"two");
    }

    #[test]
    fn skips_unknown_optional_records() {
        let file = trace(&[
            (tag::OPTIONAL | 7, b"ignored"),
            (tag::STATE, br#"{"n":0}"#),
            (tag::OPTIONAL | 7, b"ignored"),
            (tag::STATE_PAYLOAD, b"zero"),
        ]);
        let states = states(&file).unwrap();
        assert_eq!(states.len(), 1);
        assert!(states[0].payload.is_some());
    }

    #[test]
    fn rejects_unknown_required_and_misplaced_records() {
        let error = states(&trace(&[(7, b"unknown")])).unwrap_err();
        assert!(error.to_string().contains("unsupported tag 7"), "{error}");

        let error = states(&trace(&[(tag::STATE_PAYLOAD, b"x")])).unwrap_err();
        assert!(
            error.to_string().contains("state payload without a state"),
            "{error}"
        );

        let error = states(&trace(&[(tag::STATE, b"not json")])).unwrap_err();
        assert!(error.to_string().contains("invalid state"), "{error}");
    }

    #[test]
    fn checks_the_driver() {
        let file = trace(&[]);
        let error = StateReader::<_, Metadata>::new(file.as_slice())
            .unwrap()
            .expect_driver(TraceDriver::Browser)
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "not a browser trace (written by the terminal driver)"
        );
    }
}
