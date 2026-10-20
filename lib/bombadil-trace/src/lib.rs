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
//! A state's payload is often identical to an earlier state's (the
//! screen didn't change, or the browser went back to a page). The writer
//! may then write a [`tag::STATE_PAYLOAD_REF`] record in place of the
//! [`tag::STATE_PAYLOAD`] record, naming the earlier state whose payload
//! the state shares instead of holding the bytes again:
//!
//! | Offset | Size | Field                  |
//! | -----: | ---: | :--------------------- |
//! |      0 |    4 | State index (`u32`)    |
//! |      4 |    4 | Payload length (`u32`) |
//! |      8 |    4 | Payload CRC-32 (`u32`) |
//!
//! As in any record, the record's own payload CRC covers these 12 bytes.
//! States are numbered from 0 in the order they are written. The state
//! named is an earlier one whose payload is in a [`tag::STATE_PAYLOAD`]
//! record, not another reference, so references only point backwards,
//! one step, and a cut trace never holds one that can't be resolved. The
//! length and CRC-32 are the shared payload's, as written in the header
//! of the record that holds it, so readers check that a reference
//! resolves to the payload it was written for, and that the bytes they
//! read for it are those. A reference to a state that isn't earlier, has
//! no payload, has a reference itself, or whose payload doesn't match, is
//! a corrupt record. Readers report the offset and length of the payload
//! in the record the reference names, so payloads are read again by
//! offset the same way.
//!
//! The writer refers only to identical payloads: it finds candidates by
//! CRC-32 and length and compares their bytes. It doesn't refer to
//! payloads of 12 bytes or fewer, which are no larger than a reference,
//! and keeps at most 4 candidates per CRC-32 and length, so a payload
//! whose CRC-32 and length many distinct earlier payloads share may be
//! written again in full.
//!
//! Much of a state's metadata is often extractor snapshot values that
//! are the same as in the state before. A state's snapshot entries are
//! those of the `snapshots` member of its metadata, if the metadata is
//! an object with one such member, an array: the entries that are
//! objects with one `index` member, an integer from 0 naming the
//! extractor, and one member that is either `value` or `value_from`,
//! whose key is written without escapes, directly followed by a colon
//! and then the value, as serde_json writes it. Anything else, such as
//! the snapshots of a violation, isn't an entry, and is read as written.
//! An entry's `"value":<value>` may be written as `"value_from":<state>`
//! instead, in the same place, where `<state>` is the index of the last
//! earlier state that holds a value of the same extractor in full, if
//! that value is the same, byte for byte:
//!
//! ```json
//! {"index":0,"name":"title","value":"A long title","time":3}
//! {"index":0,"name":"title","value_from":7,"time":4}
//! ```
//!
//! Readers put `"value":<value>` back before reading the metadata, so it
//! is read as the bytes the writer encoded. As with payloads, references
//! only point backwards, one step, and a cut trace never holds one that
//! can't be resolved. A reference that isn't a state index, or names a
//! state that isn't earlier, or isn't the last one holding a value of the
//! extractor in full, is a corrupt record. Readers keep one value per
//! extractor, as does the writer, which writes a reference only where
//! `"value_from":<state>` is shorter than `"value":<value>`. Since
//! `value_from` names a reference, a state whose metadata has a snapshot
//! entry with a `value_from` member can't be written.
//!
//! The writer checks the size of a state's metadata before writing its
//! values once, so with every value put back, metadata is at most
//! [`RECORD_PAYLOAD_BYTES_MAX`] bytes too. Readers report a state whose
//! metadata would be larger with its values put back as a corrupt
//! record, before putting them back, so references to a large value
//! can't make a small record take any amount of memory.
//!
//! A run that is killed while writing leaves an incomplete last record:
//! a partial header, or an intact header whose payload the file ends
//! before. Readers end the trace there, with a warning, instead of
//! failing. Since the header is checked first, a corrupted length can't
//! pass for such a cut. The same goes for a trace that ends after a
//! [`tag::STATE_WITH_PAYLOAD`] record but before its payload (or its
//! reference): that state is incomplete and dropped with a warning. A
//! state record where a payload is expected is an error.
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
mod snapshots;
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

const PAYLOAD_REF_LEN: usize = 12;
const PAYLOAD_REF_STATE_OFFSET: usize = 0;
const PAYLOAD_REF_LENGTH_OFFSET: usize = 4;
const PAYLOAD_REF_CRC_OFFSET: usize = 8;

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
const _: () = assert!(PAYLOAD_REF_STATE_OFFSET == 0);
const _: () =
    assert!(PAYLOAD_REF_STATE_OFFSET + 4 == PAYLOAD_REF_LENGTH_OFFSET);
const _: () = assert!(PAYLOAD_REF_LENGTH_OFFSET + 4 == PAYLOAD_REF_CRC_OFFSET);
const _: () = assert!(PAYLOAD_REF_CRC_OFFSET + 4 == PAYLOAD_REF_LEN);
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
    /// Stands for the [`STATE_PAYLOAD`] record of the [`STATE_WITH_PAYLOAD`]
    /// record before it, whose payload is the same as an earlier state's:
    /// names that state, and the payload's length and CRC-32 (see the
    /// [crate] docs).
    pub const STATE_PAYLOAD_REF: u16 = 4;
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
        write_record(&mut file, tag::STATE_WITH_PAYLOAD, &[b"{}"]).unwrap();
        let reference = record::PayloadRef {
            state: 0,
            length: 2,
            crc: crc32fast::hash(b"ab"),
        };
        write_record(&mut file, tag::STATE_PAYLOAD_REF, &[&reference.encode()])
            .unwrap();
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
                // The second state, whose payload is the first's: its state
                // with payload record, as above, then a payload reference
                // record: tag 4, length 12, header CRC, payload CRC, then
                // state 0, length 2, and the CRC of "ab".
                3, 0, 2, 0, 0, 0, 0x86, 0x1b, 0x5f, 0x9d, 0x43, 0xbf, 0xa6,
                0xa3, b'{', b'}', 4, 0, 12, 0, 0, 0, 0x0d, 0x5c, 0x85, 0x60,
                0x7f, 0x47, 0x2c, 0x89, 0, 0, 0, 0, 2, 0, 0, 0, 0x6d, 0x48,
                0x83, 0x9e,
                // End record: tag 5, length 0, header CRC, payload CRC
                // (of no bytes).
                5, 0, 0, 0, 0, 0, 0x10, 0x30, 0x0f, 0xe1, 0, 0, 0, 0,
            ]
        );
    }
}
