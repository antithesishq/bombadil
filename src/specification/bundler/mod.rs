use std::{
    collections::{BTreeSet, VecDeque},
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow, bail};
use oxc::{
    allocator::{Allocator, TakeIn},
    ast::{NONE, ast},
    codegen::Codegen,
    parser::Parser,
    semantic::SemanticBuilder,
    span::{SPAN, SourceType},
    transformer::{TransformOptions, Transformer},
};
use oxc_resolver::{ResolveOptions, Resolver};
use oxc_traverse::{Traverse, TraverseCtx, traverse_mut};

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone)]
pub struct CanonicalPath {
    path: PathBuf,
}

impl Display for CanonicalPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.path.display().fmt(f)
    }
}

pub struct Module {
    path: CanonicalPath,
    code: String,
}

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

pub async fn bundle(path: impl AsRef<Path>, specifier: &str) -> Result<String> {
    let path: &Path = path.as_ref();
    let options = ResolveOptions::default();
    let resolver = Resolver::new(options);
    let allocator = Allocator::default();

    let mut modules = vec![];
    let mut paths_processed = BTreeSet::<CanonicalPath>::new();
    let mut queue = VecDeque::new();

    queue.push_front(CanonicalPath {
        path: resolver.resolve(path, specifier)?.full_path(),
    });

    while let Some(canonical) = queue.pop_front() {
        eprintln!("processing {:?}", &canonical.path);
        if paths_processed.contains(&canonical) {
            eprintln!("already processed, skipping {:?}", &canonical.path);
            continue;
        }

        let source_text = tokio::fs::read_to_string(&canonical.path).await?;
        let source_text = allocator.alloc_str(&source_text);

        let parser = Parser::new(
            &allocator,
            source_text,
            SourceType::from_path(&canonical.path)?,
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
        let mut state = RewriterState {
            imports: BTreeSet::new(),
            export_statements: oxc::allocator::Vec::new_in(&allocator),
            resolver: &resolver,
            base_path: path,
            resolution_errors: Vec::new(),
        };
        traverse_mut(
            &mut rewriter,
            &allocator,
            &mut program,
            scopes,
            &mut state,
        );

        if !state.resolution_errors.is_empty() {
            bail!(
                "Failed to resolve imports in {:?}:\n  {}",
                canonical.path,
                state.resolution_errors.join("\n  ")
            );
        }

        program.body.append(&mut state.export_statements);

        for import_canonical in state.imports {
            if !paths_processed.contains(&import_canonical) {
                queue.push_back(import_canonical);
            }
        }

        let transform_options = TransformOptions {
            typescript: oxc::transformer::TypeScriptOptions {
                only_remove_type_imports: true,
                allow_namespaces: true,
                remove_class_fields_without_initializer: false,
                rewrite_import_extensions: None,
                ..Default::default()
            },
            ..Default::default()
        };

        // Must we do this again after traversal?
        let semantic = SemanticBuilder::new()
            .with_check_syntax_error(true)
            .build(&program);
        if !semantic.errors.is_empty() {
            let errors = semantic.errors.to_vec();
            bail!(BundlerError::SemanticErrors(errors));
        }
        let scopes = semantic.semantic.into_scoping();

        let transformer =
            Transformer::new(&allocator, &canonical.path, &transform_options);
        transformer.build_with_scoping(scopes, &mut program);

        eprintln!("done processing {:?}", &canonical.path);
        let codegen = Codegen::new().build(&program);
        modules.push(Module {
            path: canonical.clone(),
            code: codegen.code,
        });
        paths_processed.insert(canonical);
    }

    let code: String = modules
        .iter()
        .map(|module| format!("// {} \n{}\n", module.path, module.code))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(code)
}

/// Rewrites a single module from ESM to CommonJS style, making it suitable for
/// inclusion in the bundle. Tracks what imports were made.
///
/// NOTE: Semantic differences from ESM:
/// - Exports are added at the END of the module, not inline with declarations.
///   This means side effects run before exports are set up, unlike ESM where
///   exports are hoisted.
/// - No live bindings: exported values are snapshots, mutations are not visible
///   to importers (unlike ESM where `export let x` creates a live binding).
/// - For a browser bundle where everything runs synchronously in dependency order,
///   these differences are acceptable.
#[derive(Default)]
struct Rewriter {}

struct RewriterState<'a> {
    imports: BTreeSet<CanonicalPath>,
    export_statements: oxc::allocator::Vec<'a, ast::Statement<'a>>,
    resolver: &'a Resolver,
    base_path: &'a Path,
    resolution_errors: Vec<String>,
}

