use std::{
    fmt,
    fs::File,
    io::{self, Read, Write},
    path::Path,
};

use anyhow::{Result, bail};

use crate::{
    ENVELOPE_DRIVER_OFFSET, ENVELOPE_LEN, ENVELOPE_MAGIC_OFFSET,
    ENVELOPE_VERSION_OFFSET, FORMAT_VERSION, MAGIC, TRACE_FILE_NAME,
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

pub fn write_envelope(
    writer: &mut impl Write,
    driver: TraceDriver,
) -> io::Result<()> {
    let mut envelope = [0; ENVELOPE_LEN];
    envelope[ENVELOPE_MAGIC_OFFSET..ENVELOPE_VERSION_OFFSET]
        .copy_from_slice(MAGIC);
    envelope[ENVELOPE_VERSION_OFFSET..ENVELOPE_DRIVER_OFFSET]
        .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    envelope[ENVELOPE_DRIVER_OFFSET..ENVELOPE_LEN]
        .copy_from_slice(&driver.code().to_le_bytes());
    writer.write_all(&envelope)
}

/// Reads the envelope, checking that this is a bombadil trace in the
/// supported format version, and returns the driver that wrote it.
pub fn read_envelope(reader: &mut impl Read) -> Result<TraceDriver> {
    let mut envelope = [0; ENVELOPE_LEN];
    if reader.read_exact(&mut envelope).is_err()
        || &envelope[ENVELOPE_MAGIC_OFFSET..ENVELOPE_VERSION_OFFSET] != MAGIC
    {
        bail!("not a bombadil trace");
    }
    let format_version = u16::from_le_bytes([
        envelope[ENVELOPE_VERSION_OFFSET],
        envelope[ENVELOPE_VERSION_OFFSET + 1],
    ]);
    if format_version != FORMAT_VERSION {
        bail!(
            "unsupported trace format version {format_version} \
             (expected {FORMAT_VERSION})"
        );
    }
    Ok(TraceDriver::from_code(u16::from_le_bytes([
        envelope[ENVELOPE_DRIVER_OFFSET],
        envelope[ENVELOPE_DRIVER_OFFSET + 1],
    ])))
}

/// The driver of the binary trace at `path` (a trace file or a run
/// directory), or `None` if there is no binary trace there.
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
        Err(error) => return Err(error.into()),
    };
    let mut magic = [0; MAGIC.len()];
    match file.read_exact(&mut magic) {
        Ok(()) if &magic == MAGIC => {}
        // Not a binary trace, e.g. a browser trace.jsonl.
        _ => return Ok(None),
    }
    read_envelope(&mut File::open(&path)?).map(Some)
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
    fn envelope_rejects_other_files_and_versions() {
        let error =
            read_envelope(&mut &b"{\"timestamp\":0}\n"[..]).unwrap_err();
        assert_eq!(error.to_string(), "not a bombadil trace");
        assert!(read_envelope(&mut &b""[..]).is_err());

        let mut bytes = Vec::new();
        write_envelope(&mut bytes, TraceDriver::Terminal).unwrap();
        bytes[ENVELOPE_VERSION_OFFSET..ENVELOPE_DRIVER_OFFSET]
            .copy_from_slice(&2u16.to_le_bytes());
        let error = read_envelope(&mut bytes.as_slice()).unwrap_err();
        assert!(error.to_string().contains("format version 2"), "{error}");
    }

    #[test]
    fn trace_driver_detects_binary_traces() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(trace_driver(directory.path()).unwrap(), None);

        let mut bytes = Vec::new();
        write_envelope(&mut bytes, TraceDriver::Terminal).unwrap();
        std::fs::write(directory.path().join(TRACE_FILE_NAME), &bytes).unwrap();
        assert_eq!(
            trace_driver(directory.path()).unwrap(),
            Some(TraceDriver::Terminal)
        );

        let jsonl = directory.path().join("trace.jsonl");
        std::fs::write(&jsonl, "{\"timestamp\":0}\n").unwrap();
        assert_eq!(trace_driver(&jsonl).unwrap(), None);
    }
}
