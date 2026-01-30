use std::{collections::HashMap, path::PathBuf, rc::Rc};

use boa_engine::{
    builtins::promise::PromiseState, context::ContextBuilder, Context, JsError,
    JsValue, Module, Source,
};
use result::Result;

use crate::specification::{
    module_loader::{load_bombadil_module, HybridModuleLoader},
    result::SpecificationError,
};

mod module_loader;
mod result;

#[allow(dead_code)]
struct Evaluator {
    loader: Rc<HybridModuleLoader>,
    context: Context,
    bombadil_module: Module,
    specification_module: Module,
    bombadil_exports: BombadilExports,
    properties: HashMap<String, JsValue>,
}

#[allow(dead_code)]
enum Specification<'a> {
    FromBytes(&'a [u8]),
    FromFile(PathBuf),
}

#[allow(dead_code)]
impl Evaluator {
    pub fn new(specification: Specification) -> Result<Self> {
        let loader = Rc::new(HybridModuleLoader::new()?);

        // Instantiate the execution context
        let mut context = ContextBuilder::default()
            .module_loader(loader.clone())
            .build()
            .unwrap();

        let bombadil_module = load_bombadil_module(&mut context)?;
        loader.insert_mapped_module("bombadil", bombadil_module.clone());

        let specification_module = match specification {
            Specification::FromBytes(bytes) => {
                let source = Source::from_bytes(bytes);
                Module::parse(source, None, &mut context)?
            }
            Specification::FromFile(path) => {
                let source = Source::from_filepath(&path)?;
                let module = Module::parse(source, None, &mut context)?;
                // TODO: is this needed?
                loader.insert_file_module(path, module.clone());
                module
            }
        };

        let promise = specification_module.load_link_evaluate(&mut context);
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

        let bombadil_exports =
            BombadilExports::from_module(&bombadil_module, &mut context)?;

        let specification_exports =
            module_exports(&specification_module, &mut context)?;
        let mut properties: HashMap<String, JsValue> = HashMap::new();
        for (key, value) in specification_exports.iter() {
            if value.instance_of(&bombadil_exports.always, &mut context)? {
                properties.insert(key.clone(), value.clone());
            }
        }

        Ok(Evaluator {
            loader,
            context,
            bombadil_module,
            specification_module,
            properties,
            bombadil_exports,
        })
    }

    pub fn property_names(&self) -> Vec<String> {
        self.properties.keys().cloned().collect()
    }
}

#[allow(dead_code)]
struct BombadilExports {
    formula: JsValue,
    always: JsValue,
    runtime_default: JsValue,
}

impl BombadilExports {
    fn from_module(module: &Module, context: &mut Context) -> Result<Self> {
        let exports = module_exports(module, context)?;

        let get_export = |name: &str| -> Result<JsValue> {
            exports
                .get(name)
                .cloned()
                .ok_or(SpecificationError::ModuleError(format!(
                    "{name} is missing in exports"
                )))
        };
        Ok(Self {
            formula: get_export("Formula")?,
            always: get_export("Always")?,
            runtime_default: get_export("runtime_default")?,
        })
    }
}

fn module_exports(
    module: &Module,
    context: &mut Context,
) -> Result<HashMap<String, JsValue>> {
    let mut exports = HashMap::new();
    for key in module.namespace(context).own_property_keys(context)? {
        let value = module.namespace(context).get(key.clone(), context)?;
        exports.insert(key.to_string(), value);
    }
    Ok(exports)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_property_names() {
        let evaluator = Evaluator::new(Specification::FromBytes(
            r#"
            import { always, condition, eventually, extract, time } from "bombadil";

            // Invariant

            const notification_count = extract(
              (state) => state.document.body.querySelectorAll(".notification").length,
            );

            export const max_notifications_shown = always(
              () => notification_count.current <= 5,
            );
            "#.as_bytes(),
        )).unwrap();
        assert_eq!(evaluator.property_names(), vec!["max_notifications_shown"]);
    }
}
