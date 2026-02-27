use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow, bail};
use oxc::{
    allocator::Allocator, ast::ast, parser::Parser, semantic::SemanticBuilder,
    span::SourceType,
};
use oxc_resolver::{ResolveOptions, Resolver};
use oxc_traverse::{Traverse, TraverseCtx, traverse_mut};

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct CanonicalPath(PathBuf);

impl Display for CanonicalPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.display().fmt(f)
    }
}

pub struct Modules {
    by_path: HashMap<CanonicalPath, Module>,
}

struct Module {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundlerError {
    ParseErrors(Vec<oxc::diagnostics::OxcDiagnostic>),
    SemanticErrors(Vec<oxc::diagnostics::OxcDiagnostic>),
}

impl From<BundlerError> for anyhow::Error {
    fn from(value: BundlerError) -> Self {
        anyhow!(value)
    }
}

impl Display for BundlerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BundlerError::ParseErrors(errors) => {
                write!(f, "Parse errors: {:?}", errors)
            }
            BundlerError::SemanticErrors(errors) => {
                write!(f, "Semantic errors: {:?}", errors)
            }
        }
    }
}

pub async fn bundle(
    path: impl AsRef<Path>,
    specifier: &str,
) -> Result<Modules> {
    let path: &Path = path.as_ref();
    let options = ResolveOptions::default();
    let resolver = Resolver::new(options);
    let allocator = Allocator::default();

    let mut modules = Modules {
        by_path: HashMap::new(),
    };
    let mut queue = VecDeque::new();
    queue.push_front(specifier);

    while let Some(specifier) = queue.pop_front() {
        let resolution = resolver.resolve(path, specifier)?;
        let canonical = CanonicalPath(resolution.full_path());

        let source_text =
            tokio::fs::read_to_string(resolution.full_path()).await?;
        let source_text = allocator.alloc_str(&source_text);

        let parser = Parser::new(
            &allocator,
            source_text,
            SourceType::from_path(resolution.full_path())?,
        );
        let result = parser.parse();
        if result.panicked {
            bail!(BundlerError::ParseErrors(result.errors.to_vec()));
        }
        let mut program = result.program;

        let semantic = SemanticBuilder::new()
            .with_check_syntax_error(true)
            .build(&program);
        if !semantic.errors.is_empty() {
            let errors = semantic.errors.to_vec();
            bail!(BundlerError::SemanticErrors(errors));
        }
        let scopes = semantic.semantic.into_scoping();

        let mut rewriter = Rewriter::default();
        traverse_mut(&mut rewriter, &allocator, &mut program, scopes, ());
        // TODO: handle cycles/duplicates
        queue.extend(rewriter.imports);
        modules.by_path.insert(canonical, Module {});
    }

    Ok(modules)
}

/// Rewrites a single module from ESM to CommonJS style, making it suitable for inclusion in the
/// bundle. Tracks what imports were made.
#[derive(Default)]
struct Rewriter<'a> {
    imports: HashSet<&'a str>,
}

impl<'a, 'b> Traverse<'b, ()> for Rewriter<'a>
where
    'b: 'a,
{
    fn enter_import_declaration(
        &mut self,
        import: &mut ast::ImportDeclaration<'b>,
        ctx: &mut TraverseCtx<'b, ()>,
    ) {
        log::info!("source: {:?}", import.source);
        self.imports.insert(import.source.value.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bundle() {
        let modules =
            bundle("src/specification/bundler/fixtures", "./index.ts")
                .await
                .unwrap();
        assert_eq!(
            modules
                .by_path
                .keys()
                .map(|path| path.to_string())
                .collect::<Vec<_>>(),
            vec![
                "src/specification/bundler/fixtures/index.ts",
                "src/specification/bundler/fixtures/other.ts",
            ],
        );
    }
}
