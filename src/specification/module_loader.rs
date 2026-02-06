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
use oxc::{allocator::Allocator, span::SourceType};

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
        // TODO: Do we need .parent() ?
        Ok(referrer_path
            .parent()
            .expect("referrer path has no parent directory")
            .join(specifier.to_std_string_lossy()))
    }

    fn transpile(
        &self,
        source_code: &str,
        source_type: &SourceType,
        _path: &Path,
    ) -> JsResult<String> {
        let allocator = Allocator::default();
        let parser =
            oxc::parser::Parser::new(&allocator, source_code, *source_type);
        let result = parser.parse();
        if result.panicked {
            return Err(JsError::from_rust(
                SpecificationError::TranspilationError(result.errors.to_vec()),
            ));
        }

        Ok(source_code.to_string()) // TODO: use oxc
    }
}

impl ModuleLoader for HybridModuleLoader {
    async fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        specifier: JsString,
        context: &std::cell::RefCell<&mut Context>,
    ) -> JsResult<Module> {
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

                let path = self.resolve_path(&referrer, &specifier)?;
                let ts_source =
                    fs::read_to_string(&path).map_err(JsError::from_rust)?;

                // 3. Transpile to JS source (sync is ideal so we don't .await with Context borrowed).
                let js_source =
                    self.transpile(&ts_source, &source_type, &path)?;

                let context = &mut context.borrow_mut();
                let source =
                    Source::from_reader(js_source.as_bytes(), Some(&path));
                let module = Module::parse(source, None, context)?;
                load_modules(context, std::slice::from_ref(&module))
                    .map_err(JsError::from_rust)?;
                Ok(module)
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
