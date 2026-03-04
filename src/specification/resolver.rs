use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use include_dir::{Dir, include_dir};
use oxc_resolver::ResolveOptions;

static JS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/target/specification");

#[derive(PartialEq, Eq, PartialOrd, Hash, Ord, Debug, Clone)]
pub enum ModuleKey {
    Embedded(String, PathBuf),
    OnDisk(String, PathBuf),
}

impl ModuleKey {
    pub fn specifier(&self) -> &str {
        match self {
            ModuleKey::Embedded(specifier, _path) => specifier,
            ModuleKey::OnDisk(specifier, _path) => specifier,
        }
    }
    pub fn path(&self) -> &Path {
        match self {
            ModuleKey::Embedded(_specifier, path) => path,
            ModuleKey::OnDisk(_specifier, path) => path,
        }
    }
    pub async fn source_text(&self) -> Result<String> {
        Ok(match self {
            ModuleKey::Embedded(_, path) => JS_DIR
                .get_file(path)
                .ok_or(anyhow!(
                    "embedded module at {} cannot be resolved",
                    &path.display()
                ))?
                .contents_utf8()
                .ok_or(anyhow!(
                    "embedded module is not valid utf8: {}",
                    path.display()
                ))?
                .to_string(),
            ModuleKey::OnDisk(_, path) => {
                tokio::fs::read_to_string(&path).await?
            }
        })
    }
}

pub struct Resolver {
    resolver: oxc_resolver::Resolver,
}

impl Resolver {
    pub fn new(options: ResolveOptions) -> Self {
        Self {
            resolver: oxc_resolver::Resolver::new(options),
        }
    }

    pub fn resolve(
        &self,
        path: impl AsRef<Path>,
        specifier: &str,
    ) -> Result<ModuleKey> {
        if let Ok(relative) =
            PathBuf::from(specifier).strip_prefix("@antithesishq/bombadil")
        {
            if relative == "" {
                Ok(ModuleKey::Embedded(
                    specifier.to_string(),
                    PathBuf::from("index.js"),
                ))
            } else {
                Ok(ModuleKey::Embedded(
                    specifier.to_string(),
                    relative
                        .strip_prefix("/")
                        .unwrap_or(relative)
                        .with_added_extension("js"),
                ))
            }
        } else {
            let resolution = self.resolver.resolve(path, specifier)?;
            let path = resolution.full_path();
            Ok(ModuleKey::OnDisk(
                path.to_str()
                    .ok_or(anyhow!(
                        "resolved path is not valid utf8: {}",
                        path.display()
                    ))?
                    .to_string(),
                path,
            ))
        }
    }
}
