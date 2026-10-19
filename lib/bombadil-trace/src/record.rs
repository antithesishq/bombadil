use std::io::{self, Read, Write};

use anyhow::{Context, Result, bail};

use crate::{
    RECORD_CRC_OFFSET, RECORD_HEADER_LEN, RECORD_LENGTH_OFFSET,
    RECORD_PAYLOAD_BYTES_MAX, RECORD_TAG_OFFSET,
};

/// Writes one record whose payload is `parts`, concatenated, so a large
/// part can be passed through without first being copied into a buffer.
pub fn write_record(
    writer: &mut impl Write,
    tag: u16,
    parts: &[&[u8]],
) -> io::Result<()> {
    let payload_length: usize = parts.iter().map(|part| part.len()).sum();
    if payload_length > RECORD_PAYLOAD_BYTES_MAX {
        return Err(io::Error::other("trace record too large"));
    }
    let mut header = [0; RECORD_HEADER_LEN];
    header[RECORD_TAG_OFFSET..RECORD_LENGTH_OFFSET]
        .copy_from_slice(&tag.to_le_bytes());
    header[RECORD_LENGTH_OFFSET..RECORD_CRC_OFFSET]
        .copy_from_slice(&(payload_length as u32).to_le_bytes());
    let mut crc = crc32fast::Hasher::new();
    crc.update(&header[..RECORD_CRC_OFFSET]);
    for part in parts {
        crc.update(part);
    }
    header[RECORD_CRC_OFFSET..RECORD_HEADER_LEN]
        .copy_from_slice(&crc.finalize().to_le_bytes());

    writer.write_all(&header)?;
    for part in parts {
        // Large parts exceed a `BufWriter`'s buffer, which then writes
        // them to the file directly instead of copying them.
        writer.write_all(part)?;
    }
    Ok(())
}

/// A record read back from a trace.
#[derive(Debug)]
pub struct RawRecord {
    /// Position of the record in the trace, from 0.
    pub index: usize,
    /// Byte offset of the payload in the trace file.
    pub payload_offset: u64,
    pub tag: u16,
    pub payload: Vec<u8>,
}

/// Reads the records following an envelope, checking each one's length
/// and checksum. Reading stops at the first bad record, since the
/// position of the next one is then unknown.
pub struct RecordReader<R> {
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

    fn read_record(&mut self) -> Result<Option<RawRecord>> {
        let mut header = [0; RECORD_HEADER_LEN];
        // A clean end of the file is only allowed between records.
        let mut filled = 0;
        while filled < header.len() {
            match self.reader.read(&mut header[filled..])? {
                0 if filled == 0 => return Ok(None),
                0 => bail!("truncated record"),
                read => filled += read,
            }
        }
        let field =
            |offset: usize, length: usize| &header[offset..offset + length];
        let tag = u16::from_le_bytes(
            field(RECORD_TAG_OFFSET, 2).try_into().expect("2 bytes"),
        );
        let payload_length = u32::from_le_bytes(
            field(RECORD_LENGTH_OFFSET, 4).try_into().expect("4 bytes"),
        ) as usize;
        let crc = u32::from_le_bytes(
            field(RECORD_CRC_OFFSET, 4).try_into().expect("4 bytes"),
        );
        if payload_length > RECORD_PAYLOAD_BYTES_MAX {
            bail!("corrupt record (length out of range)");
        }
        let mut payload = vec![0; payload_length];
        self.reader
            .read_exact(&mut payload)
            .context("truncated record")?;
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&header[..RECORD_CRC_OFFSET]);
        hasher.update(&payload);
        if hasher.finalize() != crc {
            bail!("corrupt record (checksum mismatch)");
        }

        let record = RawRecord {
            index: self.records_read,
            payload_offset: self.offset + RECORD_HEADER_LEN as u64,
            tag,
            payload,
        };
        self.records_read += 1;
        self.offset += (RECORD_HEADER_LEN + payload_length) as u64;
        Ok(Some(record))
    }
}

impl<R: Read> Iterator for RecordReader<R> {
    type Item = Result<RawRecord>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        let index = self.records_read;
        let result = self
            .read_record()
            .with_context(|| format!("failed to read trace record {index}"))
            .transpose()?;
        if result.is_err() {
            self.failed = true;
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ENVELOPE_LEN, tag};

    fn records(file: &[u8]) -> Result<Vec<RawRecord>> {
        RecordReader::new(file).collect()
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
        assert_eq!(records[0].payload, b"{}");
        assert_eq!(records[0].payload_offset, (ENVELOPE_LEN + 10) as u64);
        assert_eq!(records[1].index, 1);
        assert_eq!(records[1].tag, tag::STATE_PAYLOAD);
        assert_eq!(
            records[1].payload_offset,
            (ENVELOPE_LEN + 10 + 2 + 10) as u64
        );
    }

    #[test]
    fn rejects_truncated_record() {
        let mut file = two_records();
        file.truncate(file.len() - 1);
        let error = format!("{:#}", records(&file).unwrap_err());
        assert!(
            error.contains("record 1") && error.contains("truncated"),
            "{error}"
        );
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
    fn rejects_corrupted_length_without_allocating() {
        let mut file = two_records();
        file[RECORD_LENGTH_OFFSET..RECORD_CRC_OFFSET]
            .copy_from_slice(&u32::MAX.to_le_bytes());
        let error = format!("{:#}", records(&file).unwrap_err());
        assert!(error.contains("length out of range"), "{error}");
    }

    #[test]
    fn stops_after_error() {
        let mut file = two_records();
        file[RECORD_HEADER_LEN] ^= 0x01;
        let mut reader = RecordReader::new(file.as_slice());
        assert!(reader.next().unwrap().is_err());
        assert!(reader.next().is_none());
    }
}
