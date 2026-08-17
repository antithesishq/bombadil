use std::fmt::Write;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{self, Parser as _};
use oxc::{
    allocator::{self, Allocator, CloneIn},
    ast::{AstBuilder, ast},
    codegen::{self, Codegen},
    parser::Parser,
    semantic::SemanticBuilder,
    span::{SPAN, SourceType},
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

        if !module.by_name.is_empty() {
            writeln!(output, "#### Exports\n")?;
        }

        for (name, declarations) in module.by_name {
            writeln!(output, "##### `{name}`\n")?;
            for declaration in declarations {
                let code = match declaration {
                    ModuleDeclaration::Module(mut module_declaration) => {
                        module_declaration.declare = false;
                        render_statement(
                            &allocator,
                            &source_text,
                            &program.comments,
                            ast::Statement::TSModuleDeclaration(
                                module_declaration,
                            ),
                        )
                    }
                    ModuleDeclaration::Type(type_declaration) => {
                        match type_declaration {
                            TypeDeclaration::Class(mut class) => {
                                class.declare = false;
                                render_statement(
                                    &allocator,
                                    &source_text,
                                    &program.comments,
                                    ast::Statement::ClassDeclaration(class),
                                )
                            }
                            TypeDeclaration::Interface(mut interface) => {
                                interface.declare = false;
                                render_statement(
                                    &allocator,
                                    &source_text,
                                    &program.comments,
                                    ast::Statement::TSInterfaceDeclaration(
                                        interface,
                                    ),
                                )
                            }
                            TypeDeclaration::Enum(mut enum_declaration) => {
                                enum_declaration.declare = false;
                                render_statement(
                                    &allocator,
                                    &source_text,
                                    &program.comments,
                                    ast::Statement::TSEnumDeclaration(
                                        enum_declaration,
                                    ),
                                )
                            }
                            TypeDeclaration::Alias(alias) => render_statement(
                                &allocator,
                                &source_text,
                                &program.comments,
                                ast::Statement::TSTypeAliasDeclaration(alias),
                            ),
                        }
                    }
                    ModuleDeclaration::Value(value) => match value {
                        ValueDeclaration::Function(mut function) => {
                            function.declare = false;
                            render_statement(
                                &allocator,
                                &source_text,
                                &program.comments,
                                ast::Statement::FunctionDeclaration(function),
                            )
                        }
                        ValueDeclaration::Variable(mut variable) => {
                            variable.declare = false;
                            render_statement(
                                &allocator,
                                &source_text,
                                &program.comments,
                                ast::Statement::VariableDeclaration(variable),
                            )
                        }
                    },
                };
                writeln!(output, "```{{.typescript .no-copy}}\n{code}\n```\n")?;
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
    reexports: BTreeMap<String, Vec<String>>,
}

impl<'a> Module<'a> {
    fn get_name_mut<'b>(
        &'b mut self,
        name: &str,
    ) -> &'b mut Vec<ModuleDeclaration<'a>> {
        if !self.by_name.contains_key(name) {
            self.by_name.insert(name.into(), Vec::new());
        }
        self.by_name.get_mut(name).expect("value should exist")
    }
}

#[derive(Debug)]
enum ModuleDeclaration<'a> {
    Module(allocator::Box<'a, ast::TSModuleDeclaration<'a>>),
    Type(TypeDeclaration<'a>),
    Value(ValueDeclaration<'a>),
}

#[derive(Default)]
struct Traverser<'a> {
    in_exported: bool,
    in_nested_module: bool,
    declared_modules:
        BTreeMap<String, allocator::Box<'a, ast::TSModuleDeclaration<'a>>>,
    declared_types: BTreeMap<String, TypeDeclaration<'a>>,
    declared_values: BTreeMap<String, ValueDeclaration<'a>>,
    reexports: BTreeMap<String, Vec<String>>,
    referenced_types: HashSet<String>,
    referenced_values: HashSet<String>,
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
        } else {
            assert!(
                node.specifiers.is_empty(),
                "bare exports not supported yet"
            );
            self.in_exported = true;
        }
    }
    fn exit_export_named_declaration(
        &mut self,
        _node: &mut ast::ExportNamedDeclaration<'a>,
        _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        self.in_exported = false;
    }
    fn enter_export_default_declaration(
        &mut self,
        _node: &mut ast::ExportDefaultDeclaration<'a>,
        _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        self.in_exported = true;
    }
    fn exit_export_default_declaration(
        &mut self,
        _node: &mut ast::ExportDefaultDeclaration<'a>,
        _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        self.in_exported = false;
    }
    fn enter_ts_type_reference(
        &mut self,
        node: &mut ast::TSTypeReference<'a>,
        _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        if self.in_exported {
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

    fn enter_declaration(
        &mut self,
        node: &mut ast::Declaration<'a>,
        ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        use ast::Declaration::*;

        if self.in_nested_module {
            return;
        }

        let name: Option<String> = node.id().map(|id| id.name.to_string());

        match node {
            FunctionDeclaration(function) => {
                let name = name.expect("function has no name");
                if self.in_exported {
                    self.referenced_values.insert(name.clone());
                }
                self.declared_values.insert(
                    name,
                    ValueDeclaration::Function(
                        function.clone_in(ctx.ast.allocator),
                    ),
                );
            }
            VariableDeclaration(variable) => {
                for declaration in &variable.declarations {
                    let name = declaration
                        .id
                        .get_identifier_name()
                        .expect("variable declaration is missing name")
                        .to_string();
                    if self.in_exported {
                        self.referenced_values.insert(name.clone());
                    }
                    self.declared_values.insert(
                        name,
                        ValueDeclaration::Variable(
                            variable.clone_in(ctx.ast.allocator),
                        ),
                    );
                }
            }
            TSInterfaceDeclaration(interface) => {
                let name = name.expect("interface has no name");
                if self.in_exported {
                    self.referenced_types.insert(name.clone());
                }
                self.declared_types.insert(
                    name,
                    TypeDeclaration::Interface(
                        interface.clone_in(ctx.ast.allocator),
                    ),
                );
            }
            ClassDeclaration(class) => {
                let name = name.expect("class has no name");
                if self.in_exported {
                    self.referenced_types.insert(name.clone());
                }
                self.declared_types.insert(
                    name,
                    TypeDeclaration::Class(class.clone_in(ctx.ast.allocator)),
                );
            }
            TSModuleDeclaration(module) => {
                let name = name.expect("module has no name");
                self.declared_modules
                    .insert(name, module.clone_in(ctx.ast.allocator));
            }
            TSEnumDeclaration(enum_declaration) => {
                let name = name.expect("enum has no name");
                self.declared_types.insert(
                    name,
                    TypeDeclaration::Enum(
                        enum_declaration.clone_in(ctx.ast.allocator),
                    ),
                );
            }
            TSTypeAliasDeclaration(alias) => {
                let name = name.expect("alias has no name");
                self.referenced_types.insert(alias.id.name.to_string());
                self.declared_types.insert(
                    name,
                    TypeDeclaration::Alias(alias.clone_in(ctx.ast.allocator)),
                );
            }
            // TSGlobalDeclaration(tsglobal_declaration)
            // TSImportEqualsDeclaration(tsimport_equals_declaration)
            node => {
                eprintln!("ignored: {node:?}");
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
            // eprintln!("export specifier: {}", node.exported.name());
            self.referenced_values
                .insert(node.exported.name().to_string());
        }
    }

    // fn enter_export_all_declaration(
    //     &mut self,
    //     node: &mut ast::ExportAllDeclaration<'a>,
    //     _: &mut oxc_traverse::TraverseCtx<'a, ()>,
    // ) {
    //     eprintln!("export all: {:?}", node);
    // }
    //
    // fn enter_ts_export_assignment(
    //     &mut self,
    //     node: &mut ast::TSExportAssignment<'a>,
    //     _: &mut oxc_traverse::TraverseCtx<'a, ()>,
    // ) {
    //     eprintln!("export assignment: {:?}", node);
    // }
    //
    // fn enter_module_export_name(
    //     &mut self,
    //     node: &mut ast::ModuleExportName<'a>,
    //     _: &mut oxc_traverse::TraverseCtx<'a, ()>,
    // ) {
    //     eprintln!("export name: {:?}", node);
    // }
}

fn render_statement<'a>(
    allocator: &'a Allocator,
    source_text: &'a str,
    comments: &allocator::Vec<'a, ast::Comment>, // whatever type program.comments uses
    statement: ast::Statement<'a>,
) -> String {
    let builder = AstBuilder::new(allocator);
    let program_temporary = builder.program(
        SPAN,
        SourceType::d_ts(),
        source_text,
        comments.clone_in(allocator),
        None,          // hashbang
        builder.vec(), // directives
        builder.vec1(statement),
    );
    let codegen = Codegen::new().with_source_text(source_text).with_options(
        codegen::CodegenOptions {
            comments: codegen::CommentOptions {
                normal: true,
                jsdoc: true,
                annotation: false,
                legal: codegen::LegalComment::None,
            },
            single_quote: false,
            minify: false,
            source_map_path: None,
            indent_char: codegen::IndentChar::Space,
            indent_width: 2,
            initial_indent: 0,
        },
    );
    codegen.build(&program_temporary).code
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
    let mut traverser = Traverser::default();
    traverse_mut(&mut traverser, allocator, program, scoping, ());

    let mut module: Module<'a> = Default::default();

    for (name, declared_module) in traverser.declared_modules {
        module.get_name_mut(&name).push(ModuleDeclaration::Module(
            declared_module.clone_in(allocator),
        ));
    }

    for (name, declared_type) in traverser.declared_types {
        if traverser.referenced_types.contains(&name) {
            module.get_name_mut(&name).push(ModuleDeclaration::Type(
                declared_type.clone_in(allocator),
            ))
        } else {
            eprintln!("non-exported type ignored: {name}");
        }
    }

    for (name, declared_value) in traverser.declared_values {
        if traverser.referenced_values.contains(&name) {
            module.get_name_mut(&name).push(ModuleDeclaration::Value(
                declared_value.clone_in(allocator),
            ))
        } else {
            eprintln!("non-exported value ignored: {name}");
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
