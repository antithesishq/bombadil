//! Source maps for instrumented scripts: a block records the original file and
//! line it came from, so that coverage can be read per source file and app code
//! told from bundled vendor code. A script without a map records blocks as
//! before.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use serde::Deserialize;

#[derive(Deserialize)]
struct RawSourceMap {
    #[serde(default)]
    sources: Vec<String>,
    #[serde(rename = "sourceRoot", default)]
    source_root: Option<String>,
    #[serde(default)]
    mappings: String,
    /// Sources that are not the app's own code (node_modules, generated
    /// runtime).
    #[serde(rename = "x_google_ignoreList", default)]
    ignore_list: Vec<usize>,
    #[serde(default)]
    sections: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug)]
struct Segment {
    column: u32,
    source: u32,
    /// 0-based line in the source.
    line: u32,
}

pub struct SourceMap {
    sources: Vec<String>,
    ignored: Vec<bool>,
    /// Segments per 0-based generated line, sorted by column.
    lines: Vec<Vec<Segment>>,
}

/// Where a generated position came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Original {
    pub source: String,
    /// 1-based line in the source.
    pub line: u32,
    /// The source is on the map's ignore list (vendor code).
    pub ignored: bool,
}

fn vlq_decode(segment: &str) -> Result<Vec<i64>> {
    const ALPHABET: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut value: i64 = 0;
    let mut shift = 0;
    for byte in segment.bytes() {
        let digit = ALPHABET
            .iter()
            .position(|&c| c == byte)
            .ok_or_else(|| anyhow!("invalid VLQ character {byte:?}"))?
            as i64;
        value |= (digit & 31) << shift;
        shift += 5;
        if digit & 32 == 0 {
            out.push(if value & 1 == 1 { -(value >> 1) } else { value >> 1 });
            value = 0;
            shift = 0;
        }
    }
    Ok(out)
}

impl SourceMap {
    pub fn parse(json: &str) -> Result<Self> {
        let raw: RawSourceMap =
            serde_json::from_str(json).context("source map is not JSON")?;
        if raw.sections.is_some() {
            bail!("index source maps (`sections`) are not supported");
        }
        let root = raw.source_root.unwrap_or_default();
        let sources = raw
            .sources
            .iter()
            .map(|s| {
                if root.is_empty() || s.contains("://") {
                    s.clone()
                } else {
                    format!("{}/{}", root.trim_end_matches('/'), s)
                }
            })
            .collect::<Vec<_>>();
        let mut ignored = vec![false; sources.len()];
        for index in raw.ignore_list {
            if let Some(flag) = ignored.get_mut(index) {
                *flag = true;
            }
        }
        let mut lines = Vec::new();
        let (mut source, mut line) = (0i64, 0i64);
        for generated_line in raw.mappings.split(';') {
            let mut segments = Vec::new();
            let mut column = 0i64;
            for segment in generated_line.split(',').filter(|s| !s.is_empty()) {
                let fields = vlq_decode(segment)?;
                column += fields.first().copied().unwrap_or(0);
                if fields.len() >= 4 {
                    source += fields[1];
                    line += fields[2];
                    // fields[3] is the source column, fields[4] the name: not kept.
                    if source >= 0 && line >= 0 && column >= 0 {
                        segments.push(Segment {
                            column: column as u32,
                            source: source as u32,
                            line: line as u32,
                        });
                    }
                }
            }
            segments.sort_by_key(|s| s.column);
            lines.push(segments);
        }
        Ok(Self {
            sources,
            ignored,
            lines,
        })
    }

    /// The original position of a generated `line` (1-based) and `column`
    /// (0-based, UTF-16 units): the last segment at or before the column.
    pub fn lookup(&self, line: u32, column: u32) -> Option<Original> {
        let segments = self.lines.get(line.checked_sub(1)? as usize)?;
        let at = segments.partition_point(|s| s.column <= column);
        let segment = segments.get(at.checked_sub(1)?)?;
        let source = self.sources.get(segment.source as usize)?;
        Some(Original {
            source: source.clone(),
            line: segment.line + 1,
            ignored: self.ignored[segment.source as usize],
        })
    }
}

/// The `//# sourceMappingURL=` of a script, if it has one (the last such
/// comment wins, as in browsers).
pub fn source_mapping_url(source_text: &str) -> Option<&str> {
    source_text
        .rsplit('\n')
        .find_map(|line| {
            let line = line.trim();
            line.strip_prefix("//# sourceMappingURL=")
                .or_else(|| line.strip_prefix("//@ sourceMappingURL="))
        })
        .map(str::trim)
        .filter(|url| !url.is_empty())
}

/// The script's source map, inline (`data:` URL) or fetched from next to
/// the script. `None`, with a debug log, when there is none or it fails:
/// the script is instrumented either way.
pub fn fetch(script_url: &str, source_text: &str) -> Option<SourceMap> {
    let map_url = source_mapping_url(source_text)?;
    let result = (|| -> Result<SourceMap> {
        let json = if let Some(data) = map_url.strip_prefix("data:") {
            let (_, payload) = data
                .split_once(',')
                .ok_or_else(|| anyhow!("malformed data URL"))?;
            if data.starts_with("application/json;base64,")
                || data.contains(";base64,")
            {
                String::from_utf8(BASE64_STANDARD.decode(payload)?)?
            } else {
                payload.to_string()
            }
        } else {
            let base = url::Url::parse(script_url)?;
            let url = base.join(map_url)?;
            reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()?
                .get(url)
                .send()?
                .error_for_status()?
                .text()?
        };
        SourceMap::parse(&json)
    })();
    match result {
        Ok(map) => Some(map),
        Err(error) => {
            log::debug!("no usable source map for {script_url}: {error:#}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_small_map() {
        // `a.js` line 1 col 0 -> src/a.ts line 1; line 1 col 4 -> src/b.ts line 3
        let json = r#"{"version":3,"sources":["src/a.ts","src/b.ts"],"names":[],
            "mappings":"AAAA,ICEA;AAAA","x_google_ignoreList":[1]}"#;
        let map = SourceMap::parse(json).unwrap();
        assert_eq!(
            map.lookup(1, 0),
            Some(Original { source: "src/a.ts".into(), line: 1, ignored: false })
        );
        assert_eq!(map.lookup(1, 4).unwrap().source, "src/b.ts");
        assert!(map.lookup(1, 4).unwrap().ignored);
        assert_eq!(map.lookup(1, 9).unwrap().source, "src/b.ts");
        assert_eq!(map.lookup(2, 0).unwrap().line, 3);
        assert_eq!(map.lookup(3, 0), None);
    }

    #[test]
    fn finds_the_mapping_url() {
        assert_eq!(
            source_mapping_url("x();\n//# sourceMappingURL=app.js.map\n"),
            Some("app.js.map")
        );
        assert_eq!(source_mapping_url("x();\n"), None);
    }
}
