use std::{
    collections::{BTreeSet, VecDeque},
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow, bail};
use include_dir::{Dir, include_dir};
use oxc::{
    allocator::{Allocator, TakeIn},
    ast::{NONE, ast},
    codegen::Codegen,
    parser::Parser,
    semantic::SemanticBuilder,
    span::{SPAN, SourceType},
    transformer::{TransformOptions, Transformer},
};
use oxc_resolver::ResolveOptions;
use oxc_traverse::{Traverse, TraverseCtx, traverse_mut};

static JS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/target/specification");

pub struct Module {
    key: ModuleKey,
    code: String,
}

#[derive(PartialEq, Eq, PartialOrd, Hash, Ord, Debug, Clone)]
enum ModuleKey {
    Embedded(String, PathBuf),
    OnDisk(String, PathBuf),
}

impl ModuleKey {
    fn specifier(&self) -> &str {
        match self {
            ModuleKey::Embedded(specifier, _path) => specifier,
            ModuleKey::OnDisk(specifier, _path) => specifier,
        }
    }
    fn path(&self) -> &Path {
        match self {
            ModuleKey::Embedded(_specifier, path) => path,
            ModuleKey::OnDisk(_specifier, path) => path,
        }
    }
}

struct Resolver {
    resolver: oxc_resolver::Resolver,
}
impl Resolver {
    pub fn new(options: ResolveOptions) -> Self {
        Self {
            resolver: oxc_resolver::Resolver::new(options),
        }
    }

