use std::{
    fmt,
    fs::File,
    io::{self, Read, Write},
    path::Path,
};

use anyhow::{Context, Result, bail};

use crate::{
    ENVELOPE_CRC_OFFSET, ENVELOPE_DRIVER_OFFSET, ENVELOPE_LEN,
    ENVELOPE_MAGIC_OFFSET, ENVELOPE_VERSION_OFFSET, FORMAT_VERSION, MAGIC,
    TRACE_FILE_NAME, record::read_full,
};

/// The driver that wrote a trace.
///
/// | Code     | Driver                                         |
/// | -------: | :--------------------------------------------- |
/// |        0 | reserved, never written                        |
/// |        1 | terminal                                       |
/// |        2 | browser                                        |
/// | `0xFFFF` | reserved for drivers outside bombadil          |
///
/// Codes are never reused, even after a driver is removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceDriver {
    Terminal,
    Browser,
    /// A code this version of bombadil doesn't know.
    Unknown(u16),
}

impl TraceDriver {
    pub fn code(self) -> u16 {
        match self {
            TraceDriver::Terminal => 1,
            TraceDriver::Browser => 2,
            TraceDriver::Unknown(code) => code,
        }
    }

    pub fn from_code(code: u16) -> Self {
        match code {
            1 => TraceDriver::Terminal,
            2 => TraceDriver::Browser,
            code => TraceDriver::Unknown(code),
        }
    }
}

impl fmt::Display for TraceDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TraceDriver::Terminal => write!(f, "terminal"),
            TraceDriver::Browser => write!(f, "browser"),
            TraceDriver::Unknown(0xFFFF) => write!(f, "external"),
            TraceDriver::Unknown(code) => write!(f, "unknown ({code})"),
        }
    }
}

/// Writes the envelope of a trace written by `driver`. Fails for the
/// reserved code 0, and for an `Unknown` code that belongs to a known
/// driver, so a code always reads back as the driver that wrote it.
pub fn write_envelope(
    writer: &mut impl Write,
    driver: TraceDriver,
) -> io::Result<()> {
    let code = driver_code(driver)?;
    let mut envelope = [0; ENVELOPE_LEN];
    envelope[ENVELOPE_MAGIC_OFFSET..ENVELOPE_VERSION_OFFSET]
        .copy_from_slice(MAGIC);
    envelope[ENVELOPE_VERSION_OFFSET..ENVELOPE_DRIVER_OFFSET]
        .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    envelope[ENVELOPE_DRIVER_OFFSET..ENVELOPE_CRC_OFFSET]
        .copy_from_slice(&code.to_le_bytes());
    let crc = crc32fast::hash(&envelope[..ENVELOPE_CRC_OFFSET]);
    envelope[ENVELOPE_CRC_OFFSET..ENVELOPE_LEN]
        .copy_from_slice(&crc.to_le_bytes());
    writer.write_all(&envelope)
}

/// The code to write for `driver`, if it may be written (see
/// [`write_envelope`]).
pub(crate) fn driver_code(driver: TraceDriver) -> io::Result<u16> {
    let code = driver.code();
    if code == 0 || TraceDriver::from_code(code) != driver {
        return Err(io::Error::other(format!(
            "invalid trace driver code {code}"
        )));
    }
    Ok(code)
}

/// Reads the envelope, checking that this is a bombadil trace in the
/// supported format version, and returns the driver that wrote it.
pub(crate) fn read_envelope(reader: &mut impl Read) -> Result<TraceDriver> {
    let mut envelope = [0; ENVELOPE_LEN];
    let read = read_full(reader, &mut envelope)?;
    parse_envelope(&envelope[..read])
}

fn parse_envelope(envelope: &[u8]) -> Result<TraceDriver> {
    if envelope.len() < ENVELOPE_LEN {
        // Empty, or cut in the envelope: report that rather than a
        // file of another kind, unless it can't be the start of a trace.
        let magic_length = envelope.len().min(MAGIC.len());
        if envelope[..magic_length] == MAGIC[..magic_length] {
            bail!(
                "empty or truncated trace ({} bytes, the envelope alone \
                 is {ENVELOPE_LEN})",
                envelope.len()
            );
        }
    }
    if !has_magic(envelope) {
        bail!("not a bombadil trace");
    }
    let field = |offset: usize| {
        u16::from_le_bytes([envelope[offset], envelope[offset + 1]])
    };
    let format_version = field(ENVELOPE_VERSION_OFFSET);
    if format_version != FORMAT_VERSION {
        bail!(
            "unsupported trace format version {format_version} \
             (expected {FORMAT_VERSION})"
        );
    }
    let crc = u32::from_le_bytes(
        envelope[ENVELOPE_CRC_OFFSET..ENVELOPE_LEN]
            .try_into()
            .expect("4 bytes"),
    );
    if crc32fast::hash(&envelope[..ENVELOPE_CRC_OFFSET]) != crc {
        bail!("corrupt trace envelope (checksum mismatch)");
    }
    Ok(TraceDriver::from_code(field(ENVELOPE_DRIVER_OFFSET)))
}

