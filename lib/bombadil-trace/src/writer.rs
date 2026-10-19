use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::Path,
    sync::{Arc, mpsc},
    thread::JoinHandle,
};

use anyhow::{Result, anyhow, bail};
use serde::Serialize;

use crate::{TRACE_FILE_NAME, TraceDriver, tag, write_envelope, write_record};

// Bounds the number of in-flight states to cap memory while letting the
// writer thread run behind the test loop.
const PENDING_STATES_MAX: usize = 32;

/// Writes the states of a trace on a dedicated thread, so that encoding
/// and disk I/O overlap with the test loop instead of stalling it. The
/// bounded channel provides backpressure if the writer can't keep up.
pub struct TraceFileWriter<M> {
    sender: mpsc::SyncSender<Message<M>>,
    worker: Option<JoinHandle<Result<()>>>,
}

enum Message<M> {
    State(Box<PendingState<M>>),
    Flush(mpsc::SyncSender<Result<()>>),
}

struct PendingState<M> {
    metadata: M,
    // Shared rather than copied, so it reaches the file without another
    // copy of its bytes.
    payload: Option<Arc<Vec<u8>>>,
}

impl<M: Serialize + Send + 'static> TraceFileWriter<M> {
    /// Creates `trace.bin` in `run_path`, written by `driver`.
    pub fn create(
        run_path: &Path,
        driver: TraceDriver,
        overwrite: bool,
    ) -> Result<Self> {
        std::fs::create_dir_all(run_path)?;
        let trace_path = run_path.join(TRACE_FILE_NAME);
        if trace_path.try_exists()? {
            if !overwrite {
                bail!(
                    "{TRACE_FILE_NAME} already exists at {}. \
                     Use --output-path-overwrite to overwrite, or choose a different --output-path.",
                    trace_path.display(),
                );
            }
            std::fs::remove_file(&trace_path)?;
        }
        let mut trace_file = BufWriter::new(
            File::options()
                .write(true)
                .create_new(true)
                .open(&trace_path)?,
        );
        write_envelope(&mut trace_file, driver).map_err(trace_write_error)?;
        log::info!("storing run trace in {}", run_path.display());
        let (sender, receiver) = mpsc::sync_channel(PENDING_STATES_MAX);
        let worker = std::thread::Builder::new()
            .name("bombadil-trace-writer".to_string())
            .spawn(move || worker_loop(receiver, trace_file))?;
        Ok(TraceFileWriter {
            sender,
            worker: Some(worker),
        })
    }

    /// Queues a state, with its driver-defined payload if it has one.
    pub fn write_state(
        &mut self,
        metadata: M,
        payload: Option<Arc<Vec<u8>>>,
    ) -> Result<()> {
        let state = Box::new(PendingState { metadata, payload });
        if self.sender.send(Message::State(state)).is_err() {
            return Err(self.worker_failure());
        }
        Ok(())
    }
}

impl<M> TraceFileWriter<M> {
    /// Waits until every queued state is written to the file.
    pub fn flush(&mut self) -> Result<()> {
        let (ack_sender, ack_receiver) = mpsc::sync_channel(1);
        if self.sender.send(Message::Flush(ack_sender)).is_err() {
            return Err(self.worker_failure());
        }
        match ack_receiver.recv() {
            Ok(result) => result,
            Err(_) => Err(self.worker_failure()),
        }
    }

    /// Joins the worker thread to surface the error that made it exit.
    fn worker_failure(&mut self) -> anyhow::Error {
        match self.worker.take() {
            Some(worker) => match worker.join() {
                Ok(Ok(())) => {
                    anyhow!("trace writer thread exited unexpectedly")
                }
                Ok(Err(error)) => error,
                Err(_) => anyhow!("trace writer thread panicked"),
            },
            None => anyhow!("trace writer thread already failed"),
        }
    }
}

impl<M> Drop for TraceFileWriter<M> {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

fn worker_loop<M: Serialize>(
    receiver: mpsc::Receiver<Message<M>>,
    mut trace_file: BufWriter<File>,
) -> Result<()> {
    let mut metadata = Vec::new();
    while let Ok(message) = receiver.recv() {
        match message {
            Message::State(state) => {
                metadata.clear();
                serde_json::to_writer(&mut metadata, &state.metadata)?;
                write_record(&mut trace_file, tag::STATE, &[&metadata])
                    .and_then(|()| match &state.payload {
                        Some(payload) => write_record(
                            &mut trace_file,
                            tag::STATE_PAYLOAD,
                            &[payload],
                        ),
                        None => Ok(()),
                    })
                    .map_err(trace_write_error)?;
            }
            Message::Flush(ack) => {
                let result = trace_file.flush().map_err(trace_write_error);
                // The flusher may have given up waiting; ignore that.
                let _ = ack.send(result);
            }
        }
    }
    trace_file.flush().map_err(trace_write_error)?;
    Ok(())
}

fn trace_write_error(error: io::Error) -> anyhow::Error {
    let message = match error.kind() {
        io::ErrorKind::QuotaExceeded => {
            "try using `--output-path` to write traces outside \
            of the temporary directory"
        }
        _ => "failed to write to trace file",
    };
    anyhow!(error).context(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{State, open_trace};
    use serde::Deserialize;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Metadata {
        n: u32,
    }

    #[test]
    fn writes_states_that_read_back() {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = TraceFileWriter::create(
            directory.path(),
            TraceDriver::Terminal,
            false,
        )
        .unwrap();
        writer
            .write_state(Metadata { n: 0 }, Some(Arc::new(b"zero".to_vec())))
            .unwrap();
        writer.write_state(Metadata { n: 1 }, None).unwrap();
        drop(writer);

        let reader = open_trace::<Metadata>(directory.path()).unwrap();
        assert_eq!(reader.driver, TraceDriver::Terminal);
        let states = reader.collect::<Result<Vec<State<Metadata>>>>().unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].metadata, Metadata { n: 0 });
        assert_eq!(states[0].payload.as_deref(), Some(&b"zero"[..]));
        assert_eq!(states[1].payload, None);
    }

    #[test]
    fn refuses_to_overwrite_unless_asked() {
        let directory = tempfile::tempdir().unwrap();
        let create = |overwrite| {
            TraceFileWriter::<Metadata>::create(
                directory.path(),
                TraceDriver::Terminal,
                overwrite,
            )
        };
        drop(create(false).unwrap());
        assert!(create(false).is_err());
        assert!(create(true).is_ok());
    }
}
