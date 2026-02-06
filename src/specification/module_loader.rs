use std::{
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use crate::specification::result::{Result, SpecificationError};
use boa_engine::{
    module::{MapModuleLoader, ModuleLoader, Referrer, SimpleModuleLoader},
    Context, JsError, JsResult, JsString, Module, Source,
};
use include_dir::{include_dir, Dir};
use oxc::{
    allocator::Allocator,
    span::SourceType,
    transformer::{TransformOptions, Transformer},
};
use oxc::{codegen::Codegen, semantic::SemanticBuilder};

static JS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/target/specification");

pub struct HybridModuleLoader {
    map_loader: Rc<MapModuleLoader>,
    file_loader: Rc<SimpleModuleLoader>,
}

impl HybridModuleLoader {
    pub fn new() -> Result<Self> {
        Ok(HybridModuleLoader {
            map_loader: Rc::new(MapModuleLoader::new()),
            file_loader: Rc::new(SimpleModuleLoader::new(".")?),
        })
    }

    pub fn insert_mapped_module(&self, path: impl AsRef<str>, module: Module) {
        self.map_loader.insert(path, module);
    }

    fn specifier_source_type(&self, spec: &JsString) -> JsResult<SourceType> {
        let s = spec.to_std_string_escaped();
        SourceType::from_path(s).map_err(JsError::from_rust)
    }

    fn resolve_path(
        &self,
        referrer: &Referrer,
        specifier: &JsString,
    ) -> JsResult<PathBuf> {
        let referrer_path = referrer.path().ok_or(JsError::from_rust(
            SpecificationError::OtherError(format!(
                "import {:?} failed, referrer has no path: {:?}",
                specifier, referrer
            )),
        ))?;
        Ok(referrer_path
            .parent()
            .expect("referrer path has no parent directory")
            .join(specifier.to_std_string_lossy()))
    }
}

impl ModuleLoader for HybridModuleLoader {
    async fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        specifier: JsString,
        context: &std::cell::RefCell<&mut Context>,
    ) -> JsResult<Module> {
        log::debug!("loading module: {}", specifier.display_escaped());
        match self
            .map_loader
            .clone()
            .load_imported_module(referrer.clone(), specifier.clone(), context)
            .await
        {
            Ok(module) => Ok(module),
            Err(_) => {
                let source_type = self.specifier_source_type(&specifier)?;
                // If it looks like JS, use the regular file loader.
                if [SourceType::cjs(), SourceType::mjs()].contains(&source_type)
                {
                    return self
                        .file_loader
                        .clone()
                        .load_imported_module(referrer, specifier, context)
                        .await;
                }

                // Otherwise we transpile to JS and load that in-memory.
                let path = self.resolve_path(&referrer, &specifier)?;
                let ts_source =
                    fs::read_to_string(&path).map_err(JsError::from_rust)?;

                let js_source =
                    transpile(&ts_source, path.as_path(), &source_type)
                        .map_err(JsError::from_rust)?;

                let context = &mut context.borrow_mut();
                let source =
                    Source::from_reader(js_source.as_bytes(), Some(&path));
                Module::parse(source, None, context)
            }
        }
    }
}

pub fn load_bombadil_module(
    name: impl AsRef<Path>,
    context: &mut Context,
) -> Result<Module> {
    let index_js = JS_DIR.get_file(&name).unwrap_or_else(|| {
        panic!("{} not available in build", name.as_ref().to_string_lossy())
    });
    let source = Source::from_bytes(index_js.contents());
    Module::parse(source, None, context).map_err(Into::into)
}

pub fn load_modules(context: &mut Context, modules: &[Module]) -> Result<()> {
    let mut results = Vec::with_capacity(modules.len());
    for module in modules {
        results.push((module, module.load_link_evaluate(context)));
    }

    context.run_jobs()?;

    for (module, promise) in results {
        match promise.state() {
            boa_engine::builtins::promise::PromiseState::Pending => {
                return Err(SpecificationError::OtherError(format!(
                    "module did not load: {:?}",
                    module.path()
                )))
            }
            boa_engine::builtins::promise::PromiseState::Fulfilled(..) => {}
            boa_engine::builtins::promise::PromiseState::Rejected(error) => {
                return Err(SpecificationError::JS(format!(
                    "{}",
                    error.display()
                )));
            }
        }
    }

    Ok(())
}

pub fn transpile(
    source_code: &str,
    path: &Path,
    source_type: &SourceType,
) -> Result<String> {
    let allocator = Allocator::default();
    let parser =
        oxc::parser::Parser::new(&allocator, source_code, *source_type);
    let result = parser.parse();
    if result.panicked {
        return Err(SpecificationError::TranspilationError(
            result.errors.to_vec(),
        ));
    }
    let mut program = result.program;

    let semantic = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&program);
    if !semantic.errors.is_empty() {
        let errors = semantic.errors.to_vec();
        return Err(SpecificationError::TranspilationError(errors));
    }

    let scopes = semantic.semantic.into_scoping();
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

    let transformer = Transformer::new(&allocator, path, &transform_options);
    transformer.build_with_scoping(scopes, &mut program);

    let codegen = Codegen::new().build(&program);
    Ok(codegen.code)
}