fn has_magic(bytes: &[u8]) -> bool {
    bytes.get(ENVELOPE_MAGIC_OFFSET..ENVELOPE_VERSION_OFFSET) == Some(MAGIC)
}

/// The driver of the binary trace at `path` (a trace file or a run
/// directory), or `None` if there is no binary trace there.
///
/// A `trace.bin` that isn't a valid trace is an error rather than `None`,
/// so a damaged trace isn't mistaken for a missing one. Any other file is
/// only a binary trace if it starts with the magic bytes.
pub fn trace_driver(path: &Path) -> Result<Option<TraceDriver>> {
    let path = if path.is_dir() {
        path.join(TRACE_FILE_NAME)
    } else {
        path.to_path_buf()
    };
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => {
            return Err(anyhow::Error::new(error)
                .context(format!("failed to open {}", path.display())));
        }
    };
    let mut envelope = [0; ENVELOPE_LEN];
    let read = read_full(&mut file, &mut envelope)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let envelope = &envelope[..read];
    let is_trace_file = path.file_name() == Some(TRACE_FILE_NAME.as_ref());
    if !is_trace_file && !has_magic(envelope) {
        // Some other file, e.g. a legacy JSON lines trace.
        return Ok(None);
    }
    parse_envelope(envelope)
        .with_context(|| format!("failed to read {}", path.display()))
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trips_driver() {
        for driver in [
            TraceDriver::Terminal,
            TraceDriver::Browser,
            TraceDriver::Unknown(0xFFFF),
        ] {
            let mut bytes = Vec::new();
            write_envelope(&mut bytes, driver).unwrap();
            assert_eq!(bytes.len(), ENVELOPE_LEN);
            assert_eq!(read_envelope(&mut bytes.as_slice()).unwrap(), driver);
        }
    }

    #[test]
    fn envelope_refuses_reserved_and_aliased_driver_codes() {
        for driver in [TraceDriver::Unknown(0), TraceDriver::Unknown(1)] {
            assert!(write_envelope(&mut Vec::new(), driver).is_err());
        }
    }

    #[test]
    fn envelope_rejects_other_files_versions_and_corruption() {
        let error =
            read_envelope(&mut &b"{\"timestamp\":0}\n"[..]).unwrap_err();
        assert_eq!(error.to_string(), "not a bombadil trace");
        // Too short, but not the start of a trace either.
        let error = read_envelope(&mut &b"{"[..]).unwrap_err();
        assert_eq!(error.to_string(), "not a bombadil trace");

        let mut bytes = Vec::new();
        write_envelope(&mut bytes, TraceDriver::Terminal).unwrap();
        for length in [0, 4, ENVELOPE_LEN - 1] {
            let error = read_envelope(&mut &bytes[..length]).unwrap_err();
            assert!(
                error.to_string().starts_with("empty or truncated trace"),
                "{error}"
            );
        }

        let mut newer = bytes.clone();
        newer[ENVELOPE_VERSION_OFFSET..ENVELOPE_DRIVER_OFFSET]
            .copy_from_slice(&2u16.to_le_bytes());
        let error = read_envelope(&mut newer.as_slice()).unwrap_err();
        assert!(error.to_string().contains("format version 2"), "{error}");

        // Terminal (1) flipped to browser (2) would otherwise be misread.
        let mut flipped = bytes.clone();
        flipped[ENVELOPE_DRIVER_OFFSET] = 2;
        let error = read_envelope(&mut flipped.as_slice()).unwrap_err();
        assert!(error.to_string().contains("checksum"), "{error}");
    }

    #[test]
    fn trace_driver_detects_binary_traces() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(trace_driver(directory.path()).unwrap(), None);

        let mut bytes = Vec::new();
        write_envelope(&mut bytes, TraceDriver::Terminal).unwrap();
        let trace = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&trace, &bytes).unwrap();
        assert_eq!(
            trace_driver(directory.path()).unwrap(),
            Some(TraceDriver::Terminal)
        );
        // Under another name, recognized by its magic bytes.
        let other = directory.path().join("other.bin");
        std::fs::copy(&trace, &other).unwrap();
        assert_eq!(trace_driver(&other).unwrap(), Some(TraceDriver::Terminal));

        let jsonl = directory.path().join("trace.jsonl");
        std::fs::write(&jsonl, "{\"timestamp\":0}\n").unwrap();
        assert_eq!(trace_driver(&jsonl).unwrap(), None);

        // A damaged trace.bin is reported, not taken for a missing one.
        std::fs::write(&trace, b"").unwrap();
        assert!(trace_driver(directory.path()).is_err());
        assert!(trace_driver(&trace).is_err());

        // A failure to open other than a missing file is reported too.
        assert!(trace_driver(&jsonl.join(TRACE_FILE_NAME)).is_err());
    }
}
