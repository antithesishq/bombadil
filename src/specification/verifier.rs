use std::path::PathBuf;
use std::{collections::HashMap, rc::Rc};

use crate::specification::result::Result;
use crate::specification::{ltl, module_loader::load_modules};
use boa_engine::{
    context::ContextBuilder, js_string, object::builtins::JsArray,
    property::PropertyKey, Context, JsString, Module, Source,
};
use serde_json as json;

use crate::specification::{
    bombadil_exports::{module_exports, BombadilExports},
    extractors::Extractors,
    ltl::{Evaluator, Formula, Residual, Violation},
    module_loader::{load_bombadil_module, HybridModuleLoader},
    result::SpecificationError,
};

#[derive(Clone, Debug)]
pub struct Specification {
    pub contents: Vec<u8>,
    pub path: PathBuf,
}

pub struct Verifier {
    context: Context,
    bombadil_exports: BombadilExports,
    properties: HashMap<String, Property>,
    extractors: Extractors,
    extractor_functions: HashMap<u64, String>,
}

impl Verifier {
    pub fn new(specification: Specification) -> Result<Self> {
        let loader = Rc::new(HybridModuleLoader::new()?);

        // Instantiate the execution context
        let mut context = ContextBuilder::default()
            .module_loader(loader.clone())
            .build()
            .unwrap();

        let mut modules = vec![];
        let mut bombadil_module_index = None;
        for (specifier, path) in [
            ("bombadil/internal", "internal.js"),
            ("bombadil", "index.js"),
        ] {
            let module = load_bombadil_module(path, &mut context)?;
            loader.insert_mapped_module(specifier, module.clone());
            if specifier == "bombadil" {
                bombadil_module_index = Some(module.clone());
            }
            modules.push(module);
        }

        let bombadil_module_defaults =
            load_bombadil_module("defaults.js", &mut context)?;
        loader.insert_mapped_module(
            "bombadil/defaults",
            bombadil_module_defaults.clone(),
        );

        let specification_module = {
            let specification_bytes: &[u8] = &specification.contents;
            Module::parse(
                Source::from_reader(
                    specification_bytes,
                    Some(&specification.path),
                ),
                None,
                &mut context,
            )?
        };
        modules.push(specification_module.clone());
        load_modules(&mut context, &modules)?;

        let specification_exports =
            module_exports(&specification_module, &mut context)?;
        let bombadil_exports = BombadilExports::from_module(
            &bombadil_module_index.ok_or(SpecificationError::OtherError(
                "bombadil index module not loaded".to_string(),
            ))?,
            &mut context,
        )?;

        let mut properties: HashMap<String, Property> = HashMap::new();
        for (key, value) in specification_exports.iter() {
            if value.instance_of(&bombadil_exports.formula, &mut context)? {
                properties.insert(
                    key.to_string(),
                    Property {
                        name: key.to_string(),
                        state: PropertyState::Initial(Formula::from_value(
                            value,
                            &bombadil_exports,
                            &mut context,
                        )?),
                    },
                );
            } else if let PropertyKey::Symbol(symbol) = key
                && let Some(description) = symbol.description()
                && IGNORED_SYMBOL_EXPORTS.contains(&description)
            {
                continue;
            } else {
                return Err(SpecificationError::OtherError(format!(
                    "export {:?} is of unknown type ({}): {}",
                    key.to_string(),
                    value.type_of(),
                    value.display()
                )));
            }
        }

        let mut extractors = Extractors::new(&bombadil_exports);

        let extractors_value = bombadil_exports
            .runtime_default
            .get(js_string!("extractors"), &mut context)?;
        let extractors_array =
            JsArray::from_object(extractors_value.as_object().ok_or(
                SpecificationError::OtherError(format!(
                    "extractors is not an object, it is {}",
                    extractors_value.type_of()
                )),
            )?)?;
        let length = extractors_array.length(&mut context)?;
        for i in 0..length {
            extractors.register(
                extractors_array
                    .at(i as i64, &mut context)?
                    .as_object()
                    .ok_or(SpecificationError::OtherError(
                        "extractor is not an object".to_string(),
                    ))?,
            );
        }

        let extractor_functions = extractors.extract_functions(&mut context)?;

        Ok(Verifier {
            context,
            properties,
            bombadil_exports,
            extractors,
            extractor_functions,
        })
    }

    pub fn properties(&self) -> Vec<String> {
        self.properties.keys().map(|key| key.clone()).collect()
    }

    pub fn extractors(&self) -> Result<Vec<(u64, String)>> {
        let mut results = Vec::with_capacity(self.extractor_functions.len());
        for (key, value) in &self.extractor_functions {
            results.push((*key, value.clone()));
        }
        Ok(results)
    }

