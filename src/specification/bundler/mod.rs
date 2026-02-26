use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
};

use anyhow::Result;
use oxc_resolver::{self, Resolution, ResolveOptions, Resolver};

struct Modules {
    by_path: HashMap<PathBuf, Module>,
}

struct Module {}

pub fn bundle(path: impl AsRef<Path>, specifier: &str) -> Result<Modules> {
    let path: &Path = path.as_ref();
    let options = ResolveOptions::default();
    let resolver = Resolver::new(options);

    let mut modules = Modules {
        by_path: HashMap::new(),
    };
    let mut queue = VecDeque::new();
    queue.push_front(specifier);

    while let Some(specifier) = queue.pop_front() {
        let resolution = resolver.resolve(path, specifier)?;
        modules.by_path.insert(resolution.full_path(), Module {});
    }

    Ok(modules)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_bundle() {
        let modules =
            bundle("src/specification/bundler/fixtures", "./index.ts").unwrap();
        assert_eq!(
            modules.by_path.keys().cloned().collect::<Vec<_>>(),
            vec![PathBuf::from("src/specification/bundler/fixtures/index.ts")],
        );
    }
}