impl<'a, 'b> Traverse<'a, &'b mut RewriterState<'a>> for Rewriter
where
    'a: 'b,
{
    fn enter_statement(
        &mut self,
        statement: &mut ast::Statement<'a>,
        ctx: &mut TraverseCtx<'a, &'b mut RewriterState<'a>>,
    ) {
        match statement {
            ast::Statement::ImportDeclaration(import_declaration) => {
                let source_specifier = import_declaration.source.value.as_str();

                let canonical = match ctx
                    .state
                    .resolver
                    .resolve(ctx.state.base_path, source_specifier)
                {
                    Ok(resolved) => CanonicalPath {
                        path: resolved.full_path(),
                    },
                    Err(e) => {
                        ctx.state.resolution_errors.push(format!(
                            "Cannot resolve '{}': {}",
                            source_specifier, e
                        ));
                        return;
                    }
                };
                ctx.state.imports.insert(canonical.clone());

                let canonical_str = ctx
                    .ast
                    .allocator
                    .alloc_str(&canonical.path.display().to_string());
                let require_call = ctx.ast.expression_call(
                    SPAN,
                    ctx.ast.expression_identifier(SPAN, "require"),
                    Option::None::<
                        oxc::allocator::Box<
                            '_,
                            ast::TSTypeParameterInstantiation<'_>,
                        >,
                    >,
                    ctx.ast.vec1(ast::Argument::StringLiteral(ctx.ast.alloc(
                        ctx.ast.string_literal(SPAN, canonical_str, None),
                    ))),
                    false,
                );

                let binding_pattern = if let Some(specifiers) =
                    &import_declaration.specifiers
                {
                    let mut properties =
                        ctx.ast.vec_with_capacity(specifiers.len());
                    for specifier in specifiers {
                        match specifier {
                            ast::ImportDeclarationSpecifier::ImportSpecifier(import_specifier ) => {
                                let imported = &import_specifier.imported;
                                let local = &import_specifier.local;
                                match import_specifier.import_kind{
                                    ast::ImportOrExportKind::Value => {
                                        properties.push(
                                            ctx.ast.binding_property(
                                                SPAN,
                                                ctx.ast.property_key_static_identifier(SPAN, imported.name()),
                                                ctx.ast.binding_pattern_binding_identifier(SPAN, local.name),
                                                false,
                                                false
                                            )
                                        );
                                    },
                                    ast::ImportOrExportKind::Type => return,
                                }
                            },
                            ast::ImportDeclarationSpecifier::ImportDefaultSpecifier(import_default_specifier) => {
                                eprintln!("const {{ default: {} }} = require({:?});", import_default_specifier.local, source_specifier);

                            },
                            ast::ImportDeclarationSpecifier::ImportNamespaceSpecifier(import_namespace_specifier) => {
                                eprintln!("const {} = require({:?});", import_namespace_specifier.local, source_specifier);
                            },
                        }
                    }
                    ctx.ast.binding_pattern_object_pattern(
                        SPAN,
                        properties,
                        Option::None::<
                            oxc::allocator::Box<'_, ast::BindingRestElement>,
                        >,
                    )
                } else {
                    return;
                };

                *statement = ast::Statement::VariableDeclaration(
                    ctx.ast
                        .variable_declaration(
                            SPAN,
                            ast::VariableDeclarationKind::Const,
                            ctx.ast.vec1(ctx.ast.variable_declarator(
                                SPAN,
                                ast::VariableDeclarationKind::Const,
                                binding_pattern,
                                Option::None::<
                                    oxc::allocator::Box<
                                        'a,
                                        ast::TSTypeAnnotation,
                                    >,
                                >,
                                Some(require_call),
                                false,
                            )),
                            false,
                        )
                        .take_in_box(ctx.ast.allocator),
                );
            }
            ast::Statement::ExportAllDeclaration(export_all_declaration) => {
                let source_specifier =
                    export_all_declaration.source.value.as_str();

                let canonical = match ctx
                    .state
                    .resolver
                    .resolve(ctx.state.base_path, source_specifier)
                {
                    Ok(resolved) => CanonicalPath {
                        path: resolved.full_path(),
                    },
                    Err(e) => {
                        ctx.state.resolution_errors.push(format!(
                            "Cannot resolve '{}': {}",
                            source_specifier, e
                        ));
                        return;
                    }
                };
                ctx.state.imports.insert(canonical.clone());

                let canonical_str = ctx
                    .ast
                    .allocator
                    .alloc_str(&canonical.path.display().to_string());

                let require_call = ctx.ast.expression_call(
                    SPAN,
                    ctx.ast.expression_identifier(SPAN, "require"),
                    NONE,
                    ctx.ast.vec1(ast::Argument::StringLiteral(ctx.ast.alloc(
                        ctx.ast.string_literal(SPAN, canonical_str, None),
                    ))),
                    false,
                );

                let module_exports = ctx.ast.member_expression_static(
                    SPAN,
                    ctx.ast.expression_identifier(SPAN, "module"),
                    ctx.ast.identifier_name(SPAN, "exports"),
                    false,
                );

                let module_exports_expr: ast::Expression =
                    module_exports.into();
                let object_assign_call = ctx.ast.expression_call(
                    SPAN,
                    ctx.ast
                        .member_expression_static(
                            SPAN,
                            ctx.ast.expression_identifier(SPAN, "Object"),
                            ctx.ast.identifier_name(SPAN, "assign"),
                            false,
                        )
                        .into(),
                    NONE,
                    ctx.ast.vec_from_iter([
                        ast::Argument::from(module_exports_expr),
                        ast::Argument::from(require_call),
                    ]),
                    false,
                );

                *statement =
                    ctx.ast.statement_expression(SPAN, object_assign_call);
            }
            ast::Statement::ExportDefaultDeclaration(
                export_default_declaration,
            ) => {
                let module_exports = ctx.ast.member_expression_static(
                    SPAN,
                    ctx.ast.expression_identifier(SPAN, "module"),
                    ctx.ast.identifier_name(SPAN, "exports"),
                    false,
                );

                let member_expr = ctx.ast.member_expression_static(
                    SPAN,
                    module_exports.into(),
                    ctx.ast.identifier_name(SPAN, "default"),
                    false,
                );

                let declaration_expr = match &mut export_default_declaration.declaration {
                    ast::ExportDefaultDeclarationKind::FunctionDeclaration(func) => {
                        ast::Expression::FunctionExpression(func.take_in_box(ctx.ast.allocator))
                    }
                    ast::ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                        ast::Expression::ClassExpression(class.take_in_box(ctx.ast.allocator))
                    }
                    ast::ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => {
                        return;
                    }
                    expr => {
                        expr.to_expression_mut().take_in(ctx.ast.allocator)
                    }
                };

                let assignment = ctx.ast.expression_assignment(
                    SPAN,
                    ast::AssignmentOperator::Assign,
                    member_expr.into(),
                    declaration_expr,
                );

                *statement = ctx.ast.statement_expression(SPAN, assignment);
            }
            ast::Statement::ExportNamedDeclaration(
                export_named_declaration,
            ) => match export_named_declaration.export_kind {
                ast::ImportOrExportKind::Value => {
                    if let Some(declaration) =
                        &mut export_named_declaration.declaration
                    {
                        match declaration {
                            ast::Declaration::VariableDeclaration(
                                variable_declaration,
                            ) => {
                                for declarator in
                                    &variable_declaration.declarations
                                {
                                    let mut queue = VecDeque::new();
                                    queue.push_front(&declarator.id);

                                    while let Some(id) = queue.pop_front() {
                                        match id {
                                                ast::BindingPattern::BindingIdentifier(binding_identifier) => {
                                                    let export_statement = commonjs_export_name(binding_identifier.name, ctx);
                                                    ctx.state.export_statements.push(export_statement);
                                                },
                                                ast::BindingPattern::ObjectPattern(object_pattern) => {
                                                    for property in &object_pattern.properties {
                                                        queue.push_back(&property.value);
                                                    }
                                                },
                                                ast::BindingPattern::ArrayPattern(array_pattern) => {
                                                    for pattern in (&array_pattern.elements).into_iter().flatten() {
                                                        queue.push_back(pattern);
                                                    }
                                                },
                                                ast::BindingPattern::AssignmentPattern(assignment_pattern) => {
                                                    queue.push_back(&assignment_pattern.left)
                                                },
                                            }
                                    }
                                }
                                *statement =
                                    ast::Statement::VariableDeclaration(
                                        variable_declaration
                                            .take_in_box(ctx.ast.allocator),
                                    );
                            }
                            ast::Declaration::FunctionDeclaration(function) => {
                                let export_statement = commonjs_export_name(
                                    function.name().expect(
                                        "cannot export function without a name",
                                    ),
                                    ctx,
                                );
                                ctx.state
                                    .export_statements
                                    .push(export_statement);
                                *statement =
                                    ast::Statement::FunctionDeclaration(
                                        function.take_in_box(ctx.ast.allocator),
                                    );
                            }
                            ast::Declaration::ClassDeclaration(class) => {
                                let export_statement = commonjs_export_name(
                                    class.name().expect(
                                        "cannot export class without a name",
                                    ),
                                    ctx,
                                );
                                ctx.state
                                    .export_statements
                                    .push(export_statement);
                                *statement = ast::Statement::ClassDeclaration(
                                    class.take_in_box(ctx.ast.allocator),
                                );
                            }
                            ast::Declaration::TSTypeAliasDeclaration(_)
                            | ast::Declaration::TSInterfaceDeclaration(_)
                            | ast::Declaration::TSEnumDeclaration(_)
                            | ast::Declaration::TSModuleDeclaration(_)
                            | ast::Declaration::TSGlobalDeclaration(_)
                            | ast::Declaration::TSImportEqualsDeclaration(_) => {
                            }
                        }
                    } else if export_named_declaration.source.is_none() {
                        panic!(
                            "unsupported export: {:?}",
                            export_named_declaration
                        );
                    }
                }
                ast::ImportOrExportKind::Type => {}
            },
            _ => {}
        }
    }

    fn exit_statement(
        &mut self,
        statement: &mut ast::Statement<'a>,
        _ctx: &mut TraverseCtx<'a, &'b mut RewriterState<'a>>,
    ) {
        if let ast::Statement::ExportNamedDeclaration(export_named_declaration) =
            statement
            && export_named_declaration.source.is_some()
            && export_named_declaration.export_kind
                == ast::ImportOrExportKind::Value
        {
            // TODO: Implement re-exports (export { x } from "./other.ts")
            // Currently causes stack overflow during AST construction - unclear why
            // Re-export statement is left unmodified for now
        }
    }
}

fn commonjs_export_name<'a>(
    name: oxc::span::Ident<'a>,
    ctx: &mut TraverseCtx<'a, &mut RewriterState<'a>>,
) -> ast::Statement<'a> {
    let module_exports = ctx.ast.member_expression_static(
        SPAN,
        ctx.ast.expression_identifier(SPAN, "module"),
        ctx.ast.identifier_name(SPAN, "exports"),
        false,
    );

    let member_expr = ctx.ast.member_expression_static(
        SPAN,
        module_exports.into(),
        ctx.ast.identifier_name(SPAN, name),
        false,
    );

    let assignment = ctx.ast.expression_assignment(
        SPAN,
        ast::AssignmentOperator::Assign,
        member_expr.into(),
        ctx.ast.expression_identifier(SPAN, name),
    );

    ctx.ast.statement_expression(SPAN, assignment)
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    #[tokio::test]
    async fn test_bundle() {
        let bundle = bundle("src/specification/bundler/fixtures", "./index.ts")
            .await
            .unwrap();
        assert_snapshot!(bundle);
    }
}
