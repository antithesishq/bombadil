/// This program renders our TypeScript package's modules
/// as Markdown, which is used in the manual. It supports
/// a subset of TypeScript that we use. We might need to
/// support more TypeScript constructs in this program
/// if required by the package.
///
/// We also impose certain rules, and make assumptions,
/// like how declarations should be commented.
use std::fmt::Write;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{self, Parser as _};
use oxc::allocator::hash_set::HashSet;
use oxc::allocator::{HashMap, Vec};
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
use oxc_str::Ident;
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
        let module = Module::extract(&allocator, &mut program)?;

        {
            let undocumented = module.undocumented_declarations();
            if !undocumented.is_empty() {
                eprintln!(
                    "some exported declarations in {} are not documented:\n\n{}\n",
                    specifier,
                    std::vec::Vec::from_iter(undocumented.iter().map(
                        |export| format!(
                            "- {}",
                            export.name().unwrap_or("default export")
                        )
                    ),)
                    .join("\n")
                );
            }
        }

        {
            let references = module.internal_references();
            if !references.is_empty() {
                eprintln!(
                    "some internal names in {} are referred to by exported declarations:\n\n{}\n",
                    specifier,
                    std::vec::Vec::from_iter(
                        references
                            .iter()
                            .map(|identifier| format!("- {}", identifier)),
                    )
                    .join("\n")
                );
            }
        }

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
            writeln!(output, "##### {name}\n")?;
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
                    Vec::from_iter_in(
                        identifiers
                            .iter()
                            .map(|identifier| identifier.as_str()),
                        &allocator
                    )
                    .join(", "),
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
            std::vec::Vec::from_iter(
                result.errors.iter().map(ToString::to_string),
            )
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

struct Module<'a> {
    allocator: &'a Allocator,
    by_name: BTreeMap<String, Vec<'a, ModuleDeclaration<'a>>>,
    defaults: Vec<'a, ModuleDeclaration<'a>>,
    reexports: HashMap<'a, Ident<'a>, Vec<'a, Ident<'a>>>,
    imported_names: HashSet<'a, Ident<'a>>,
    referenced_names: HashSet<'a, Ident<'a>>,
}

impl<'a> Module<'a> {
    fn new(allocator: &'a Allocator) -> Self {
        Module {
            allocator,
            by_name: Default::default(),
            defaults: Vec::new_in(allocator),
            reexports: HashMap::new_in(allocator),
            imported_names: HashSet::new_in(allocator),
            referenced_names: HashSet::new_in(allocator),
        }
    }

    fn extract(
        allocator: &'a Allocator,
        program: &mut ast::Program<'a>,
    ) -> Result<Self> {
        let semantic = SemanticBuilder::new()
            .with_check_syntax_error(true)
            .build(program);

        if !semantic.errors.is_empty() {
            bail!(
                "semantic error(s):\n\n{}",
                std::vec::Vec::from_iter(
                    semantic.errors.iter().map(ToString::to_string),
                )
                .join("\n")
            )
        }
        let scoping = semantic.semantic.into_scoping();

        let comments = HashMap::from_iter_in(
            program
                .comments
                .iter()
                .filter(|comment| comment.is_leading())
                .map(|comment| (comment.attached_to, *comment)),
            allocator,
        );

        let mut traverser = Traverser {
            comments,
            ..Traverser::new(allocator)
        };
        traverse_mut(&mut traverser, allocator, program, scoping, ());

        let mut module = Module::new(allocator);
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
        module.referenced_names = traverser.referenced_names;
        module.reexports = traverser.reexports;
        module.imported_names = traverser.imported_names;

        Ok(module)
    }

    fn undocumented_declarations(&self) -> Vec<'a, Export<Ident<'a>>> {
        let mut undocumented = Vec::new_in(self.allocator);
        for export in self
            .defaults
            .iter()
            .map(|declaration| declaration.export.clone())
            .filter(|export| export.comment().is_none())
        {
            undocumented.push(export);
        }
        for export in self
            .by_name
            .values()
            .flatten()
            .map(|declaration| declaration.export.clone())
            .filter(|export| export.comment().is_none())
        {
            undocumented.push(export);
        }
        undocumented
    }

    fn internal_references<'b>(&'b self) -> Vec<'a, Ident<'a>> {
        let mut unexported = Vec::new_in(self.allocator);
        for identifier in &self.referenced_names {
            if !self.by_name.contains_key(identifier.as_str())
                && !self.imported_names.contains(identifier)
                && self
                    .reexports
                    .values()
                    .find(|identifiers| identifiers.contains(identifier))
                    .is_none()
            {
                unexported.push(*identifier);
            }
        }
        unexported
    }

    fn get_by_name_mut<'b>(
        &'b mut self,
        name: &str,
    ) -> &'b mut Vec<'a, ModuleDeclaration<'a>> {
        self.by_name
            .entry(name.into())
            .or_insert(Vec::new_in(self.allocator))
    }
}

