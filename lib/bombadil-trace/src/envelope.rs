use std::{
    fmt,
    fs::File,
    io::{self, Read, Write},
    mem::offset_of,
    path::Path,
};

use anyhow::{Context, Result, bail};
use zerocopy::{FromBytes, Immutable, IntoBytes};

use crate::{ENVELOPE_LEN, FORMAT_VERSION, MAGIC, record::read_full};

/// The envelope, as laid out in the file.
#[derive(FromBytes, IntoBytes, Immutable)]
#[repr(C)]
pub(crate) struct Envelope {
    magic: [u8; 8],
    version: u16,
    driver: u16,
    crc: u32,
}

impl Envelope {
    /// The CRC of the fields before `crc`.
    fn compute_crc(&self) -> u32 {
        crc32fast::hash(&self.as_bytes()[..offset_of!(Envelope, crc)])
    }
}

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
    let mut envelope = Envelope {
        magic: *MAGIC,
        version: FORMAT_VERSION,
        driver: driver_code(driver)?,
        crc: 0,
    };
    envelope.crc = envelope.compute_crc();
    writer.write_all(envelope.as_bytes())
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

fn parse_envelope(bytes: &[u8]) -> Result<TraceDriver> {
    // Not even the start of a trace.
    let magic_length = bytes.len().min(MAGIC.len());
    if bytes[..magic_length] != MAGIC[..magic_length] {
        bail!("not a bombadil trace");
    }
    let Ok(envelope) = Envelope::read_from_bytes(bytes) else {
        // Empty, or cut in the envelope: report that rather than a file
        // of another kind.
        bail!(
            "empty or truncated trace ({} bytes, the envelope alone is \
             {ENVELOPE_LEN})",
            bytes.len()
        );
    };
    if envelope.version != FORMAT_VERSION {
        bail!(
            "unsupported trace format version {} (expected \
             {FORMAT_VERSION})",
            envelope.version
        );
    }
    if envelope.compute_crc() != envelope.crc {
        bail!("corrupt trace envelope (checksum mismatch)");
    }
    Ok(TraceDriver::from_code(envelope.driver))
}

fn has_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

/// The driver of the trace file at `path`, or `None` if it is missing or
/// does not start with the trace magic. Directories are rejected.
pub fn trace_driver(path: &Path) -> Result<Option<TraceDriver>> {
    let mut file = match File::open(path) {
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
    if !has_magic(envelope) {
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
    use crate::TRACE_FILE_NAME;

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

        let mut newer = Envelope::read_from_bytes(&bytes).unwrap();
        newer.version = 2;
        let error = read_envelope(&mut newer.as_bytes()).unwrap_err();
        assert!(error.to_string().contains("format version 2"), "{error}");

        // Terminal (1) flipped to browser (2) would otherwise be misread.
        let mut flipped = Envelope::read_from_bytes(&bytes).unwrap();
        flipped.driver = 2;
        let error = read_envelope(&mut flipped.as_bytes()).unwrap_err();
        assert!(error.to_string().contains("checksum"), "{error}");
    }

    #[test]
    fn trace_driver_detects_binary_traces() {
        let directory = tempfile::tempdir().unwrap();
        assert!(trace_driver(directory.path()).is_err());

        let mut bytes = Vec::new();
        write_envelope(&mut bytes, TraceDriver::Terminal).unwrap();
        let trace = directory.path().join(TRACE_FILE_NAME);
        std::fs::write(&trace, &bytes).unwrap();
        assert_eq!(trace_driver(&trace).unwrap(), Some(TraceDriver::Terminal));
        // Under another name, recognized by its magic bytes.
        let other = directory.path().join("other.bin");
        std::fs::copy(&trace, &other).unwrap();
        assert_eq!(trace_driver(&other).unwrap(), Some(TraceDriver::Terminal));

        let jsonl = directory.path().join("trace.jsonl");
        std::fs::write(&jsonl, "{\"timestamp\":0}\n").unwrap();
        assert_eq!(trace_driver(&jsonl).unwrap(), None);

        // A damaged envelope is reported under any name.
        bytes[12] ^= 1;
        std::fs::write(&trace, &bytes).unwrap();
        assert!(trace_driver(directory.path()).is_err());
        assert!(trace_driver(&trace).is_err());

        // A failure to open other than a missing file is reported too.
        assert!(trace_driver(&jsonl.join(TRACE_FILE_NAME)).is_err());
    }
}
