/// This program renders our TypeScript package's modules
/// as Markdown, which is used in the manual. It supports
/// a subset of TypeScript that we use. We might need to
/// support more TypeScript constructs in this program
/// if required by the package.
///
/// We also impose certain rules, and make assumptions,
/// like how declarations should be commented.
use std::collections::HashMap;
use std::fmt::Write;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{self, Parser as _};
use oxc::codegen::Gen;
use oxc::span::GetSpan;
use oxc::{
    allocator::{self, Allocator, CloneIn},
    ast::ast,
    codegen::{self, Codegen},
    parser::Parser,
    semantic::SemanticBuilder,
    span::SourceType,
};
use oxc_traverse::{Traverse, traverse_mut};
use serde::Deserialize;
use serde_json as json;

#[derive(clap::Parser)]
#[command(name = "typescript-reference")]
struct Cli {
    #[arg(long = "package-root")]
    package_root: PathBuf,
}

fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();
    let exported_modules = ExportedModules::build(&cli.package_root)?;
    print!("{}", generate_reference(exported_modules)?);
    Ok(())
}

#[derive(Deserialize)]
struct JsPackage {
    name: String,
    exports: BTreeMap<PathBuf, JsExport>,
}

#[derive(Deserialize)]
struct JsExport {
    types: PathBuf,
}

struct ExportedModules {
    by_specifier: BTreeMap<String, ExportedModule>,
}

struct ExportedModule {
    path: PathBuf,
}

impl ExportedModules {
    fn build(package_root: &Path) -> Result<Self> {
        let package_json_path = package_root.join("package.json");
        let package: JsPackage = json::from_reader(
            std::fs::File::open(&package_json_path)
                .context(format!("read {package_json_path:?} failed"))?,
        )?;
        let mut by_specifier = BTreeMap::new();
        for (subpath, target) in package.exports {
            let resolved = package_root
                .join(&target.types)
                .canonicalize()
                .context(format!("resolving {:?}", &target.types))?;

            let specifier = if subpath == Path::new(".") {
                package.name.clone()
            } else {
                normalize_lexically(&Path::new(&package.name).join(&subpath))
                    .to_str()
                    .ok_or(anyhow!("invalid utf-8 in package name or subpath"))?
                    .to_string()
            };

            by_specifier.insert(specifier, ExportedModule { path: resolved });
        }

        Ok(Self { by_specifier })
    }
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn generate_reference(exported_modules: ExportedModules) -> Result<String> {
    let allocator = Allocator::default();
    let mut output = String::new();

    for (specifier, module) in exported_modules.by_specifier {
        let source_text = fs::read_to_string(&module.path)?;
        let mut program = parse(&allocator, &source_text, SourceType::d_ts())?;
        let module = module_extract(&allocator, &mut program)?;

        writeln!(output, "### {specifier}\n")?;

        if !module.defaults.is_empty() {
            writeln!(output, "### Default export\n")?;
        }

        for declaration in module.defaults {
            declaration.render(&allocator, &source_text, &mut output)?;
        }

        if !module.by_name.is_empty() {
            writeln!(output, "#### Exports\n")?;
        }

        for (name, declarations) in module.by_name {
            writeln!(output, "##### `{name}`\n")?;
            for declaration in declarations {
                declaration.render(&allocator, &source_text, &mut output)?;
            }
        }

        if !module.reexports.is_empty() {
            writeln!(output, "#### Reexports\n")?;
            for (specifier, identifiers) in module.reexports {
                writeln!(
                    output,
                    "```{{.typescript .no-copy}}\nexport {{ {} }} from {:?};\n```",
                    identifiers.to_vec().join(", "),
                    specifier
                )?;
            }
        }
    }
    Ok(output)
}

fn parse<'a>(
    allocator: &'a Allocator,
    source_text: &'a str,
    source_type: SourceType,
) -> Result<ast::Program<'a>> {
    let parser = Parser::new(allocator, source_text, source_type);
    let result = parser.parse();
    if result.panicked {
        bail!(
            "parse error(s):\n\n{}",
            result
                .errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
    Ok(result.program)
}

#[derive(Debug)]
enum ValueDeclaration<'a> {
    Function(allocator::Box<'a, ast::Function<'a>>),
    Variable(allocator::Box<'a, ast::VariableDeclaration<'a>>),
}

impl<'a> CloneIn<'a> for ValueDeclaration<'a> {
    type Cloned = Self;

    fn clone_in(&self, allocator: &'a Allocator) -> Self::Cloned {
        match self {
            Self::Function(function) => {
                Self::Function(function.clone_in(allocator))
            }
            Self::Variable(variable) => {
                Self::Variable(variable.clone_in(allocator))
            }
        }
    }
}