#[derive(Debug)]
struct ModuleDeclaration<'a> {
    export: Export<Ident<'a>>,
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
            let text = Vec::from_iter_in(
                comment.content_span().source_text(source_text).lines().map(
                    |line| {
                        let trimmed = line.trim_start();
                        trimmed
                            .strip_prefix('*')
                            .unwrap_or(trimmed)
                            .trim_start()
                    },
                ),
                allocator,
            )
            .join("\n")
            .trim()
            .to_string();
            writeln!(output, "{text}\n")?;
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

impl<'a> Export<Option<Ident<'a>>> {
    fn require_named(self) -> Export<Ident<'a>> {
        match self {
            Export::Named { name, comment } => Export::Named {
                name: name.expect("export has no name"),
                comment,
            },
            Export::Default { comment } => Export::Default { comment },
        }
    }
}

impl<'a> Export<Ident<'a>> {
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

struct Traverser<'a> {
    exported_current: Option<Export<Option<Ident<'a>>>>,
    in_nested_module: bool,
    declarations: Vec<'a, ModuleDeclaration<'a>>,
    reexports: HashMap<'a, Ident<'a>, Vec<'a, Ident<'a>>>,
    imported_names: HashSet<'a, Ident<'a>>,
    referenced_names: HashSet<'a, Ident<'a>>,
    comments: HashMap<'a, u32, ast::Comment>,
}

impl<'a> Traverser<'a> {
    fn new(allocator: &'a Allocator) -> Self {
        Traverser {
            exported_current: Default::default(),
            in_nested_module: false,
            declarations: Vec::new_in(allocator),
            reexports: HashMap::new_in(allocator),
            imported_names: HashSet::new_in(allocator),
            referenced_names: HashSet::new_in(allocator),
            comments: HashMap::new_in(allocator),
        }
    }
}

impl<'a> Traverse<'a, ()> for Traverser<'a> {
    fn enter_import_declaration_specifier(
        &mut self,
        node: &mut ast::ImportDeclarationSpecifier<'a>,
        _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        self.imported_names.insert(node.local().name);
    }

    fn enter_import_default_specifier(
        &mut self,
        node: &mut ast::ImportDefaultSpecifier<'a>,
        _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        self.imported_names.insert(node.local.name);
    }

    fn enter_export_named_declaration(
        &mut self,
        node: &mut ast::ExportNamedDeclaration<'a>,
        ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        if let Some(source) = &node.source {
            let mut identifiers: Vec<'a, Ident<'a>> =
                Vec::new_in(ctx.ast.allocator);
            for specifier in &node.specifiers {
                if specifier.local.name() != specifier.exported.name() {
                    eprintln!(
                        "ignoring renamed reexport: {:?} -> {:?}",
                        specifier.local.name(),
                        specifier.exported.name()
                    );
                }
                identifiers.push(specifier.local.name().into());
            }
            self.reexports.insert(source.value.into(), identifiers);
        } else if let Some(declaration) = &node.declaration {
            if self.in_nested_module {
                return;
            }

            let name = declaration.id().map(|id| id.name);
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
        ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        if self.exported_current.is_some() {
            let mut current = Some(&node.type_name);
            while let Some(node) = current.take() {
                match node {
                    ast::TSTypeName::IdentifierReference(
                        identifier_reference,
                    ) => {
                        if ctx
                            .scoping()
                            .get_root_binding(identifier_reference.name)
                            .is_some()
                        {
                            // Only track references to root-scope bindings (i.e. free variables).
                            self.referenced_names
                                .insert(identifier_reference.name);
                        }
                    }
                    ast::TSTypeName::QualifiedName(name) => {
                        current = Some(&name.left);
                        // Only track references to root-scope bindings (i.e. free variables).
                        if ctx
                            .scoping()
                            .get_root_binding(name.right.name)
                            .is_some()
                        {
                            self.referenced_names.insert(name.right.name);
                        }
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
            self.referenced_names.insert(node.exported.name().into());
        }
    }

    fn enter_declaration(
        &mut self,
        node: &mut ast::Declaration<'a>,
        ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        use ast::Declaration::*;

        if self.in_nested_module {
            return;
        }

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
                        .expect("variable declaration is missing name");
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
                self.referenced_names.insert(alias.id.name);
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
