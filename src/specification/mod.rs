use std::{collections::HashMap, path::PathBuf, rc::Rc};

use boa_engine::{
    builtins::promise::PromiseState, context::ContextBuilder, js_string,
    object::builtins::JsArray, Context, JsError, JsObject, JsValue, Module,
    Source,
};
use result::Result;

use crate::specification::{
    extractors::Extractors,
    module_loader::{load_bombadil_module, HybridModuleLoader},
    result::SpecificationError,
};

mod extractors;
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
    extractors: Extractors,
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

        let mut extractors = Extractors::new();
        let extractors_array = JsArray::from_object(
            bombadil_exports
                .runtime_default
                .get(js_string!("extractors"), &mut context)?
                .as_object()
                .ok_or(SpecificationError::ModuleError(
                    "extractors is not an object".to_string(),
                ))?,
        )?;
        let length = extractors_array.length(&mut context)?;
        for i in 0..length {
            extractors.register(
                extractors_array
                    .at(i as i64, &mut context)?
                    .as_object()
                    .ok_or(SpecificationError::ModuleError(
                        "extractors is not an object".to_string(),
                    ))?,
            );
        }

        Ok(Evaluator {
            loader,
            context,
            bombadil_module,
            specification_module,
            properties,
            bombadil_exports,
            extractors,
        })
    }

    pub fn property_names(&self) -> Vec<String> {
        self.properties.keys().cloned().collect()
    }

    pub fn extractors(&mut self) -> Result<HashMap<u64, String>> {
        self.extractors.extract_functions(&mut self.context)
    }
}

#[allow(dead_code)]
struct BombadilExports {
    formula: JsValue,
    always: JsValue,
    runtime_default: JsObject,
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
            runtime_default: get_export("runtime_default")?.as_object().ok_or(
                SpecificationError::ModuleError(
                    "runtime_default is not an object".to_string(),
                ),
            )?,
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
        let mut evaluator = Evaluator::new(Specification::FromBytes(
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

    #[test]
    fn test_extractors() {
        let mut evaluator = Evaluator::new(Specification::FromBytes(
            r#"
            import { extract } from "bombadil";

            const notification_count = extract(
              (state) => state.foo
            );

            function test() {
                let local = extract(s => s.bar);
                let other = extract(function foo(state) { return state.baz; });
            }

            test();
            "#
            .as_bytes(),
        ))
        .unwrap();

        let extractors: Vec<String> = evaluator
            .extractors()
            .unwrap()
            .iter()
            .map(|(_, value)| value.clone())
            .collect();

        assert_eq!(
            extractors,
            vec![
                "(state) => state.foo",
                "(s) => s.bar",
                "function foo(state) { return state.baz; }"
            ]
        );
    }
}
