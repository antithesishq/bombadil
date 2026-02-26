use std::path::Path;

use anyhow::Result;
use oxc_resolver::{self, Resolution, ResolveOptions, Resolver};

pub fn bundle(path: impl AsRef<Path>, specifier: &str) -> Result<Resolution> {
    let options = ResolveOptions::default();
    let resolver = Resolver::new(options);
    Ok(resolver.resolve(path, specifier)?)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_bundle() {
        assert_eq!(
            bundle("src/specification/bundler/fixtures", "./index.ts")
                .unwrap()
                .full_path(),
            PathBuf::from("src/specification/bundler/fixtures/index.ts")
        );
    }
}
