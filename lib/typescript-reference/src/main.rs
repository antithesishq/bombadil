use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{self, Parser as _};
use oxc::{
    allocator::{self, Allocator, CloneIn, TakeIn},
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
    generate_reference(exported_modules)?;
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

fn generate_reference(exported_modules: ExportedModules) -> Result<()> {
    let allocator = Allocator::default();

    for (specifier, module) in exported_modules.by_specifier {
        let source_text = fs::read_to_string(&module.path)?;
        let mut program = parse(&allocator, &source_text, SourceType::d_ts())?;
        let declarations = module_declarations(&allocator, &mut program)?;

        println!("### {specifier}\n");

        for declaration in declarations {
            let (name, code) = match declaration {
                ModuleDeclaration::Type(name, type_declaration) => {
                    let code = match type_declaration {
                        TypeDeclaration::Class(class) => render_statement(
                            &allocator,
                            &source_text,
                            &program.comments,
                            ast::Statement::ClassDeclaration(
                                allocator::Box::new_in(class, &allocator),
                            ),
                        ),
                        TypeDeclaration::Interface(interface) => {
                            render_statement(
                                &allocator,
                                &source_text,
                                &program.comments,
                                ast::Statement::TSInterfaceDeclaration(
                                    allocator::Box::new_in(
                                        interface, &allocator,
                                    ),
                                ),
                            )
                        }
                    };
                    (name, code)
                }
                ModuleDeclaration::Value(name, value) => {
                    let code = match value {
                        ValueDeclaration::Function(function) => {
                            render_statement(
                                &allocator,
                                &source_text,
                                &program.comments,
                                ast::Statement::FunctionDeclaration(
                                    allocator::Box::new_in(
                                        function, &allocator,
                                    ),
                                ),
                            )
                        }
                        ValueDeclaration::Variable(variable) => {
                            render_statement(
                                &allocator,
                                &source_text,
                                &program.comments,
                                ast::Statement::VariableDeclaration(
                                    allocator::Box::new_in(
                                        variable, &allocator,
                                    ),
                                ),
                            )
                        }
                    };
                    (name, code)
                }
            };
            println!(
                "#### {name}\n\n```{{.typescript .no-copy}}\n{code}\n```\n"
            );
        }
    }
    Ok(())
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
    Function(ast::Function<'a>),
    Variable(ast::VariableDeclaration<'a>),
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
    Class(ast::Class<'a>),
    Interface(ast::TSInterfaceDeclaration<'a>),
}

impl<'a> CloneIn<'a> for TypeDeclaration<'a> {
    type Cloned = Self;

    fn clone_in(&self, allocator: &'a Allocator) -> Self::Cloned {
        match self {
            Self::Class(class) => Self::Class(class.clone_in(allocator)),
            Self::Interface(interface) => {
                Self::Interface(interface.clone_in(allocator))
            }
        }
    }
}

#[derive(Debug)]
enum ModuleDeclaration<'a> {
    Type(String, TypeDeclaration<'a>),
    Value(String, ValueDeclaration<'a>),
}

#[derive(Default)]
struct Traverser<'a> {
    in_exported: bool,
    declared_types: BTreeMap<String, TypeDeclaration<'a>>,
    declared_values: BTreeMap<String, ValueDeclaration<'a>>,
    referenced_types: HashSet<String>,
    referenced_values: HashSet<String>,
}

impl<'a> Traverse<'a, ()> for Traverser<'a> {
    fn enter_export_named_declaration(
        &mut self,
        _node: &mut ast::ExportNamedDeclaration<'a>,
        _ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        self.in_exported = true;
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
            match &node.type_name {
                ast::TSTypeName::IdentifierReference(identifier_reference) => {
                    self.referenced_types
                        .insert(identifier_reference.name.to_string());
                }
                ast::TSTypeName::QualifiedName(name) => {
                    eprintln!("ignoring qualified type reference: {name:?}");
                }
                ast::TSTypeName::ThisExpression(this) => {
                    eprintln!("ignoring This type reference: {this:?}");
                }
            }
        }
    }

    fn exit_declaration(
        &mut self,
        node: &mut ast::Declaration<'a>,
        ctx: &mut oxc_traverse::TraverseCtx<'a, ()>,
    ) {
        use ast::Declaration::*;
        let name = if let Some(id) = node.id() {
            id.name.to_string()
        } else {
            return;
        };

        match node {
            FunctionDeclaration(function) => {
                if self.in_exported {
                    self.referenced_values.insert(name.clone());
                }
                self.declared_values.insert(
                    name,
                    ValueDeclaration::Function(
                        function.take_in(ctx.ast.allocator),
                    ),
                );
            }
            VariableDeclaration(variable) => {
                if self.in_exported {
                    self.referenced_values.insert(name.clone());
                }
                self.declared_values.insert(
                    name,
                    ValueDeclaration::Variable(
                        variable.take_in(ctx.ast.allocator),
                    ),
                );
            }
            TSInterfaceDeclaration(interface) => {
                if self.in_exported {
                    self.referenced_types.insert(name.clone());
                }
                self.declared_types.insert(
                    name,
                    TypeDeclaration::Interface(
                        interface.take_in(ctx.ast.allocator),
                    ),
                );
            }
            ClassDeclaration(class) => {
                if self.in_exported {
                    self.referenced_types.insert(name.clone());
                }
                self.declared_types.insert(
                    name,
                    TypeDeclaration::Class(class.take_in(ctx.ast.allocator)),
                );
            }
            // TSTypeAliasDeclaration(tstype_alias_declaration)
            // TSEnumDeclaration(tsenum_declaration)
            // TSModuleDeclaration(tsmodule_declaration)
            // TSGlobalDeclaration(tsglobal_declaration)
            // TSImportEqualsDeclaration(tsimport_equals_declaration)
            node => {
                eprintln!("ignored: {node:?}");
            }
        }
    }
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

fn module_declarations<'a>(
    allocator: &'a Allocator,
    program: &mut ast::Program<'a>,
) -> Result<Vec<ModuleDeclaration<'a>>> {
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

    let mut results = vec![];

    for (name, declared_type) in traverser.declared_types {
        if traverser.referenced_types.contains(&name) {
            results.push(ModuleDeclaration::Type(
                name,
                declared_type.clone_in(allocator),
            ))
        }
    }

    for (name, declared_value) in traverser.declared_values {
        if traverser.referenced_values.contains(&name) {
            results.push(ModuleDeclaration::Value(
                name,
                declared_value.clone_in(allocator),
            ))
        }
    }

    Ok(results)
}