    fn resolve(
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
    let mut keys_processed = BTreeSet::<ModuleKey>::new();
    let mut queue = VecDeque::new();

    queue.push_front(resolver.resolve(path, specifier)?);

    while let Some(key) = queue.pop_front() {
        if keys_processed.contains(&key) {
            continue;
        }

        let source_text = match &key {
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
        };

        let source_text = allocator.alloc_str(&source_text);

        let parser = Parser::new(
            &allocator,
            source_text,
            SourceType::from_path(key.path())?,
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
            resolution_errors: Vec::new(),
            key: key.clone(),
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
                key.path(),
                state.resolution_errors.join("\n  ")
            );
        }

        program.body.append(&mut state.export_statements);

        for import_canonical in state.imports {
            if !keys_processed.contains(&import_canonical) {
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

        let semantic = SemanticBuilder::new()
            .with_check_syntax_error(true)
            .build(&program);
        if !semantic.errors.is_empty() {
            let errors = semantic.errors.to_vec();
            bail!(BundlerError::SemanticErrors(errors));
        }
        let scopes = semantic.semantic.into_scoping();

        let transformer =
            Transformer::new(&allocator, key.path(), &transform_options);
        transformer.build_with_scoping(scopes, &mut program);

        let codegen = Codegen::new().build(&program);
        modules.push(Module {
            key: key.clone(),
            code: codegen.code,
        });
        keys_processed.insert(key);
    }

    let mut bundle = String::from(
        r#"(function() {
  const modules = {};
  const cache = {};

  function require(path) {
    if (cache[path]) {
      return cache[path].exports;
    }

    const module = { exports: {} };
    cache[path] = module;

    if (!modules[path]) {
      throw new Error("Module not found: " + path);
    }

    modules[path](module, module.exports, require);
    return module.exports;
  }

"#,
    );

    for module in &modules {
        bundle.push_str(&format!(
            "  modules[{:?}] = function(module, exports, require) {{\n",
            module.key.specifier()
        ));
        for line in module.code.lines() {
            bundle.push_str("    ");
            bundle.push_str(line);
            bundle.push('\n');
        }
        bundle.push_str("  };\n\n");
    }

    if let Some(entry) = modules.first() {
        bundle.push_str(&format!("  require({:?});\n", entry.key.specifier()));
    }

    bundle.push_str("})();\n");

    Ok(bundle)
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
    imports: BTreeSet<ModuleKey>,
    export_statements: oxc::allocator::Vec<'a, ast::Statement<'a>>,
    resolver: &'a Resolver,
    key: ModuleKey,
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

                let Some(key) = resolve_import(source_specifier, ctx) else {
                    return;
                };
                ctx.state.imports.insert(key.clone());

                let require_call = build_require_call(&key, ctx);

                let specifiers = match &import_declaration.specifiers {
                    Some(s) => s,
                    None => {
                        *statement =
                            ctx.ast.statement_expression(SPAN, require_call);
                        return;
                    }
                };

                let namespace_import = specifiers.iter().find_map(|s| match s {
                    ast::ImportDeclarationSpecifier::ImportNamespaceSpecifier(ns) => Some(ns),
                    _ => None,
                });

                let binding_pattern = if let Some(ns) = namespace_import {
                    if specifiers.len() > 1 {
                        panic!(
                            "Cannot mix namespace import with other imports"
                        );
                    }
                    ctx.ast
                        .binding_pattern_binding_identifier(SPAN, ns.local.name)
                } else {
                    let mut properties =
                        ctx.ast.vec_with_capacity(specifiers.len());
                    for specifier in specifiers {
                        match specifier {
                            ast::ImportDeclarationSpecifier::ImportSpecifier(import_specifier) => {
                                let imported = &import_specifier.imported;
                                let local = &import_specifier.local;
                                match import_specifier.import_kind {
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
                                    ast::ImportOrExportKind::Type => continue,
                                }
                            },
                            ast::ImportDeclarationSpecifier::ImportDefaultSpecifier(import_default_specifier) => {
                                properties.push(
                                    ctx.ast.binding_property(
                                        SPAN,
                                        ctx.ast.property_key_static_identifier(SPAN, "default"),
                                        ctx.ast.binding_pattern_binding_identifier(SPAN, import_default_specifier.local.name),
                                        false,
                                        false
                                    )
                                );
                            },
                            ast::ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => {
                                unreachable!("handled above");
                            },
                        }
                    }
                    if properties.is_empty() {
                        *statement =
                            ctx.ast.statement_expression(SPAN, require_call);
                        return;
                    }
                    ctx.ast
                        .binding_pattern_object_pattern(SPAN, properties, NONE)
                };

                *statement =
                    build_const_declaration(binding_pattern, require_call, ctx);
            }
            ast::Statement::ExportAllDeclaration(export_all_declaration) => {
                let source_specifier =
                    export_all_declaration.source.value.as_str();

                let Some(key) = resolve_import(source_specifier, ctx) else {
                    return;
                };
                ctx.state.imports.insert(key.clone());

                let require_call = build_require_call(&key, ctx);
                let module_exports = build_module_exports(ctx);

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
                        ast::Argument::from(ast::Expression::from(
                            module_exports,
                        )),
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
                let declaration_expression = match &mut export_default_declaration.declaration {
                    ast::ExportDefaultDeclarationKind::FunctionDeclaration(func) => {
                        ast::Expression::FunctionExpression(func.take_in_box(ctx.ast.allocator))
                    }
                    ast::ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                        ast::Expression::ClassExpression(class.take_in_box(ctx.ast.allocator))
                    }
                    ast::ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => {
                        return;
                    }
                    expression => {
                        expression.to_expression_mut().take_in(ctx.ast.allocator)
                    }
                };

                *statement = build_module_exports_assignment(
                    "default",
                    declaration_expression,
                    ctx,
                );
            }
            ast::Statement::ExportNamedDeclaration(
                export_named_declaration,
            ) => match export_named_declaration.export_kind {
                ast::ImportOrExportKind::Type => {
                    *statement = ctx.ast.statement_empty(SPAN);
                }
                ast::ImportOrExportKind::Value => {
                    if export_named_declaration.declaration.is_none()
                        && export_named_declaration.specifiers.is_empty()
                        && export_named_declaration.source.is_none()
                    {
                        *statement = ctx.ast.statement_empty(SPAN);
                        return;
                    }
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
                                *statement = ctx.ast.statement_empty(SPAN);
                            }
                        }
                    } else if let Some(source) =
                        &export_named_declaration.source
                    {
                        let source_specifier = source.value.as_str();

                        let Some(key) = resolve_import(source_specifier, ctx)
                        else {
                            return;
                        };
                        ctx.state.imports.insert(key.clone());

                        let require_call = build_require_call(&key, ctx);

                        let mut properties = ctx.ast.vec_with_capacity(
                            export_named_declaration.specifiers.len(),
                        );
                        for export_specifier in
                            &export_named_declaration.specifiers
                        {
                            let local_name = export_specifier.local.name();
                            let exported_name =
                                export_specifier.exported.name();

                            properties.push(ctx.ast.binding_property(
                                SPAN,
                                ctx.ast.property_key_static_identifier(
                                    SPAN, local_name,
                                ),
                                ctx.ast.binding_pattern_binding_identifier(
                                    SPAN, local_name,
                                ),
                                false,
                                false,
                            ));

                            let export_statement =
                                build_module_exports_assignment(
                                    exported_name.as_str(),
                                    ctx.ast.expression_identifier(
                                        SPAN, local_name,
                                    ),
                                    ctx,
                                );
                            ctx.state.export_statements.push(export_statement);
                        }

                        let binding_pattern =
                            ctx.ast.binding_pattern_object_pattern(
                                SPAN, properties, NONE,
                            );

                        *statement = build_const_declaration(
                            binding_pattern,
                            require_call,
                            ctx,
                        );
                    } else {
                        for export_specifier in
                            &export_named_declaration.specifiers
                        {
                            let local_name = export_specifier.local.name();
                            let exported_name =
                                export_specifier.exported.name();

                            let export_statement =
                                build_module_exports_assignment(
                                    exported_name.as_str(),
                                    ctx.ast.expression_identifier(
                                        SPAN, local_name,
                                    ),
                                    ctx,
                                );
                            ctx.state.export_statements.push(export_statement);
                        }
                        *statement = ctx.ast.statement_empty(SPAN);
                    }
                }
            },
            _ => {}
        }
    }
}

