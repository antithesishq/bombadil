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
//!
//! Driver codes are assigned in [`TraceDriver`] and never reused.
//!
//! # Records
//!
//! From offset 12 to the end of the file. Offsets are relative to the
//! start of the record.
//!
//! | Offset | Size          | Field                  |
//! | -----: | ------------: | :--------------------- |
//! |      0 |             2 | Tag (`u16`)            |
//! |      2 |             4 | Payload length (`u32`) |
//! |      6 |             4 | CRC-32 (`u32`)         |
//! |     10 | `payload_len` | Payload                |
//!
//! The CRC-32 (IEEE) covers the tag, the payload length and the payload,
//! so a corrupted record is reported instead of misread. The payload
//! length bounds the record. Tags are assigned in [`tag`]; a tag with
//! [`tag::OPTIONAL`] set may be skipped by readers that don't know it,
//! and any other unknown tag is rejected.
//!
//! A trace is a sequence of states: a [`tag::STATE`] record holding the
//! state's JSON metadata, each optionally followed by a
//! [`tag::STATE_PAYLOAD`] record holding driver-defined bytes for it
//! (e.g. a terminal snapshot or a screenshot).

mod envelope;
mod reader;
mod record;
mod writer;

pub use envelope::{TraceDriver, read_envelope, trace_driver, write_envelope};
pub use reader::{State, StateReader, open_trace};
pub use record::{RawRecord, RecordReader, write_record};
pub use writer::TraceFileWriter;

/// File name of a binary trace in a run directory.
pub const TRACE_FILE_NAME: &str = "trace.bin";

pub const MAGIC: &[u8; 8] = b"BMBDTRC\0";
pub const FORMAT_VERSION: u16 = 1;

pub const ENVELOPE_LEN: usize = 12;
pub const ENVELOPE_MAGIC_OFFSET: usize = 0;
pub const ENVELOPE_VERSION_OFFSET: usize = 8;
pub const ENVELOPE_DRIVER_OFFSET: usize = 10;

pub const RECORD_HEADER_LEN: usize = 10;
pub const RECORD_TAG_OFFSET: usize = 0;
pub const RECORD_LENGTH_OFFSET: usize = 2;
pub const RECORD_CRC_OFFSET: usize = 6;

/// Largest payload accepted, checked before allocating so a corrupted
/// length is reported rather than turned into a huge allocation.
pub const RECORD_PAYLOAD_BYTES_MAX: usize = 256 * 1024 * 1024;

// These sizes are the wire format. Changing them needs a format version
// bump.
const _: () = assert!(ENVELOPE_LEN == 12);
const _: () = assert!(ENVELOPE_DRIVER_OFFSET + 2 == ENVELOPE_LEN);
const _: () = assert!(RECORD_HEADER_LEN == 10);
const _: () = assert!(RECORD_CRC_OFFSET + 4 == RECORD_HEADER_LEN);

/// Record tags.
pub mod tag {
    /// One state's JSON metadata.
    pub const STATE: u16 = 1;
    /// Driver-defined bytes for the state just before it.
    pub const STATE_PAYLOAD: u16 = 2;
    /// Set on tags that readers which don't know them may skip.
    pub const OPTIONAL: u16 = 0x8000;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact bytes of a small trace, so a layout change can't slip in
    /// without a format version bump.
    #[test]
    fn golden_bytes() {
        let mut file = Vec::new();
        write_envelope(&mut file, TraceDriver::Terminal).unwrap();
        write_record(&mut file, tag::STATE, &[b"{}"]).unwrap();
        write_record(&mut file, tag::STATE_PAYLOAD, &[b"ab"]).unwrap();
        assert_eq!(
            file,
            [
                // Envelope: magic, version 1, driver 1.
                b'B', b'M', b'B', b'D', b'T', b'R', b'C', 0, 1, 0, 1, 0,
                // State record: tag 1, length 2, CRC, "{}".
                1, 0, 2, 0, 0, 0, 0x40, 0xd3, 0x3f, 0x06, b'{', b'}',
                // State payload record: tag 2, length 2, CRC, "ab".
                2, 0, 2, 0, 0, 0, 0x8d, 0x23, 0x95, 0xb5, b'a', b'b',
            ]
        );
    }
}