#[derive(Debug)]
enum TypeDeclaration<'a> {
    Class(allocator::Box<'a, ast::Class<'a>>),
    Interface(allocator::Box<'a, ast::TSInterfaceDeclaration<'a>>),
    Enum(allocator::Box<'a, ast::TSEnumDeclaration<'a>>),
    Alias(allocator::Box<'a, ast::TSTypeAliasDeclaration<'a>>),
}

impl<'a> CloneIn<'a> for TypeDeclaration<'a> {
    type Cloned = Self;

    fn clone_in(&self, allocator: &'a Allocator) -> Self::Cloned {
        match self {
            Self::Class(class) => Self::Class(class.clone_in(allocator)),
            Self::Interface(interface) => {
                Self::Interface(interface.clone_in(allocator))
            }
            Self::Enum(enum_declaration) => {
                Self::Enum(enum_declaration.clone_in(allocator))
            }
            Self::Alias(alias) => Self::Alias(alias.clone_in(allocator)),
        }
    }
}

#[derive(Debug, Default)]
struct Module<'a> {
    by_name: BTreeMap<String, Vec<ModuleDeclaration<'a>>>,
    defaults: Vec<ModuleDeclaration<'a>>,
    reexports: BTreeMap<String, Vec<String>>,
}

impl<'a> Module<'a> {
    fn get_by_name_mut<'b>(
        &'b mut self,
        name: &str,
    ) -> &'b mut Vec<ModuleDeclaration<'a>> {
        self.by_name.entry(name.into()).or_default()
    }
}

#[derive(Debug)]
struct ModuleDeclaration<'a> {
    export: Export<String>,
    kind: ModuleDeclarationKind<'a>,
}

impl<'a> ModuleDeclaration<'a> {
    fn render<Output: Write>(
        self,
        allocator: &'a Allocator,
        source_text: &str,
        output: &mut Output,
    ) -> anyhow::Result<()> {
        let code = match self.kind {
            ModuleDeclarationKind::Module(mut module_declaration) => {
                module_declaration.declare = false;
                render_node(allocator, source_text, module_declaration)
            }
            ModuleDeclarationKind::Type(type_declaration) => {
                match type_declaration {
                    TypeDeclaration::Class(mut class) => {
                        class.declare = false;
                        render_node(allocator, source_text, class)
                    }
                    TypeDeclaration::Interface(mut interface) => {
                        interface.declare = false;
                        render_node(allocator, source_text, interface)
                    }
                    TypeDeclaration::Enum(mut enum_declaration) => {
                        enum_declaration.declare = false;
                        render_node(allocator, source_text, enum_declaration)
                    }
                    TypeDeclaration::Alias(alias) => {
                        render_node(allocator, source_text, alias)
                    }
                }
            }
            ModuleDeclarationKind::Value(value) => match value {
                ValueDeclaration::Function(mut function) => {
                    function.declare = false;
                    render_node(allocator, source_text, function)
                }
                ValueDeclaration::Variable(mut variable) => {
                    variable.declare = false;
                    render_node(allocator, source_text, variable)
                }
            },
        };

        if let Some(comment) = self.export.comment() {
            let text = comment
                .content_span()
                .source_text(source_text)
                .lines()
                .map(|line| {
                    let trimmed = line.trim_start();
                    trimmed.strip_prefix('*').unwrap_or(trimmed).trim_start()
                })
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string();
            writeln!(output, "{text}\n")?;
        } else {
            bail!(
                "{} is not documented",
                self.export.name().unwrap_or("default export"),
            );
        }
        writeln!(output, "```{{.typescript .no-copy}}\n{code}\n```\n")?;
        Ok(())
    }
}

#[derive(Debug)]
enum ModuleDeclarationKind<'a> {
    Module(allocator::Box<'a, ast::TSModuleDeclaration<'a>>),
    Type(TypeDeclaration<'a>),
    Value(ValueDeclaration<'a>),
}

#[derive(Debug, Clone)]
enum Export<Name> {
    Named {
        name: Name,
        comment: Option<ast::Comment>,
    },
    Default {
        comment: Option<ast::Comment>,
    },
}

impl Export<Option<String>> {
    fn require_named(self) -> Export<String> {
        match self {
            Export::Named { name, comment } => Export::Named {
                name: name.expect("export has no name"),
                comment,
            },
            Export::Default { comment } => Export::Default { comment },
        }
    }
}

impl Export<String> {
    fn name(&self) -> Option<&str> {
        match self {
            Export::Named { name, .. } => Some(name),
            Export::Default { .. } => None,
        }
    }
}

impl<Name> Export<Name> {
    fn comment(&self) -> Option<ast::Comment> {
        match self {
            Export::Named { comment, .. } => *comment,
            Export::Default { comment } => *comment,
        }
    }
}

#[derive(Default)]
struct Traverser<'a> {
    exported_current: Option<Export<Option<String>>>,
    in_nested_module: bool,
    declarations: Vec<ModuleDeclaration<'a>>,
    reexports: BTreeMap<String, Vec<String>>,
    referenced_types: HashSet<String>,
    referenced_values: HashSet<String>,
    comments: HashMap<u32, ast::Comment>,
}