fn build_require_call<'a>(
    key: &ModuleKey,
    ctx: &mut TraverseCtx<'a, &mut RewriterState<'a>>,
) -> ast::Expression<'a> {
    let key_string = ctx.ast.allocator.alloc_str(key.specifier());
    ctx.ast.expression_call(
        SPAN,
        ctx.ast.expression_identifier(SPAN, "require"),
        NONE,
        ctx.ast.vec1(ast::Argument::StringLiteral(
            ctx.ast
                .alloc(ctx.ast.string_literal(SPAN, key_string, None)),
        )),
        false,
    )
}

fn build_module_exports<'a>(
    ctx: &mut TraverseCtx<'a, &mut RewriterState<'a>>,
) -> ast::MemberExpression<'a> {
    ctx.ast.member_expression_static(
        SPAN,
        ctx.ast.expression_identifier(SPAN, "module"),
        ctx.ast.identifier_name(SPAN, "exports"),
        false,
    )
}

fn build_module_exports_assignment<'a>(
    export_name: &'a str,
    value: ast::Expression<'a>,
    ctx: &mut TraverseCtx<'a, &mut RewriterState<'a>>,
) -> ast::Statement<'a> {
    let module_exports = build_module_exports(ctx);
    let member_expression = ctx.ast.member_expression_static(
        SPAN,
        module_exports.into(),
        ctx.ast.identifier_name(SPAN, export_name),
        false,
    );
    let assignment = ctx.ast.expression_assignment(
        SPAN,
        ast::AssignmentOperator::Assign,
        member_expression.into(),
        value,
    );
    ctx.ast.statement_expression(SPAN, assignment)
}

