use std::{
    io::{self, Cursor, Read, Write},
    mem::offset_of,
};

use anyhow::{Result, bail};
use zerocopy::{FromBytes, FromZeros, Immutable, IntoBytes};

use crate::{RECORD_HEADER_LEN, RECORD_PAYLOAD_BYTES_MAX, tag};

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
    let mut payload_crc = crc32fast::Hasher::new();
    for part in parts {
        payload_crc.update(part);
    }
    let header =
        RecordHeader::new(tag, payload_length as u32, payload_crc.finalize());

    writer.write_all(header.as_bytes())?;
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

/// How many bytes of a payload [`RecordHeader::read_payload`] reads at a
/// time.
pub(crate) const PAYLOAD_READ_CHUNK: usize = 64 * 1024;

/// A record's header, as laid out in the file.
#[derive(FromBytes, IntoBytes, Immutable)]
#[repr(C)]
pub(crate) struct RecordHeader {
    pub tag: u16,
    /// Padding made explicit, as zerocopy refuses implicit padding.
    /// Written as 0, and rejected otherwise, so it stays free to use.
    reserved: u16,
    length: u32,
    header_crc: u32,
    payload_crc: u32,
}

impl RecordHeader {
    pub fn new(tag: u16, length: u32, payload_crc: u32) -> Self {
        let mut header = RecordHeader {
            tag,
            reserved: 0,
            length,
            header_crc: 0,
            payload_crc,
        };
        header.header_crc = header.compute_header_crc();
        header
    }

    /// Reads a header, unchecked, with the number of bytes read: fewer
    /// than [`RECORD_HEADER_LEN`] if the input ends first.
    pub fn read(reader: &mut impl Read) -> io::Result<(Self, usize)> {
        let mut header = RecordHeader::new_zeroed();
        let read = read_full(reader, header.as_mut_bytes())?;
        Ok((header, read))
    }

    /// The CRC of the fields before `header_crc`.
    fn compute_header_crc(&self) -> u32 {
        crc32fast::hash(&self.as_bytes()[..offset_of!(Self, header_crc)])
    }

    /// Whether the header's checksum matches.
    pub fn checksum_matches(&self) -> bool {
        self.compute_header_crc() == self.header_crc
    }

    /// Fails unless the checksum matches and the fields are valid.
    pub fn check(&self) -> Result<()> {
        if !self.checksum_matches() {
            bail!("corrupt record header (checksum mismatch)");
        }
        // The checksum matches, so the field was set on purpose, for a
        // use this reader doesn't know: refuse rather than ignore it.
        if self.reserved != 0 {
            bail!("unsupported record header (reserved field set)");
        }
        if self.length() > RECORD_PAYLOAD_BYTES_MAX {
            bail!("corrupt record (length out of range)");
        }
        Ok(())
    }

    /// The payload length.
    pub fn length(&self) -> usize {
        self.length as usize
    }

