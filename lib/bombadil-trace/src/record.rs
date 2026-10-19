use std::io::{self, Read, Write};

use anyhow::{Result, bail};

use crate::{
    RECORD_HEADER_CRC_OFFSET, RECORD_HEADER_LEN, RECORD_LENGTH_OFFSET,
    RECORD_PAYLOAD_BYTES_MAX, RECORD_PAYLOAD_CRC_OFFSET, RECORD_TAG_OFFSET,
    tag,
};

/// Fails if a payload of `length` bytes can't be written to a trace.
pub(crate) fn check_payload_length(length: usize) -> io::Result<()> {
    if length > RECORD_PAYLOAD_BYTES_MAX {
        return Err(io::Error::other(format!(
            "trace record too large ({length} bytes, at most \
             {RECORD_PAYLOAD_BYTES_MAX})"
        )));
    }
    Ok(())
}

/// Writes one record whose payload is `parts`, concatenated, so a large
/// part can be passed through without first being copied into a buffer.
pub fn write_record(
    writer: &mut impl Write,
    tag: u16,
    parts: &[&[u8]],
) -> io::Result<()> {
    let payload_length: usize = parts.iter().map(|part| part.len()).sum();
    check_payload_length(payload_length)?;
    let mut header = [0; RECORD_HEADER_LEN];
    header[RECORD_TAG_OFFSET..RECORD_LENGTH_OFFSET]
        .copy_from_slice(&tag.to_le_bytes());
    header[RECORD_LENGTH_OFFSET..RECORD_HEADER_CRC_OFFSET]
        .copy_from_slice(&(payload_length as u32).to_le_bytes());
    let header_crc = crc32fast::hash(&header[..RECORD_HEADER_CRC_OFFSET]);
    header[RECORD_HEADER_CRC_OFFSET..RECORD_PAYLOAD_CRC_OFFSET]
        .copy_from_slice(&header_crc.to_le_bytes());
    let mut payload_crc = crc32fast::Hasher::new();
    for part in parts {
        payload_crc.update(part);
    }
    header[RECORD_PAYLOAD_CRC_OFFSET..RECORD_HEADER_LEN]
        .copy_from_slice(&payload_crc.finalize().to_le_bytes());

    writer.write_all(&header)?;
    for part in parts {
        // Large parts exceed a `BufWriter`'s buffer, which then writes
        // them to the file directly instead of copying them.
        writer.write_all(part)?;
    }
    Ok(())
}

/// Reads into `buffer` until it is full or the input ends, and returns
/// the number of bytes read.
pub(crate) fn read_full(
    reader: &mut impl Read,
    buffer: &mut [u8],
) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// A record's header fields, checked against the header's CRC.
pub(crate) struct RecordHeader {
    pub tag: u16,
    pub length: usize,
    payload_crc: u32,
}

impl RecordHeader {
    /// Whether `bytes` is a record header whose checksum matches.
    pub fn checksum_matches(bytes: &[u8; RECORD_HEADER_LEN]) -> bool {
        let header_crc = u32::from_le_bytes(
            bytes[RECORD_HEADER_CRC_OFFSET..RECORD_PAYLOAD_CRC_OFFSET]
                .try_into()
                .expect("4 bytes"),
        );
        crc32fast::hash(&bytes[..RECORD_HEADER_CRC_OFFSET]) == header_crc
    }

    pub fn parse(bytes: [u8; RECORD_HEADER_LEN]) -> Result<Self> {
        let field = |start: usize, end: usize| {
            u32::from_le_bytes(bytes[start..end].try_into().expect("4 bytes"))
        };
        if !Self::checksum_matches(&bytes) {
            bail!("corrupt record header (checksum mismatch)");
        }
        let tag = u16::from_le_bytes(
            bytes[RECORD_TAG_OFFSET..RECORD_LENGTH_OFFSET]
                .try_into()
                .expect("2 bytes"),
        );
        let length =
            field(RECORD_LENGTH_OFFSET, RECORD_HEADER_CRC_OFFSET) as usize;
        if length > RECORD_PAYLOAD_BYTES_MAX {
            bail!("corrupt record (length out of range)");
        }
        Ok(RecordHeader {
            tag,
            length,
            payload_crc: field(RECORD_PAYLOAD_CRC_OFFSET, RECORD_HEADER_LEN),
        })
    }