fn resolve_import<'a>(
    source_specifier: &str,
    ctx: &mut TraverseCtx<'a, &mut RewriterState<'a>>,
) -> Option<ModuleKey> {
    match ctx.state.resolver.resolve(
        ctx.state
            .key
            .path()
            .parent()
            .expect("no parent to resolve from"),
        source_specifier,
    ) {
        Ok(key) => Some(key),
        Err(e) => {
            ctx.state
                .resolution_errors
                .push(format!("Cannot resolve '{}': {}", source_specifier, e));
            None
        }
    }
}

fn build_const_declaration<'a>(
    binding_pattern: ast::BindingPattern<'a>,
    init: ast::Expression<'a>,
    ctx: &mut TraverseCtx<'a, &mut RewriterState<'a>>,
) -> ast::Statement<'a> {
    ast::Statement::VariableDeclaration(
        ctx.ast
            .variable_declaration(
                SPAN,
                ast::VariableDeclarationKind::Const,
                ctx.ast.vec1(ctx.ast.variable_declarator(
                    SPAN,
                    ast::VariableDeclarationKind::Const,
                    binding_pattern,
                    NONE,
                    Some(init),
                    false,
                )),
                false,
            )
            .take_in_box(ctx.ast.allocator),
    )
}

fn commonjs_export_name<'a>(
    name: oxc::span::Ident<'a>,
    ctx: &mut TraverseCtx<'a, &mut RewriterState<'a>>,
) -> ast::Statement<'a> {
    build_module_exports_assignment(
        name.as_str(),
        ctx.ast.expression_identifier(SPAN, name),
        ctx,
    )
}

#[cfg(test)]
mod tests {
    use boa_engine::{JsValue, NativeFunction, Source};
    use insta::assert_snapshot;

    use super::*;

    #[tokio::test]
    async fn test_bundle() {
        let bundle =
            bundle("src/specification/bundler/fixtures/snapshot", "./index.ts")
                .await
                .unwrap();
        assert_snapshot!(bundle);
    }

    async fn eval_module_with_logging(
        entry_path: impl AsRef<Path>,
    ) -> Result<Vec<String>> {
        use crate::specification::{
            module_loader::{load_modules, transpile},
            result::SpecificationError,
        };
        use boa_engine::{
            context::ContextBuilder, js_string, module::SimpleModuleLoader,
            object::builtins::JsArray,
        };
        use std::rc::Rc;

        let entry_path = entry_path.as_ref();

        let loader = Rc::new(SimpleModuleLoader::new(".").map_err(|e| {
            SpecificationError::JS(format!("Failed to create loader: {}", e))
        })?);

        let mut context = ContextBuilder::default()
            .module_loader(loader.clone())
            .build()
            .map_err(|e| SpecificationError::JS(e.to_string()))?;

        let logs_array = JsArray::new(&mut context);
        context
            .register_global_property(
                js_string!("__logs"),
                logs_array.clone(),
                boa_engine::property::Attribute::all(),
            )
            .unwrap();

        context
            .register_global_builtin_callable(
                js_string!("log"),
                1,
                NativeFunction::from_copy_closure(|_, args, context| {
                    if let Some(arg) = args.first() {
                        let logs = context
                            .global_object()
                            .get(js_string!("__logs"), context)?;
                        if let Some(obj) = logs.as_object() {
                            let len = obj
                                .get(js_string!("length"), context)?
                                .to_length(context)?;
                            obj.set(len, arg.clone(), true, context)?;
                        }
                    }
                    Ok(JsValue::undefined())
                }),
            )
            .unwrap();

        let source_text = tokio::fs::read_to_string(&entry_path).await?;
        let source_type = SourceType::from_path(entry_path)?;
        let js_source = if source_type == SourceType::mjs() {
            source_text
        } else {
            transpile(&source_text, entry_path, &source_type)?
        };

        let source =
            Source::from_reader(js_source.as_bytes(), Some(entry_path));
        let module = boa_engine::Module::parse(source, None, &mut context)
            .map_err(|e| {
                SpecificationError::JS(format!("Failed to parse module: {}", e))
            })?;

        load_modules(&mut context, &[module])?;

        let logs_value = context
            .global_object()
            .get(js_string!("__logs"), &mut context)
            .unwrap();
        let logs_obj = logs_value.as_object().unwrap();
        let len = logs_obj
            .get(js_string!("length"), &mut context)
            .unwrap()
            .to_u32(&mut context)
            .unwrap();

        Ok((0..len)
            .map(|i| {
                logs_obj
                    .get(i, &mut context)
                    .unwrap()
                    .to_string(&mut context)
                    .unwrap()
                    .to_std_string_escaped()
            })
            .collect())
    }

