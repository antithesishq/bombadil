use std::{io, path::PathBuf, rc::Rc};

use boa_engine::{
    builtins::promise::PromiseState,
    context::ContextBuilder,
    js_string,
    module::{MapModuleLoader, ModuleLoader, Referrer, SimpleModuleLoader},
    Context, JsError, JsResult, JsString, JsValue, Module, Source,
};
use include_dir::{include_dir, Dir};

#[derive(Debug)]
pub enum SpecificationError {
    IoError(io::Error),
    JsError(JsError),
}

impl From<JsError> for SpecificationError {
    fn from(value: JsError) -> Self {
        SpecificationError::JsError(value)
    }
}

type Result<T> = std::result::Result<T, SpecificationError>;

static JS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/target/specification");

struct HybridModuleLoader {
    map_loader: Rc<MapModuleLoader>,
    file_loader: Rc<SimpleModuleLoader>,
}

impl HybridModuleLoader {
    fn new() -> Result<Self> {
        Ok(HybridModuleLoader {
            map_loader: Rc::new(MapModuleLoader::new()),
            file_loader: Rc::new(SimpleModuleLoader::new(".")?),
        })
    }

    fn insert_mapped_module(&self, path: impl AsRef<str>, module: Module) {
        self.map_loader.insert(path, module);
    }

    fn insert_file_module(&self, path: PathBuf, module: Module) {
        self.file_loader.insert(path, module);
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
                self.file_loader
                    .clone()
                    .load_imported_module(referrer, specifier, context)
                    .await
            }
        }
    }
}

fn load_bombadil_module(context: &mut Context) -> Result<Module> {
    let index_js = JS_DIR
        .get_file("index.js")
        .expect("index.js not available in build");
    let source = Source::from_bytes(index_js.contents());
    return Module::parse(source, None, context)
        .map_err(SpecificationError::JsError);
}

fn spec_module(context: &mut Context) -> Result<Module> {
    let source = Source::from_bytes(
        r#"
        import { always, condition, eventually, extract, time } from "bombadil";

        // Invariant

        const notification_count = extract(
          (state) => state.document.body.querySelectorAll(".notification").length,
        );

        export const max_notifications_shown = always(
          () => notification_count.current <= 5,
        );
        "#,
    );
    return Module::parse(source, None, context)
        .map_err(SpecificationError::JsError);
}

#[allow(dead_code)]
fn test() -> Result<()> {
    let loader = Rc::new(HybridModuleLoader::new()?);

    // Instantiate the execution context
    let mut context = ContextBuilder::default()
        .module_loader(loader.clone())
        .build()
        .unwrap();

    let bombadil_module = load_bombadil_module(&mut context)?;
    loader.insert_mapped_module("bombadil", bombadil_module.clone());

    let spec_module = spec_module(&mut context)?;
    loader.insert_file_module(PathBuf::from("test.js"), spec_module.clone());

    let promise = spec_module.load_link_evaluate(&mut context);
    context.run_jobs().unwrap();
    let result = promise.state();

    match result {
        PromiseState::Pending => panic!("module didn't execute"),
        PromiseState::Fulfilled(result) => {
            assert_eq!(result, JsValue::undefined());
        }
        PromiseState::Rejected(err) => {
            panic!(
                "failed: {:?}",
                JsError::from_opaque(err).into_erased(&mut context)
            )
        }
    };

    for key in bombadil_module
        .namespace(&mut context)
        .own_property_keys(&mut context)
        .map_err(SpecificationError::JsError)?
    {
        println!("bombadil export: {key:?}");
    }

    let formula_type = bombadil_module
        .namespace(&mut context)
        .get(js_string!("Formula"), &mut context)
        .map_err(SpecificationError::JsError)?;

    let mut properties = vec![];
    for key in spec_module
        .namespace(&mut context)
        .own_property_keys(&mut context)
        .map_err(SpecificationError::JsError)?
    {
        let value = spec_module
            .namespace(&mut context)
            .get(key.clone(), &mut context)
            .map_err(SpecificationError::JsError)?;

        if value.instance_of(&formula_type, &mut context)? {
            properties.push((key, value));
        } else {
            log::debug!("ignoring exported member {key:?}");
        }
    }

    assert_eq!(
        properties
            .iter()
            .map(|(key, _)| key.to_string())
            .collect::<Vec<_>>(),
        vec!["max_notifications_shown"]
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_js() {
        test().unwrap();
    }
}