    /// Reads the payload, checking it against the header's payload CRC.
    /// Keeps it only if `keep` is set. Returns `None` if the input ends
    /// first: the header is intact, so that is a cut, not a bad length.
    pub fn read_payload(
        &self,
        reader: &mut impl Read,
        keep: bool,
    ) -> Result<Option<Option<Vec<u8>>>> {
        let mut hasher = crc32fast::Hasher::new();
        // Read in chunks, so a large payload isn't allocated before the
        // input is known to hold it.
        let mut payload = keep.then(Vec::new);
        let mut chunk = vec![0; self.length.min(64 * 1024)];
        let mut remaining = self.length;
        while remaining > 0 {
            let wanted = remaining.min(chunk.len());
            let read = read_full(reader, &mut chunk[..wanted])?;
            hasher.update(&chunk[..read]);
            if let Some(payload) = &mut payload {
                payload.extend_from_slice(&chunk[..read]);
            }
            if read < wanted {
                return Ok(None);
            }
            remaining -= read;
        }
        if hasher.finalize() != self.payload_crc {
            bail!("corrupt record (checksum mismatch)");
        }
        Ok(Some(payload))
    }
}

/// A record read back from a trace.
#[derive(Debug)]
pub(crate) struct RawRecord {
    /// Position of the record in the trace, from 0.
    pub index: usize,
    /// Byte offset of the payload in the trace file.
    pub payload_offset: u64,
    pub payload_length: usize,
    pub tag: u16,
    /// The payload, unless it was skipped.
    pub payload: Option<Vec<u8>>,
}

/// Reads the records following an envelope, checking each one's length
/// and checksum. Reading stops at the first bad record, since the
/// position of the next one is then unknown. An incomplete last record,
/// left by a run that was killed while writing it, ends the records with
/// a warning instead.
pub(crate) struct RecordReader<R> {
    reader: R,
    records_read: usize,
    offset: u64,
    failed: bool,
}

impl<R: Read> RecordReader<R> {
    /// `reader` must be positioned right after the envelope.
    pub fn new(reader: R) -> Self {
        RecordReader {
            reader,
            records_read: 0,
            offset: crate::ENVELOPE_LEN as u64,
            failed: false,
        }
    }

    /// The next record, with its payload unless `keep_payload` is false
    /// and it isn't a state's metadata.
    pub fn next_record(
        &mut self,
        keep_payload: bool,
    ) -> Option<Result<RawRecord>> {
        if self.failed {
            return None;
        }
        let index = self.records_read;
        let result = self.read_record(keep_payload).transpose()?;
        if result.is_err() {
            self.failed = true;
        }
        Some(result.map_err(|error| {
            error.context(format!("failed to read trace record {index}"))
        }))
    }

    fn read_record(&mut self, keep_payload: bool) -> Result<Option<RawRecord>> {
        let mut header = [0; RECORD_HEADER_LEN];
        let read = read_full(&mut self.reader, &mut header)?;
        if read == 0 {
            return Ok(None);
        }
        if read < header.len() {
            return Ok(self.truncated());
        }
        let header = RecordHeader::parse(header)?;
        let keep = keep_payload
            || matches!(header.tag, tag::STATE | tag::STATE_WITH_PAYLOAD);
        let Some(payload) = header.read_payload(&mut self.reader, keep)? else {
            return Ok(self.truncated());
        };
        let record = RawRecord {
            index: self.records_read,
            payload_offset: self.offset + RECORD_HEADER_LEN as u64,
            payload_length: header.length,
            tag: header.tag,
            payload,
        };
        self.records_read += 1;
        self.offset += (RECORD_HEADER_LEN + header.length) as u64;
        Ok(Some(record))
    }

    /// Fails unless the input ends here, after the last record read.
    pub fn expect_end(&mut self) -> Result<()> {
        if read_full(&mut self.reader, &mut [0])? != 0 {
            self.failed = true;
            bail!(
                "trace continues after its end record (trace record {})",
                self.records_read.saturating_sub(1)
            );
        }
        Ok(())
    }

    fn truncated(&mut self) -> Option<RawRecord> {
        log::warn!(
            "trace ends with an incomplete record {} (the run was probably \
             killed while writing it); reading up to it",
            self.records_read
        );
        self.failed = true;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ENVELOPE_LEN, tag};

    fn records(file: &[u8]) -> Result<Vec<RawRecord>> {
        let mut reader = RecordReader::new(file);
        std::iter::from_fn(|| reader.next_record(true)).collect()
    }

