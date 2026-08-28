use std::fs;
use std::path::Path;

use crate::pdl::error::*;

/// PDL can contain `include` statements that can reference
/// other PDL files. This function resolves these includes
/// by reading the referenced file and returning full
/// resolved content.
pub fn resolve_pdl(path: &Path, input: &str) -> Result<String, Error> {
    let Some(dir) = path.parent() else {
        bail!("Failed to get parent directory");
    };

    let mut resolved = String::new();
    for line in input.lines() {
        if line.starts_with("include") {
            // Load the file
            let Some(name) = line.split_whitespace().nth(1) else {
                bail!("Failed to get file name from include statement");
            };
            let Ok(content) = fs::read_to_string(dir.join(name)) else {
                bail!("Failed to read file {}", name);
            };

            // Remove the license header
            let Some((_, content)) = content.split_once("\n\n") else {
                bail!("Failed to split license header from file {}", name);
            };

            resolved.push_str(content);
        } else {
            resolved.push_str(line);
        }
        resolved.push('\n');
    }

    Ok(resolved)
}