impl<'a> Traverse<'a, ()> for Traverser<'a> {
    fn enter_export_named_declaration(
        &mut self,
        node: &mut ast::ExportNamedDeclaration<'a>,
        _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        if let Some(source) = &node.source {
            let mut identifiers = vec![];
            for specifier in &node.specifiers {
                if specifier.local.name() != specifier.exported.name() {
                    eprintln!(
                        "ignoring renamed reexport: {:?} -> {:?}",
                        specifier.local.name(),
                        specifier.exported.name()
                    );
                }
                identifiers.push(specifier.local.name().to_string());
            }
            self.reexports.insert(source.value.to_string(), identifiers);
        } else if let Some(declaration) = &node.declaration {
            if self.in_nested_module {
                return;
            }

            let name = declaration.id().map(|id| id.name.to_string());
            let comment = self.comments.get(&node.span.start).cloned();
            self.exported_current = Some(Export::Named { name, comment });
        } else {
            assert!(
                node.specifiers.is_empty(),
                "bare exports not supported yet"
            );
        }
    }
    fn exit_export_named_declaration(
        &mut self,
        _node: &mut ast::ExportNamedDeclaration<'a>,
        _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        self.exported_current = None;
    }

    fn enter_export_default_declaration(
        &mut self,
        node: &mut ast::ExportDefaultDeclaration<'a>,
        ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        let comment = self.comments.get(&node.span.start).cloned();

        let export = Export::Default { comment };
        match &node.declaration {
            ast::ExportDefaultDeclarationKind::FunctionDeclaration(
                function,
            ) => {
                self.declarations.push(ModuleDeclaration {
                    export,
                    kind: ModuleDeclarationKind::Value(
                        ValueDeclaration::Function(
                            function.clone_in(ctx.ast.allocator),
                        ),
                    ),
                });
            }
            ast::ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                self.declarations.push(ModuleDeclaration {
                    export,
                    kind: ModuleDeclarationKind::Type(TypeDeclaration::Class(
                        class.clone_in(ctx.ast.allocator),
                    )),
                });
            }
            ast::ExportDefaultDeclarationKind::TSInterfaceDeclaration(
                interface,
            ) => {
                self.declarations.push(ModuleDeclaration {
                    export,
                    kind: ModuleDeclarationKind::Type(
                        TypeDeclaration::Interface(
                            interface.clone_in(ctx.ast.allocator),
                        ),
                    ),
                });
            }
            declaration => {
                panic!(
                    "no support for default-exported expressions: {:?}",
                    declaration.clone_in(ctx.ast.allocator).into_expression()
                );
            }
        }
        self.exported_current = Some(Export::Default { comment });
    }
    fn exit_export_default_declaration(
        &mut self,
        _node: &mut ast::ExportDefaultDeclaration<'a>,
        _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        self.exported_current = None;
    }
    fn enter_ts_type_reference(
        &mut self,
        node: &mut ast::TSTypeReference<'a>,
        _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        if self.exported_current.is_some() {
            let mut current = Some(&node.type_name);
            while let Some(node) = current.take() {
                match node {
                    ast::TSTypeName::IdentifierReference(
                        identifier_reference,
                    ) => {
                        self.referenced_types
                            .insert(identifier_reference.name.to_string());
                    }
                    ast::TSTypeName::QualifiedName(name) => {
                        current = Some(&name.left);
                        self.referenced_types.insert(name.right.to_string());
                    }
                    ast::TSTypeName::ThisExpression(this) => {
                        eprintln!("ignoring This type reference: {this:?}");
                    }
                }
            }
        }
    }

    fn enter_ts_module_declaration(
        &mut self,
        _: &mut ast::TSModuleDeclaration<'a>,
        _: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        self.in_nested_module = true;
    }

    fn exit_ts_module_declaration(
        &mut self,
        _: &mut ast::TSModuleDeclaration<'a>,
        _: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        self.in_nested_module = false;
    }

    fn enter_export_specifier(
        &mut self,
        node: &mut ast::ExportSpecifier<'a>,
        _: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        if node.local.name() != node.exported.name() {
            eprintln!(
                "ignoring renamed export: {:?} -> {:?}",
                node.local.name(),
                node.exported.name()
            );
        } else {
            self.referenced_values
                .insert(node.exported.name().to_string());
        }
    }

    fn enter_declaration(
        &mut self,
        node: &mut ast::Declaration<'a>,
        ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        use ast::Declaration::*;

        let export = if let Some(export) = &self.exported_current {
            export.clone()
        } else {
            return;
        };

        match node {
            FunctionDeclaration(function) => {
                self.declarations.push(ModuleDeclaration {
                    export: export.require_named(),
                    kind: ModuleDeclarationKind::Value(
                        ValueDeclaration::Function(
                            function.clone_in(ctx.ast.allocator),
                        ),
                    ),
                });
            }
            VariableDeclaration(variable) => {
                for declaration in &variable.declarations {
                    let name = declaration
                        .id
                        .get_identifier_name()
                        .expect("variable declaration is missing name")
                        .to_string();
                    let comment = self
                        .comments
                        .get(&declaration.span.start)
                        .cloned()
                        .or(export.comment());
                    self.declarations.push(ModuleDeclaration {
                        export: Export::Named { name, comment },
                        kind: ModuleDeclarationKind::Value(
                            ValueDeclaration::Variable(
                                variable.clone_in(ctx.ast.allocator),
                            ),
                        ),
                    });
                }
            }
            TSInterfaceDeclaration(interface) => {
                self.declarations.push(ModuleDeclaration {
                    export: export.require_named(),
                    kind: ModuleDeclarationKind::Type(
                        TypeDeclaration::Interface(
                            interface.clone_in(ctx.ast.allocator),
                        ),
                    ),
                });
            }
            ClassDeclaration(class) => {
                self.declarations.push(ModuleDeclaration {
                    export: export.require_named(),
                    kind: ModuleDeclarationKind::Type(TypeDeclaration::Class(
                        class.clone_in(ctx.ast.allocator),
                    )),
                });
            }
            TSModuleDeclaration(module) => {
                self.declarations.push(ModuleDeclaration {
                    export: export.require_named(),
                    kind: ModuleDeclarationKind::Module(
                        module.clone_in(ctx.ast.allocator),
                    ),
                });
            }
            TSEnumDeclaration(enum_declaration) => {
                self.declarations.push(ModuleDeclaration {
                    export: export.require_named(),
                    kind: ModuleDeclarationKind::Type(TypeDeclaration::Enum(
                        enum_declaration.clone_in(ctx.ast.allocator),
                    )),
                });
            }
            TSTypeAliasDeclaration(alias) => {
                self.referenced_types.insert(alias.id.name.to_string());
                self.declarations.push(ModuleDeclaration {
                    export: export.require_named(),
                    kind: ModuleDeclarationKind::Type(TypeDeclaration::Alias(
                        alias.clone_in(ctx.ast.allocator),
                    )),
                });
            }
            node => {
                panic!("unsupported node: {node:?}");
            }
        }
    }
}

