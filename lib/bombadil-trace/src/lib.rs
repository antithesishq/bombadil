//! Bombadil's binary trace format (`trace.bin`).
//!
//! A trace is an envelope followed by records, back to back, in the style
//! of ghostty's snapshot format. Every integer is unsigned and
//! little-endian. The file is append-only: each record carries its own
//! length and checksum, so nothing is rewritten after it is written.
//!
//! # Envelope
//!
//! Exactly one, at byte zero. It identifies the file as a bombadil trace,
//! selects the one format version that governs it, and names the driver
//! that wrote it.
//!
//! | Offset | Size | Field                  |
//! | -----: | ---: | :--------------------- |
//! |      0 |    8 | Magic (`BMBDTRC\0`)    |
//! |      8 |    2 | Format version (`u16`) |
//! |     10 |    2 | Driver (`u16`)         |
//! |     12 |    4 | CRC-32 (`u32`)         |
//!
//! The CRC-32 (IEEE) covers the first 12 bytes. Driver codes are assigned
//! in [`TraceDriver`] and never reused.
//!
//! # Records
//!
//! From offset 16 to the end of the file. Offsets are relative to the
//! start of the record.
//!
//! | Offset | Size          | Field                     |
//! | -----: | ------------: | :------------------------ |
//! |      0 |             2 | Tag (`u16`)               |
//! |      2 |             4 | Payload length (`u32`)    |
//! |      6 |             4 | Header CRC-32 (`u32`)     |
//! |     10 |             4 | Payload CRC-32 (`u32`)    |
//! |     14 | `payload_len` | Payload                   |
//!
//! Both are CRC-32 (IEEE). The header CRC covers the tag and the payload
//! length, so a corrupted length is detected before it is trusted; the
//! payload CRC covers the payload. A corrupted record is reported instead
//! of misread. The payload length bounds the record. A record's payload
//! is at most 256 MiB; readers reject larger lengths. Tags are assigned in
//! [`tag`]; a tag with [`tag::OPTIONAL`] set may be skipped by readers
//! that don't know it, and any other unknown tag is rejected.
//!
//! A trace is a sequence of states. Each state's JSON metadata is held by
//! a [`tag::STATE`] record if the state has no payload, or by a
//! [`tag::STATE_WITH_PAYLOAD`] record if it does, followed by a
//! [`tag::STATE_PAYLOAD`] record holding driver-defined bytes for it
//! (e.g. a terminal snapshot or a screenshot). Optional records may come
//! in between. The tag says whether a payload follows, so a lost payload
//! can't pass for a state without one.
//!
//! A run that is killed while writing leaves an incomplete last record:
//! a partial header, or an intact header whose payload the file ends
//! before. Readers end the trace there, with a warning, instead of
//! failing. Since the header is checked first, a corrupted length can't
//! pass for such a cut. The same goes for a trace that ends after a
//! [`tag::STATE_WITH_PAYLOAD`] record but before its payload: that state
//! is incomplete and dropped with a warning. A state record where a
//! payload is expected is an error.
//!
//! A finished trace ends with a [`tag::END`] record, with an empty
//! payload, at a state boundary (not while a payload is expected).
//! Without it, a trace cut exactly between two states, which is the most
//! likely result of killing a run since the writer flushes whenever it is
//! idle, would look finished. Readers warn when a trace has no END, and
//! reject an END while a payload is expected, an END with a payload, and
//! anything after an END. END is required, not optional: a reader that
//! skipped it would also accept records after it, and would take a cut
//! trace for a finished one without knowing.
//!
//! # Creating a trace
//!
//! Unless asked to overwrite, the writer creates `trace.bin` only if no
//! file has that name, so of two runs writing to the same directory one
//! fails instead of replacing the other's trace. It writes and flushes
//! the envelope there, and removes the file if that fails.
//!
//! To overwrite, it writes and flushes the envelope to a temporary file
//! next to `trace.bin` (`trace.bin.<pid>.<n>.tmp`, numbered within the
//! process, never reusing a name that is taken), then renames it over
//! `trace.bin` and keeps writing to the same open file. Renaming is the
//! last step that can fail, so a failure leaves the trace being
//! overwritten as it was. A run killed before that step can leave a
//! `trace.bin.*.tmp` file behind.
//!
//! Either way, a failure to create a trace leaves no new file behind,
//! though the run directory may have been created.
//!
//! END means the writer was finished explicitly: it is written only by
//! [`TraceFileWriter::finish`]. A writer dropped without it, e.g. on a
//! panic or an early return, writes every queued state and leaves the
//! trace unfinished.

mod envelope;
mod reader;
mod record;
mod writer;

pub use envelope::{TraceDriver, trace_driver, write_envelope};
pub use reader::{
    State, StateReader, open_trace, read_payload_at, read_payload_from,
};
pub use record::write_record;
pub use writer::TraceFileWriter;

/// File name of a binary trace in a run directory.
pub const TRACE_FILE_NAME: &str = "trace.bin";

/// How a writer's error starts when it repeats an earlier write failure,
/// as every call after one does.
pub const WRITER_FAILED_EARLIER: &str = "trace writer failed earlier";

const MAGIC: &[u8; 8] = b"BMBDTRC\0";
const FORMAT_VERSION: u16 = 1;