    pub fn step(
        &mut self,
        snapshots: Vec<(u64, json::Value)>,
        time: ltl::Time,
    ) -> Result<Vec<(String, ltl::Value)>> {
        self.extractors.update_from_snapshots(
            snapshots,
            time,
            &mut self.context,
        )?;
        let mut results = Vec::with_capacity(self.properties.len());
        let mut evaluator =
            Evaluator::new(&self.bombadil_exports, &mut self.context);
        for property in self.properties.values_mut() {
            let value = match &property.state {
                PropertyState::Initial(formula) => {
                    evaluator.evaluate(formula, time)?
                }
                PropertyState::Residual(residual) => {
                    evaluator.step(residual, time)?
                }
                PropertyState::DefinitelyTrue => ltl::Value::True,
                PropertyState::DefinitelyFalse(violation) => {
                    ltl::Value::False(violation.clone())
                }
            };
            results.push((
                property.name.clone(),
                match value {
                    ltl::Value::True => {
                        property.state = PropertyState::DefinitelyTrue;
                        ltl::Value::True
                    }
                    ltl::Value::False(violation) => {
                        property.state =
                            PropertyState::DefinitelyFalse(violation.clone());
                        ltl::Value::False(violation)
                    }
                    ltl::Value::Residual(residual) => {
                        property.state =
                            PropertyState::Residual(residual.clone());
                        ltl::Value::Residual(residual)
                    }
                },
            ));
        }
        Ok(results)
    }
}

const IGNORED_SYMBOL_EXPORTS: &[JsString] = &[js_string!("Symbol.toStringTag")];

#[derive(Debug, Clone)]
pub struct Property {
    pub name: String,
    state: PropertyState,
}

#[derive(Debug, Clone)]
enum PropertyState {
    Initial(Formula),
    Residual(Residual),
    DefinitelyTrue,
    DefinitelyFalse(Violation),
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        time::{Duration, SystemTime},
    };

    use super::*;

    fn verifier(specification: &str) -> Verifier {
        Verifier::new(Specification {
            path: PathBuf::from("fake.ts"),
            contents: specification.to_string().into_bytes(),
        })
        .unwrap()
    }

    #[test]
    fn test_property_names() {
        let verifier = verifier(
            r#"
            import { always, extract } from "bombadil";

            // Invariant

            const notification_count = extract(
              (state) => state.document.body.querySelectorAll(".notification").length,
            );

            export const max_notifications_shown = always(
              () => notification_count.current <= 5,
            );
            "#,
        );
        assert_eq!(verifier.properties(), vec!["max_notifications_shown"]);
    }

    #[test]
    fn test_extractors() {
        let evaluator = verifier(
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
            "#,
        );

        let mut extractors: Vec<String> = evaluator
            .extractors()
            .unwrap()
            .iter()
            .map(|(_, value)| value.clone())
            .collect();

        extractors.sort();

        assert_eq!(
            extractors,
            vec![
                "(state) => state.foo",
                "function foo(state) { return state.baz; }",
                "s => s.bar",
            ]
        );
    }

    #[test]
    fn test_property_evaluation_always() {
        let mut verifier = verifier(
            r#"
            import { extract, always } from "bombadil";
            
            const foo = extract((state) => state.foo);

            export const my_prop = always(() => foo.current < 100);
            "#,
        );

        let extractor_id =
            verifier.extractors().unwrap().iter().next().unwrap().0;

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        for i in 0..=100 {
            let time = time_at(0);
            let results = verifier
                .step(vec![(extractor_id, json::json!(i))], time)
                .unwrap();

            let (name, value) = results.iter().next().unwrap();
            assert_eq!(*name, "my_prop");

            if i == 100 {
                assert!(matches!(
                    value,
                    ltl::Value::False(Violation::Always {
                        violation: _,
                        subformula: _,
                        ..
                    })
                ))
            } else {
                match value {
                    ltl::Value::Residual(residual) => {
                        match ltl::stop_default(residual, time) {
                            Some(ltl::StopDefault::True) => {}
                            _ => panic!("should have a true stop default"),
                        }
                    }
                    _ => panic!("should be residual"),
                }
            }
        }
    }

    #[test]
    fn test_property_evaluation_eventually() {
        let mut verifier = verifier(
            r#"
            import { extract, eventually } from "bombadil";
            
            const foo = extract((state) => state.foo);

            export const my_prop = eventually(() => foo.current === 9).within(5, "seconds");
            "#,
        );

        let extractor_id =
            verifier.extractors().unwrap().iter().next().unwrap().0;

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        for i in 0..10 {
            let time = time_at(i);
            let results = verifier
                .step(vec![(extractor_id, json::json!(i))], time)
                .unwrap();

            let (name, value) = results.iter().next().unwrap();
            assert_eq!(*name, "my_prop");

            if i == 9 {
                assert!(matches!(value, ltl::Value::True));
            } else {
                match value {
                    ltl::Value::Residual(residual) => {
                        match ltl::stop_default(residual, time) {
                            Some(ltl::StopDefault::False(_)) => {}
                            _ => panic!("should have a false stop default"),
                        }
                    }
                    _ => panic!("should be residual"),
                }
            }
        }
    }

    #[test]
    fn test_load_ts_file() {
        let mut imported_file =
            tempfile::NamedTempFile::with_suffix(".ts").unwrap();
        imported_file
            .write_all(
                r#"
                import { extract } from "bombadil";
                const example = extract((state) => state.example);
                "#
                .as_bytes(),
            )
            .unwrap();

        let verifier = verifier(&format!(
            r#"
            export * from "{}";
            "#,
            imported_file.path().display(),
        ));

        let extractors = verifier.extractors().unwrap();
        let (_, name) = extractors.first().unwrap();
        assert_eq!(name, "(state) => state.example");
    }
}
