use std::{
    error::Error,
    fmt::Display,
    path::{Path, PathBuf},
};

use include_dir::{Dir, include_dir};
use oxc_resolver::{ResolveError, ResolveOptions};

static JS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/target/specification");

#[derive(Debug)]
pub enum ResolutionError {
    EmbeddedFileNotFound { path: PathBuf },
    InvalidUtf8 { path: PathBuf },
    ResolveError(ResolveError),
    IoError(std::io::Error),
}

impl Display for ResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolutionError::EmbeddedFileNotFound { path } => {
                write!(f, "embedded file not found: {}", path.display())
            }
            ResolutionError::InvalidUtf8 { path } => {
                write!(f, "invalid utf8 in file: {}", path.display())
            }
            ResolutionError::ResolveError(error) => error.fmt(f),
            ResolutionError::IoError(error) => error.fmt(f),
        }
    }
}

impl Error for ResolutionError {}

impl From<ResolveError> for ResolutionError {
    fn from(value: ResolveError) -> Self {
        Self::ResolveError(value)
    }
}

impl From<std::io::Error> for ResolutionError {
    fn from(value: std::io::Error) -> Self {
        Self::IoError(value)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Hash, Ord, Debug, Clone)]
pub enum ModuleKey {
    Embedded { specifier: String, path: PathBuf },
    OnDisk { specifier: String, path: PathBuf },
}

impl ModuleKey {
    pub fn specifier(&self) -> &str {
        match self {
            ModuleKey::Embedded { specifier, .. } => specifier,
            ModuleKey::OnDisk { specifier, .. } => specifier,
        }
    }
    pub fn path(&self) -> &Path {
        match self {
            ModuleKey::Embedded { path, .. } => path,
            ModuleKey::OnDisk { path, .. } => path,
        }
    }
    // NOTE: this needs to be sync in order for our boa_engine module
    // loader to have a non-async API, otherwise the verifier gets into
    // trouble with the boa_engine primitives not being Send.
    pub fn source_text(&self) -> Result<String, ResolutionError> {
        Ok(match self {
            ModuleKey::Embedded { path, .. } => JS_DIR
                .get_file(path)
                .ok_or(ResolutionError::EmbeddedFileNotFound {
                    path: path.clone(),
                })?
                .contents_utf8()
                .ok_or(ResolutionError::InvalidUtf8 { path: path.clone() })?
                .to_string(),
            ModuleKey::OnDisk { path, .. } => std::fs::read_to_string(path)?,
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
    ) -> Result<ModuleKey, ResolutionError> {
        if let Ok(relative) =
            PathBuf::from(specifier).strip_prefix("@antithesishq/bombadil")
        {
            if relative == "" {
                Ok(ModuleKey::Embedded {
                    specifier: specifier.to_string(),
                    path: PathBuf::from("index.js"),
                })
            } else {
                Ok(ModuleKey::Embedded {
                    specifier: specifier.to_string(),
                    path: relative
                        .strip_prefix("/")
                        .unwrap_or(relative)
                        .with_added_extension("js"),
                })
            }
        } else {
            let resolution = self.resolver.resolve(path, specifier)?;
            let path = resolution.full_path();
            Ok(ModuleKey::OnDisk {
                specifier: path
                    .to_str()
                    .ok_or(ResolutionError::InvalidUtf8 { path: path.clone() })?
                    .to_string(),
                path,
            })
        }
    }
}
