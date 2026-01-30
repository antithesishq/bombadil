use std::{collections::HashMap, path::PathBuf, rc::Rc};

use boa_engine::{
    builtins::promise::PromiseState, context::ContextBuilder, js_string,
    object::builtins::JsArray, Context, JsError, JsValue, Module, Source,
};
use result::Result;

use crate::specification::{
    bombadil_exports::{module_exports, BombadilExports},
    extractors::Extractors,
    ltl::Formula,
    module_loader::{load_bombadil_module, HybridModuleLoader},
    result::SpecificationError,
};

mod bombadil_exports;
mod extractors;
mod ltl;
mod module_loader;
mod result;

#[allow(dead_code)]
struct Evaluator {
    loader: Rc<HybridModuleLoader>,
    context: Context,
    bombadil_module: Module,
    specification_module: Module,
    bombadil_exports: BombadilExports,
    properties: HashMap<String, Property>,
    extractors: Extractors,
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

        let mut properties: HashMap<String, Property> = HashMap::new();
        for (key, value) in specification_exports.iter() {
            if value.instance_of(&bombadil_exports.always, &mut context)? {
                properties.insert(
                    key.clone(),
                    Property {
                        name: key.clone(),
                        formula: Formula::from_value(
                            value,
                            &bombadil_exports,
                            &mut context,
                        )?,
                    },
                );
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

    pub fn properties(&self) -> Vec<Property> {
        self.properties.values().cloned().collect()
    }

    pub fn extractors(&mut self) -> Result<HashMap<u64, String>> {
        self.extractors.extract_functions(&mut self.context)
    }
}

#[allow(dead_code)]
enum Specification<'a> {
    FromBytes(&'a [u8]),
    FromFile(PathBuf),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct Property {
    name: String,
    formula: Formula,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_property_names() {
        let evaluator = Evaluator::new(Specification::FromBytes(
            r#"
            import { always, extract } from "bombadil";

            // Invariant

            const notification_count = extract(
              (state) => state.document.body.querySelectorAll(".notification").length,
            );

            export const max_notifications_shown = always(
              () => notification_count.current <= 5,
            );
            "#.as_bytes(),
        )).unwrap();
        assert_eq!(
            evaluator
                .properties()
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>(),
            vec!["max_notifications_shown"]
        );
    }

    #[test]
    fn test_property_formula_conversion() {
        let evaluator = Evaluator::new(Specification::FromBytes(
            r#"
            import { always } from "bombadil";

            export const max_notifications_shown = always(() => true);
            "#
            .as_bytes(),
        ))
        .unwrap();

        let properties = evaluator.properties();
        let property = properties.first().unwrap();

        match &property.formula {
            Formula::Always(subformula) => match subformula.as_ref() {
                Formula::Contextful(_) => return,
                _ => {}
            },
            _ => {}
        };

        panic!("unexpected formula: {:?}", property.formula)
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