    /// Sets the reserved field, keeping the checksum matching.
    #[cfg(test)]
    pub fn set_reserved(&mut self, reserved: u16) {
        self.reserved = reserved;
        self.header_crc = self.compute_header_crc();
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
        let mut chunk = vec![0; self.length().min(PAYLOAD_READ_CHUNK)];
        let mut remaining = self.length();
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayloadLocation {
    pub(crate) offset: u64,
    pub(crate) chunk: Option<u64>,
}

/// A record read back from a trace.
#[derive(Debug)]
pub(crate) struct RawRecord {
    /// Position of the record in the trace, from 0.
    pub index: usize,
    pub location: PayloadLocation,
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
    chunk: Option<(u64, Cursor<Vec<u8>>)>,
}

impl<R: Read> RecordReader<R> {
    /// `reader` must be positioned right after the envelope.
    pub fn new(reader: R) -> Self {
        RecordReader {
            reader,
            records_read: 0,
            offset: crate::ENVELOPE_LEN as u64,
            failed: false,
            chunk: None,
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
        loop {
            if self.chunk.as_ref().is_some_and(|(_, chunk)| {
                chunk.position() == chunk.get_ref().len() as u64
            }) {
                self.chunk = None;
            }
            let (offset, chunk_offset, mut reader): (_, _, &mut dyn Read) =
                match &mut self.chunk {
                    Some((offset, chunk)) => {
                        (chunk.position(), Some(*offset), chunk)
                    }
                    None => (self.offset, None, &mut self.reader),
                };
            let (header, read) = RecordHeader::read(&mut reader)?;
            if read < RECORD_HEADER_LEN {
                if chunk_offset.is_some() {
                    bail!("compressed chunk ends inside a record header");
                }
                return Ok(if read == 0 { None } else { self.truncated() });
            }
            header.check()?;
            if chunk_offset.is_some()
                && matches!(header.tag, tag::CHUNK | tag::END)
            {
                bail!("invalid record inside compressed chunk");
            }
            let keep = keep_payload
                || matches!(
                    header.tag,
                    tag::STATE | tag::STATE_WITH_PAYLOAD | tag::CHUNK
                );
            let Some(payload) = header.read_payload(&mut reader, keep)? else {
                if chunk_offset.is_some() {
                    bail!("compressed chunk ends inside a record");
                }
                return Ok(self.truncated());
            };
            if chunk_offset.is_none() {
                self.offset += (RECORD_HEADER_LEN + header.length()) as u64;
            }
            if header.tag == tag::CHUNK {
                let decoded =
                    crate::chunk::decode(payload.as_deref().unwrap())?;
                self.chunk = Some((offset, Cursor::new(decoded)));
                continue;
            }
            let record = RawRecord {
                index: self.records_read,
                location: PayloadLocation {
                    offset: offset + RECORD_HEADER_LEN as u64,
                    chunk: chunk_offset,
                },
                payload_length: header.length(),
                tag: header.tag,
                payload,
            };
            self.records_read += 1;
            return Ok(Some(record));
        }
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
    use crate::tag;

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
        // (the first record is 18 bytes).
        for length in [18 + 4, file.len() - 1] {
            let records = records(&file[..length]).unwrap();
            assert_eq!(records.len(), 1, "cut at {length}");
        }
        // Cut in the last byte of an empty record's header, whose payload
        // CRC is 0, so the bytes read would still pass for a header.
        let mut end = Vec::new();
        write_record(&mut end, tag::END, &[]).unwrap();
        assert!(records(&end[..RECORD_HEADER_LEN - 1]).unwrap().is_empty());
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
        for record_start in [0, 18] {
            let mut file = two_records();
            let length = record_start + offset_of!(RecordHeader, length) + 3;
            file[length] ^= 0x10;
            let error = format!("{:#}", records(&file).unwrap_err());
            assert!(error.contains("record header"), "{error}");
        }

        // Out of range, with a matching header checksum.
        let header = RecordHeader::new(0, u32::MAX, 0);
        let error = format!("{:#}", records(header.as_bytes()).unwrap_err());
        assert!(error.contains("length out of range"), "{error}");

        // The limit is part of the format: 256 MiB exactly.
        assert_eq!(RECORD_PAYLOAD_BYTES_MAX, 256 << 20);
        let max = RECORD_PAYLOAD_BYTES_MAX as u32;
        let header = RecordHeader::new(0, max, 0);
        assert!(header.check().is_ok());
        assert_eq!(header.length(), 256 << 20);
        assert!(RecordHeader::new(0, max + 1, 0).check().is_err());
        assert!(check_payload_length(RECORD_PAYLOAD_BYTES_MAX).is_ok());
        assert!(check_payload_length(RECORD_PAYLOAD_BYTES_MAX + 1).is_err());
    }

    #[test]
    fn rejects_a_nonzero_reserved_field() {
        let reserved = offset_of!(RecordHeader, reserved);
        // Corrupted: the header's checksum covers the field.
        let mut file = two_records();
        file[reserved] ^= 0x01;
        let error = format!("{:#}", records(&file).unwrap_err());
        assert!(error.contains("record header (checksum"), "{error}");

        // Set, with a matching checksum: still rejected.
        let mut file = two_records();
        let second = 18..18 + RECORD_HEADER_LEN;
        let mut header =
            RecordHeader::read_from_bytes(&file[second.clone()]).unwrap();
        header.set_reserved(1);
        file[second].copy_from_slice(header.as_bytes());
        let error = format!("{:#}", records(&file).unwrap_err());
        assert_eq!(
            error,
            "failed to read trace record 1: unsupported record header \
             (reserved field set)"
        );
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