    fn two_records() -> Vec<u8> {
        let mut file = Vec::new();
        write_record(&mut file, tag::STATE, &[b"{", b"}"]).unwrap();
        write_record(&mut file, tag::STATE_PAYLOAD, &[b"payload"]).unwrap();
        file
    }

    #[test]
    fn reads_records_with_offsets() {
        let records = records(&two_records()).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].payload.as_deref(), Some(&b"{}"[..]));
        assert_eq!(records[0].payload_offset, (ENVELOPE_LEN + 14) as u64);
        assert_eq!(records[1].index, 1);
        assert_eq!(records[1].tag, tag::STATE_PAYLOAD);
        assert_eq!(records[1].payload_length, 7);
        assert_eq!(
            records[1].payload_offset,
            (ENVELOPE_LEN + 14 + 2 + 14) as u64
        );
    }

    #[test]
    fn skips_payloads_but_still_checks_them() {
        let file = two_records();
        let mut reader = RecordReader::new(&file[..]);
        let state = reader.next_record(false).unwrap().unwrap();
        assert_eq!(state.payload.as_deref(), Some(&b"{}"[..]));
        let payload = reader.next_record(false).unwrap().unwrap();
        assert_eq!((payload.payload, payload.payload_length), (None, 7));

        let mut file = two_records();
        let last = file.len() - 1;
        file[last] ^= 0x01;
        let mut reader = RecordReader::new(&file[..]);
        reader.next_record(false).unwrap().unwrap();
        assert!(reader.next_record(false).unwrap().is_err());
    }

    #[test]
    fn ends_at_an_incomplete_last_record() {
        let file = two_records();
        // Cut inside the second record's header and inside its payload
        // (the first record is 16 bytes).
        for length in [16 + 4, file.len() - 1] {
            let records = records(&file[..length]).unwrap();
            assert_eq!(records.len(), 1, "cut at {length}");
        }
    }

    #[test]
    fn rejects_corrupted_record() {
        let mut file = two_records();
        let last = file.len() - 1;
        file[last] ^= 0x01;
        let error = format!("{:#}", records(&file).unwrap_err());
        assert!(
            error.contains("record 1") && error.contains("checksum"),
            "{error}"
        );
    }

    #[test]
    fn rejects_corrupted_lengths() {
        // A length pointing past the end of the file isn't taken for a
        // cut: the header's checksum catches it, in any record.
        for record_start in [0, 16] {
            let mut file = two_records();
            let length = record_start + RECORD_LENGTH_OFFSET + 3;
            file[length] ^= 0x10;
            let error = format!("{:#}", records(&file).unwrap_err());
            assert!(error.contains("record header"), "{error}");
        }

        // Out of range, with a matching header checksum.
        let header = |length: u32| {
            let mut header = [0; RECORD_HEADER_LEN];
            header[RECORD_LENGTH_OFFSET..RECORD_HEADER_CRC_OFFSET]
                .copy_from_slice(&length.to_le_bytes());
            let crc = crc32fast::hash(&header[..RECORD_HEADER_CRC_OFFSET]);
            header[RECORD_HEADER_CRC_OFFSET..RECORD_PAYLOAD_CRC_OFFSET]
                .copy_from_slice(&crc.to_le_bytes());
            header
        };
        let error = format!("{:#}", records(&header(u32::MAX)).unwrap_err());
        assert!(error.contains("length out of range"), "{error}");

        // The limit is part of the format: 256 MiB exactly.
        assert_eq!(RECORD_PAYLOAD_BYTES_MAX, 256 << 20);
        let max = RECORD_PAYLOAD_BYTES_MAX as u32;
        assert_eq!(RecordHeader::parse(header(max)).unwrap().length, 256 << 20);
        assert!(RecordHeader::parse(header(max + 1)).is_err());
        assert!(check_payload_length(RECORD_PAYLOAD_BYTES_MAX).is_ok());
        assert!(check_payload_length(RECORD_PAYLOAD_BYTES_MAX + 1).is_err());
    }

    #[test]
    fn stops_after_error() {
        let mut file = two_records();
        file[RECORD_HEADER_LEN] ^= 0x01;
        let mut reader = RecordReader::new(file.as_slice());
        assert!(reader.next_record(true).unwrap().is_err());
        assert!(reader.next_record(true).is_none());
    }
}