fn render_node<'a, A: GetSpan + Gen + std::fmt::Debug>(
    _allocator: &'a Allocator,
    source_text: &'a str,
    statement: allocator::Box<'a, A>,
) -> String {
    let mut codegen = Codegen::new()
        .with_source_text(source_text)
        .with_options(codegen::CodegenOptions {
            comments: codegen::CommentOptions::default(),
            single_quote: false,
            minify: false,
            source_map_path: None,
            indent_char: codegen::IndentChar::Space,
            indent_width: 2,
            initial_indent: 0,
        });
    let ctx = oxc_codegen::Context::empty().with_typescript();
    statement.r#gen(&mut codegen, ctx);
    codegen.into_source_text()
}

fn module_extract<'a>(
    allocator: &'a Allocator,
    program: &mut ast::Program<'a>,
) -> Result<Module<'a>> {
    let semantic = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(program);

    if !semantic.errors.is_empty() {
        bail!(
            "semantic error(s):\n\n{}",
            semantic
                .errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
    let scoping = semantic.semantic.into_scoping();

    let comments = program
        .comments
        .iter()
        .filter(|comment| comment.is_leading())
        .map(|comment| (comment.attached_to, *comment))
        .collect::<HashMap<u32, oxc::ast::Comment>>();

    let mut traverser = Traverser {
        comments,
        ..Default::default()
    };
    traverse_mut(&mut traverser, allocator, program, scoping, ());

    let mut module: Module<'a> = Default::default();
    traverser
        .declarations
        .sort_by_key(|module| module.export.name().map(|s| s.to_string()));
    for declaration in traverser.declarations {
        match &declaration.export {
            Export::Named { name, .. } => {
                module.get_by_name_mut(name).push(declaration);
            }
            Export::Default { .. } => {
                module.defaults.push(declaration);
            }
        }
    }
    module.reexports = traverser.reexports;

    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    #[test]
    fn test_() {
        let exported_modules =
            ExportedModules::build(Path::new("test/modules")).unwrap();
        let output = generate_reference(exported_modules).unwrap();
        assert_snapshot!(output);
    }
}
