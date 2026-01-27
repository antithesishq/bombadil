use std::{path::PathBuf, rc::Rc};

use boa_engine::{
    builtins::promise::PromiseState,
    context::ContextBuilder,
    js_string,
    module::{MapModuleLoader, ModuleLoader, Referrer, SimpleModuleLoader},
    Context, JsError, JsResult, JsString, JsValue, Module, Source,
};

struct HybridModuleLoader {
    map_loader: Rc<MapModuleLoader>,
    file_loader: Rc<SimpleModuleLoader>,
}

impl HybridModuleLoader {
    fn new() -> JsResult<Self> {
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

fn bombadil_module(context: &mut Context) -> Module {
    let source = Source::from_bytes(
        r#"
        export const value = 3;
        "#,
    );
    return Module::parse(source, None, context).unwrap();
}

fn spec_module(context: &mut Context) -> Module {
    let source = Source::from_bytes(
        r#"
        import { value } from "bombadil";
        export default new Array(value).fill(1);
        "#,
    );
    return Module::parse(source, None, context).unwrap();
}

#[allow(dead_code)]
fn test() {
    let loader = Rc::new(HybridModuleLoader::new().unwrap());

    // Instantiate the execution context
    let mut context = ContextBuilder::default()
        .module_loader(loader.clone())
        .build()
        .unwrap();

    loader.insert_mapped_module("bombadil", bombadil_module(&mut context));

    let spec_module = spec_module(&mut context);
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

    let default = spec_module
        .namespace(&mut context)
        .get(js_string!("default"), &mut context)
        .unwrap();

    assert_eq!(default.display().to_string(), "[ 1, 1, 1 ]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_js() {
        test();
    }
}
