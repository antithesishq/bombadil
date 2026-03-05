use std::{cell::RefCell, collections::HashMap, path::Path, rc::Rc, sync::Arc};

use crate::specification::{
    resolver::{ModuleKey, Resolver},
    result::{Result, SpecificationError},
};
use boa_engine::{
    Context, JsError, JsResult, JsString, Module, Source,
    module::{ModuleLoader, Referrer},
};
use oxc::{
    allocator::Allocator,
    span::SourceType,
    transformer::{TransformOptions, Transformer},
};
use oxc::{codegen::Codegen, semantic::SemanticBuilder};

#[derive(Clone)]
pub struct HybridModuleLoader {
    resolver: Arc<Resolver>,
    cache: Rc<RefCell<HashMap<String, Module>>>,
}

impl HybridModuleLoader {
    pub fn new() -> Result<Self> {
        Ok(HybridModuleLoader {
            resolver: Arc::new(Resolver::new()),
            cache: Rc::new(RefCell::new(HashMap::new())),
        })
    }

    fn resolve_path(
        &self,
        referrer: &Path,
        specifier: &JsString,
    ) -> JsResult<ModuleKey> {
        let referrer = if referrer.is_absolute() {
            referrer.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(JsError::from_rust)?
                .join(referrer)
        };

        let referrer = if referrer.is_file() {
            referrer
                .parent()
                .expect("absolute path should have parent")
        } else {
            &referrer
        };

        self.resolver
            .resolve(referrer, &specifier.to_std_string_lossy())
            .map_err(JsError::from_rust)
    }

    pub fn load_module(
        &self,
        referrer: &Path,
        specifier: JsString,
        context: &std::cell::RefCell<&mut Context>,
    ) -> JsResult<Module> {
        log::debug!("loading module: {}", specifier.display_escaped());
        let key = self.resolve_path(referrer, &specifier)?;

        if let Some(module) = self.cache.borrow().get(key.specifier()) {
            return Ok(module.clone());
        }

        let source_type = match &key {
            ModuleKey::Embedded { .. } => SourceType::mjs(),
            ModuleKey::OnDisk { specifier, .. } => {
                SourceType::from_path(specifier).map_err(JsError::from_rust)?
            }
        };
        let mut source_text = key.source_text().map_err(JsError::from_rust)?;

        if ![SourceType::cjs(), SourceType::mjs()].contains(&source_type) {
            source_text = transpile(&source_text, key.path(), &source_type)
                .map_err(JsError::from_rust)?;
        }

        let context = &mut context.borrow_mut();
        let source = Source::from_reader(
            source_text.as_bytes(),
            Some(Path::new(key.specifier())),
        );
        let module = Module::parse(source, None, context)?;

        self.cache
            .borrow_mut()
            .insert(key.specifier().to_string(), module.clone());

        Ok(module)
    }
}

impl ModuleLoader for HybridModuleLoader {
    async fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        specifier: JsString,
        context: &std::cell::RefCell<&mut Context>,
    ) -> JsResult<Module> {
        let referrer_path = referrer.path().ok_or(JsError::from_rust(
            SpecificationError::OtherError(format!(
                "import {:?} failed, referrer has no path: {:?}",
                specifier, referrer
            )),
        ))?;
        self.load_module(referrer_path, specifier, context)
    }
}

pub fn load_modules(context: &mut Context, modules: &[&Module]) -> Result<()> {
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
                )));
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