    async fn eval_script_with_logging(code: &str) -> Result<Vec<String>> {
        use crate::specification::result::SpecificationError;
        use boa_engine::{Context, js_string, object::builtins::JsArray};

        let mut context = Context::default();

        let logs_array = JsArray::new(&mut context);
        context
            .register_global_property(
                js_string!("__logs"),
                logs_array.clone(),
                boa_engine::property::Attribute::all(),
            )
            .map_err(|e| {
                SpecificationError::JS(format!(
                    "Failed to register __logs: {}",
                    e
                ))
            })?;

        context
            .register_global_builtin_callable(
                js_string!("log"),
                1,
                NativeFunction::from_copy_closure(|_, args, context| {
                    if let Some(arg) = args.first() {
                        let logs = context
                            .global_object()
                            .get(js_string!("__logs"), context)?;
                        if let Some(obj) = logs.as_object() {
                            let len = obj
                                .get(js_string!("length"), context)?
                                .to_length(context)?;
                            obj.set(len, arg.clone(), true, context)?;
                        }
                    }
                    Ok(JsValue::undefined())
                }),
            )
            .map_err(|e| {
                SpecificationError::JS(format!("Failed to register log: {}", e))
            })?;

        context.eval(Source::from_bytes(code)).map_err(|e| {
            SpecificationError::JS(format!("Eval failed: {}", e))
        })?;

        let logs_value = context
            .global_object()
            .get(js_string!("__logs"), &mut context)
            .map_err(|e| {
                SpecificationError::JS(format!("Failed to get logs: {}", e))
            })?;
        let logs_obj = logs_value.as_object().ok_or_else(|| {
            SpecificationError::JS("__logs is not an object".to_string())
        })?;
        let len = logs_obj
            .get(js_string!("length"), &mut context)
            .map_err(|e| {
                SpecificationError::JS(format!("Failed to get length: {}", e))
            })?
            .to_u32(&mut context)
            .map_err(|e| {
                SpecificationError::JS(format!(
                    "Failed to convert length: {}",
                    e
                ))
            })?;

        Ok((0..len)
            .map(|i| {
                logs_obj
                    .get(i, &mut context)
                    .unwrap()
                    .to_string(&mut context)
                    .unwrap()
                    .to_std_string_escaped()
            })
            .collect())
    }

    #[tokio::test]
    async fn test_bundle_runtime() {
        let base_path = "src/specification/bundler/fixtures/runtime";
        let entry = "./a.ts";

        let bundled_code = bundle(base_path, entry).await.unwrap();
        let bundled_logs =
            eval_script_with_logging(&bundled_code).await.unwrap();

        let options = ResolveOptions::default();
        let resolver = Resolver::new(options);
        let key = resolver.resolve(base_path, entry).unwrap();
        let entry_path = key.path();

        let unbundled_logs =
            eval_module_with_logging(&entry_path).await.unwrap();

        assert_eq!(bundled_logs, unbundled_logs);
    }
}