const ENVELOPE_LEN: usize = 16;
const ENVELOPE_MAGIC_OFFSET: usize = 0;
const ENVELOPE_VERSION_OFFSET: usize = 8;
const ENVELOPE_DRIVER_OFFSET: usize = 10;
const ENVELOPE_CRC_OFFSET: usize = 12;

const RECORD_HEADER_LEN: usize = 14;
const RECORD_TAG_OFFSET: usize = 0;
const RECORD_LENGTH_OFFSET: usize = 2;
const RECORD_HEADER_CRC_OFFSET: usize = 6;
const RECORD_PAYLOAD_CRC_OFFSET: usize = 10;

/// Largest payload accepted, checked before writing and before reading
/// so a corrupted length is reported rather than trusted.
pub const RECORD_PAYLOAD_BYTES_MAX: usize = 256 * 1024 * 1024;

// The fields are contiguous and in the documented order.
const _: () =
    assert!(ENVELOPE_MAGIC_OFFSET + MAGIC.len() == ENVELOPE_VERSION_OFFSET);
const _: () = assert!(ENVELOPE_VERSION_OFFSET + 2 == ENVELOPE_DRIVER_OFFSET);
const _: () = assert!(ENVELOPE_DRIVER_OFFSET + 2 == ENVELOPE_CRC_OFFSET);
const _: () = assert!(ENVELOPE_CRC_OFFSET + 4 == ENVELOPE_LEN);
const _: () = assert!(RECORD_TAG_OFFSET + 2 == RECORD_LENGTH_OFFSET);
const _: () = assert!(RECORD_LENGTH_OFFSET + 4 == RECORD_HEADER_CRC_OFFSET);
const _: () =
    assert!(RECORD_HEADER_CRC_OFFSET + 4 == RECORD_PAYLOAD_CRC_OFFSET);
const _: () = assert!(RECORD_PAYLOAD_CRC_OFFSET + 4 == RECORD_HEADER_LEN);
// A payload length must fit the u32 field.
const _: () = assert!(RECORD_PAYLOAD_BYTES_MAX <= u32::MAX as usize);

/// Record tags.
pub mod tag {
    /// The JSON metadata of a state without a payload.
    pub const STATE: u16 = 1;
    /// Driver-defined bytes for the [`STATE_WITH_PAYLOAD`] record before
    /// it.
    pub const STATE_PAYLOAD: u16 = 2;
    /// The JSON metadata of a state whose payload follows.
    pub const STATE_WITH_PAYLOAD: u16 = 3;
    // 4 is reserved for payload references.
    /// Marks the trace as finished. Empty, and last in the file.
    pub const END: u16 = 5;
    /// Set on tags that readers which don't know them may skip.
    pub const OPTIONAL: u16 = 0x8000;
}

/// Captures warnings and errors, which tests running at the same time
/// may also log.
#[cfg(test)]
mod test_log {
    struct WarningLog;

    static WARNINGS: std::sync::Mutex<Vec<String>> =
        std::sync::Mutex::new(Vec::new());

    impl log::Log for WarningLog {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::Level::Warn
        }

        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata()) {
                WARNINGS.lock().unwrap().push(record.args().to_string());
            }
        }

        fn flush(&self) {}
    }

    pub(crate) fn capture_warnings() {
        let _ = log::set_logger(&WarningLog);
        log::set_max_level(log::LevelFilter::Warn);
    }

    /// Whether a warning or error containing `message` was logged.
    pub(crate) fn warned(message: &str) -> bool {
        WARNINGS
            .lock()
            .unwrap()
            .iter()
            .any(|warning| warning.contains(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact bytes of a small trace, so a layout change can't slip in
    /// without a format version bump. The CRCs were computed with an
    /// independent CRC-32 implementation (`gzip`).
    #[test]
    fn golden_bytes() {
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Terminal).unwrap();
        write_record(&mut file, tag::STATE_WITH_PAYLOAD, &[b"{}"]).unwrap();
        write_record(&mut file, tag::STATE_PAYLOAD, &[b"ab"]).unwrap();
        write_record(&mut file, tag::END, &[]).unwrap();
        assert_eq!(
            file,
            [
                // Envelope: magic, version 1, driver 1, CRC.
                b'B', b'M', b'B', b'D', b'T', b'R', b'C', 0, 1, 0, 1, 0, 0x3a,
                0x15, 0x2d, 0xee,
                // State with payload record: tag 3, length 2, header CRC,
                // payload CRC, "{}".
                3, 0, 2, 0, 0, 0, 0x86, 0x1b, 0x5f, 0x9d, 0x43, 0xbf, 0xa6,
                0xa3, b'{', b'}',
                // State payload record: tag 2, length 2, header CRC,
                // payload CRC, "ab".
                2, 0, 2, 0, 0, 0, 0x23, 0xc8, 0x03, 0x56, 0x6d, 0x48, 0x83,
                0x9e, b'a', b'b',
                // End record: tag 5, length 0, header CRC, payload CRC
                // (of no bytes).
                5, 0, 0, 0, 0, 0, 0x10, 0x30, 0x0f, 0xe1, 0, 0, 0, 0,
            ]
        );
    }
}
