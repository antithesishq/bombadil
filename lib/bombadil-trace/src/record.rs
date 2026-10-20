use std::{
    io::{self, Read, Seek, SeekFrom, Write},
    mem::offset_of,
};

use anyhow::{Result, bail};
use zerocopy::{FromBytes, FromZeros, Immutable, IntoBytes};

use crate::{
    PAYLOAD_REF_LEN, RECORD_HEADER_LEN, RECORD_PAYLOAD_BYTES_MAX, tag,
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
    pub payload_crc: u32,
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

/// A record read back from a trace.
#[derive(Debug)]
pub(crate) struct RawRecord {
    /// Position of the record in the trace, from 0.
    pub index: usize,
    /// Byte offset of the payload in the trace file.
    pub payload_offset: u64,
    pub payload_length: usize,
    /// The CRC-32 of the payload, checked even if it was skipped.
    pub payload_crc: u32,
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
    /// and it isn't a state's metadata or a payload reference.
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
        let (header, read) = RecordHeader::read(&mut self.reader)?;
        if read == 0 {
            return Ok(None);
        }
        if read < RECORD_HEADER_LEN {
            return Ok(self.truncated());
        }
        header.check()?;
        // Checked before its payload is read, so a reference can't pass
        // for a cut record either.
        if header.tag == tag::STATE_PAYLOAD_REF
            && header.length() != PAYLOAD_REF_LEN
        {
            bail!(
                "corrupt payload reference ({} bytes, not {PAYLOAD_REF_LEN})",
                header.length()
            );
        }
        let keep = keep_payload
            || matches!(
                header.tag,
                tag::STATE | tag::STATE_WITH_PAYLOAD | tag::STATE_PAYLOAD_REF
            );
        let Some(payload) = header.read_payload(&mut self.reader, keep)? else {
            return Ok(self.truncated());
        };
        let record = RawRecord {
            index: self.records_read,
            payload_offset: self.offset + RECORD_HEADER_LEN as u64,
            payload_length: header.length(),
            payload_crc: header.payload_crc,
            tag: header.tag,
            payload,
        };
        self.records_read += 1;
        self.offset += (RECORD_HEADER_LEN + header.length()) as u64;
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

impl<R: Read + Seek> RecordReader<R> {
    /// Reads again the payload of the [`tag::STATE_PAYLOAD`] record at
    /// `offset` (an earlier record's [`RawRecord::payload_offset`]),
    /// checking that it is the one read there before, of `length` bytes
    /// with CRC-32 `crc`. Reading then carries on where it left off, and
    /// stops if this fails.
    pub fn read_payload_again(
        &mut self,
        offset: u64,
        length: usize,
        crc: u32,
    ) -> Result<Vec<u8>> {
        let header_offset = offset.saturating_sub(RECORD_HEADER_LEN as u64);
        let payload = self
            .reader
            .seek(SeekFrom::Start(header_offset))
            .map_err(cant_seek)
            .and_then(|_| read_payload_record(&mut self.reader, length, crc));
        let payload = payload.and_then(|payload| {
            self.reader
                .seek(SeekFrom::Start(self.offset))
                .map_err(cant_seek)?;
            Ok(payload)
        });
        if payload.is_err() {
            self.failed = true;
        }
        payload
    }
}

/// A failure to seek back to a shared payload, which a pipe can't.
fn cant_seek(error: io::Error) -> anyhow::Error {
    anyhow::Error::from(error).context(
        "failed to seek to it (reading payloads shared between states \
         needs a seekable trace file, not a pipe)",
    )
}

/// Reads the [`tag::STATE_PAYLOAD`] record `reader` is at, which must
/// have a payload of `length` bytes with CRC-32 `crc`, and returns its
/// payload, checked against both checksums.
fn read_payload_record(
    reader: &mut impl Read,
    length: usize,
    crc: u32,
) -> Result<Vec<u8>> {
    let (header, read) = RecordHeader::read(reader)?;
    if read < RECORD_HEADER_LEN {
        bail!("truncated state payload");
    }
    header.check()?;
    if header.tag != tag::STATE_PAYLOAD
        || header.length() != length
        || header.payload_crc != crc
    {
        bail!("not the state payload read there before");
    }
    match header.read_payload(reader, true)? {
        Some(Some(payload)) => Ok(payload),
        _ => bail!("truncated state payload"),
    }
}

/// The payload of a [`tag::STATE_PAYLOAD_REF`] record, as laid out in the
/// file: the earlier state whose payload it shares, and that payload's
/// length and CRC-32.
#[derive(FromBytes, IntoBytes, Immutable)]
#[repr(C)]
pub(crate) struct PayloadRef {
    pub state: u32,
    pub length: u32,
    pub crc: u32,
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

    /// A payload reference of another length is a bad record, even if
    /// the file ends before its payload would.
    #[test]
    fn rejects_payload_references_of_another_length() {
        for (length, cut) in
            [(PAYLOAD_REF_LEN - 1, 0), (PAYLOAD_REF_LEN + 1, 2)]
        {
            let mut file = Vec::new();
            write_record(
                &mut file,
                tag::STATE_PAYLOAD_REF,
                &[&vec![0; length]],
            )
            .unwrap();
            file.truncate(file.len() - cut);
            let error = format!("{:#}", records(&file).unwrap_err());
            assert_eq!(
                error,
                format!(
                    "failed to read trace record 0: corrupt payload \
                     reference ({length} bytes, not 12)"
                )
            );
        }
        let mut file = Vec::new();
        write_record(
            &mut file,
            tag::STATE_PAYLOAD_REF,
            &[&[0; PAYLOAD_REF_LEN]],
        )
        .unwrap();
        assert_eq!(records(&file).unwrap().len(), 1);
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
