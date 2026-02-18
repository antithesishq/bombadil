use std::path::{Path, PathBuf};
use std::{collections::HashMap, rc::Rc};

use crate::specification::js::{
    BombadilExports, Extractors, RuntimeFunction, module_exports,
};
use crate::specification::module_loader::transpile;
use crate::specification::result::Result;
use crate::specification::syntax::Syntax;
use crate::specification::{ltl, module_loader::load_modules};
use crate::tree::Tree;
use boa_engine::{
    Context, JsString, Module, NativeFunction, Source,
    context::ContextBuilder,
    js_string,
    object::builtins::{JsArray, JsUint8Array},
    property::PropertyKey,
};
use boa_engine::{JsError, JsObject, JsValue};
use oxc::span::SourceType;
use serde_json as json;

use crate::specification::{
    ltl::{Evaluator, Formula, Residual, Violation},
    module_loader::{HybridModuleLoader, load_bombadil_module},
    result::SpecificationError,
};

#[derive(Clone, Debug)]
pub struct Specification {
    contents: Vec<u8>,
    path: PathBuf,
}

impl Specification {
    pub async fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = tokio::fs::read_to_string(path)
            .await
            .map_err(SpecificationError::IO)?;
        Self::from_string(&contents, path)
    }
    pub fn from_string(contents: &str, path: impl AsRef<Path>) -> Result<Self> {
        let path: &Path = path.as_ref();
        let source_type = SourceType::from_path(path).map_err(|error| {
            SpecificationError::OtherError(error.to_string())
        })?;
        let contents =
            if [SourceType::cjs(), SourceType::mjs()].contains(&source_type) {
                contents.to_string()
            } else {
                log::debug!(
                    "transpiling {} ({:?}) to javascript",
                    path.display(),
                    &source_type,
                );
                transpile(contents, path, &source_type)?
            };
        Ok(Specification {
            contents: contents.into_bytes(),
            path: path.to_path_buf(),
        })
    }
}

#[derive(Clone)]
pub struct StepResult<A> {
    pub properties: Vec<(String, ltl::Value<RuntimeFunction>)>,
    pub actions: Tree<A>,
}

pub struct Verifier {
    context: Context,
    bombadil_exports: BombadilExports,
    properties: HashMap<String, Property>,
    action_generators: HashMap<String, ActionGenerator>,
    extractors: Extractors,
    extractor_functions: HashMap<u64, String>,
}

const RANDOM_BYTES_COUNT_MAX: usize = 4096;

impl Verifier {
    pub fn new(specification: Specification) -> Result<Self> {
        let loader = Rc::new(HybridModuleLoader::new()?);

        // Instantiate the execution context
        let mut context = ContextBuilder::default()
            .module_loader(loader.clone())
            .build()
            .map_err(|error| SpecificationError::JS(error.to_string()))?;

        // Expose random byte generation to JS
        context.register_global_builtin_callable(
            js_string!("__bombadil_random_bytes"),
            1,
            NativeFunction::from_copy_closure(|_this, args, context| {
                let n = args
                    .first()
                    .map(|v| v.to_u32(context))
                    .transpose()?
                    .unwrap_or(0) as usize;
                if n > RANDOM_BYTES_COUNT_MAX {
                    return Err(JsError::from_rust(SpecificationError::JS(
                        format!(
                            "n cannot be larger than {RANDOM_BYTES_COUNT_MAX}"
                        ),
                    )));
                }
                let mut buf = vec![0u8; n];
                rand::fill(&mut buf[..]);
                Ok(JsUint8Array::from_iter(buf, context)?.into())
            }),
        )?;

        // Non-special modules loaded in dependency order
        let modules = [
            ("internal.js", "@antithesishq/bombadil/internal"),
            ("random.js", "@antithesishq/bombadil/random"),
            ("actions.js", "@antithesishq/bombadil/actions"),
        ];
        for (file, import_path) in modules {
            let module = load_bombadil_module(file, &mut context)?;
            loader.insert_mapped_module(import_path, module);
        }

        // Index module — special: needed for BombadilExports
        let bombadil_module_index = {
            let module = load_bombadil_module("index.js", &mut context)?;
            loader
                .insert_mapped_module("@antithesishq/bombadil", module.clone());
            module
        };

        // Modules that depend on index
        let modules = [
            (
                "defaults/actions.js",
                "@antithesishq/bombadil/defaults/actions",
            ),
            (
                "defaults/properties.js",
                "@antithesishq/bombadil/defaults/properties",
            ),
            ("defaults.js", "@antithesishq/bombadil/defaults"),
        ];
        for (file, import_path) in modules {
            let module = load_bombadil_module(file, &mut context)?;
            loader.insert_mapped_module(import_path, module);
        }

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
        load_modules(
            &mut context,
            std::slice::from_ref(&specification_module),
        )?;

        let specification_exports =
            module_exports(&specification_module, &mut context)?;
        let bombadil_exports =
            BombadilExports::from_module(&bombadil_module_index, &mut context)?;

        let mut properties: HashMap<String, Property> = HashMap::new();
        let mut action_generators: HashMap<String, ActionGenerator> =
            HashMap::new();
        for (key, value) in specification_exports.iter() {
            if value.instance_of(&bombadil_exports.formula, &mut context)? {
                let syntax =
                    Syntax::from_value(value, &bombadil_exports, &mut context)?;
                let formula = syntax.nnf();
                properties.insert(
                    key.to_string(),
                    Property {
                        name: key.to_string(),
                        state: PropertyState::Initial(formula),
                    },
                );
            } else if value
                .instance_of(&bombadil_exports.action_generator, &mut context)?
            {
                let object = value.as_object().ok_or(
                    SpecificationError::OtherError(format!(
                        "action generator {} is not an object, it is {}",
                        key,
                        value.type_of()
                    )),
                )?;
                let function = object
                    .get(js_string!("generate"), &mut context)
                    .map_err(|error| SpecificationError::JS(error.to_string()))?
                    .as_object()
                    .ok_or(SpecificationError::OtherError(format!(
                        "action {} is not a function, it is {}",
                        key,
                        value.type_of()
                    )))?;
                action_generators.insert(
                    key.to_string(),
                    ActionGenerator {
                        name: key.to_string(),
                        this: value.clone(),
                        function,
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

        if action_generators.is_empty() {
            return Err(SpecificationError::OtherError(
                "specification exports no action generators".to_string(),
            ));
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
            action_generators,
            bombadil_exports,
            extractors,
            extractor_functions,
        })
    }

    pub fn properties(&self) -> Vec<String> {
        self.properties.keys().cloned().collect()
    }

    pub fn extractors(&self) -> Result<Vec<(u64, String)>> {
        let mut results = Vec::with_capacity(self.extractor_functions.len());
        for (key, value) in &self.extractor_functions {
            results.push((*key, value.clone()));
        }
        Ok(results)
    }

    pub fn step<A: serde::de::DeserializeOwned>(
        &mut self,
        snapshots: Vec<(u64, json::Value)>,
        time: ltl::Time,
    ) -> Result<StepResult<A>> {
        self.extractors.update_from_snapshots(
            snapshots,
            time,
            &mut self.context,
        )?;
        let mut result_properties = Vec::with_capacity(self.properties.len());
        let mut generator_branches: Vec<(u16, Tree<A>)> = Vec::new();

        let context = &mut self.context;
        let mut evaluate_thunk = |function: &RuntimeFunction,
                                  negated: bool|
         -> Result<Formula<RuntimeFunction>> {
            let value =
                function.object.call(&JsValue::undefined(), &[], context)?;
            let syntax =
                Syntax::from_value(&value, &self.bombadil_exports, context)?;
            Ok((if negated {
                Syntax::Not(Box::new(syntax))
            } else {
                syntax
            })
            .nnf())
        };
        let mut evaluator = Evaluator::new(&mut evaluate_thunk);

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
            result_properties.push((
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

        for action_generator in self.action_generators.values() {
            // All exported generators are weighted equally.
            generator_branches.push((1, action_generator.generate(context)?));
        }

        let action_tree = Tree::Branch {
            branches: generator_branches,
        };

        Ok(StepResult {
            properties: result_properties,
            actions: action_tree,
        })
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
    Initial(Formula<RuntimeFunction>),
    Residual(Residual<RuntimeFunction>),
    DefinitelyTrue,
    DefinitelyFalse(Violation<RuntimeFunction>),
}

#[derive(Debug, Clone)]
pub struct ActionGenerator {
    pub name: String,
    this: JsValue,
    function: JsObject,
}

impl ActionGenerator {
    fn generate<A: serde::de::DeserializeOwned>(
        &self,
        context: &mut Context,
    ) -> Result<Tree<A>> {
        let value = self.function.call(&self.this, &[], context)?;
        let actions_json =
            value
                .to_json(context)?
                .ok_or(SpecificationError::OtherError(format!(
                    "action generator {} returned undefined",
                    self.name
                )))?;
        let tree: Tree<A> =
            json::from_value(actions_json).map_err(|error| {
                SpecificationError::OtherError(format!(
                    "failed to convert JSON object from `{}` to action, {}: {}",
                    self.name,
                    error,
                    value.display(),
                ))
            })?;
        Ok(tree)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        time::{Duration, SystemTime},
    };

    use crate::specification::stop::{StopDefault, stop_default};

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
            import { actions, always, extract } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

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
            import { actions, extract } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

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
    fn test_property_evaluation_not() {
        let mut verifier = verifier(
            r#"
            import { actions, extract, now } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = now(() => foo.current).not();
            "#,
        );

        let extractors = verifier.extractors().unwrap();
        let extractor_foo_id = extractors.first().unwrap().0;

        let time = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_millis(0))
            .unwrap();

        let result: StepResult<json::Value> = verifier
            .step(vec![(extractor_foo_id, json::json!(false))], time)
            .unwrap();

        let (name, value) = result.properties.first().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(matches!(value, ltl::Value::True));
    }

    #[test]
    fn test_property_evaluation_and() {
        let mut verifier = verifier(
            r#"
            import { actions, extract, now } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);
            const bar = extract((state) => state.bar);

            export const my_prop = now(() => foo.current).and(() => bar.current);
            "#,
        );

        let extractors = verifier.extractors().unwrap();
        let extractor_foo_id = extractors.first().unwrap().0;
        let extractor_bar_id = extractors.get(1).unwrap().0;

        let time = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_millis(0))
            .unwrap();

        let result: StepResult<json::Value> = verifier
            .step(
                vec![
                    (extractor_foo_id, json::json!(true)),
                    (extractor_bar_id, json::json!(true)),
                ],
                time,
            )
            .unwrap();

        let (name, value) = result.properties.first().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(matches!(value, ltl::Value::True));
    }

    #[test]
    fn test_property_evaluation_or() {
        let mut verifier = verifier(
            r#"
            import { actions, extract, now } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);
            const bar = extract((state) => state.bar);

            export const my_prop = now(() => foo.current).or(() => bar.current);
            "#,
        );

        let extractors = verifier.extractors().unwrap();
        let extractor_foo_id = extractors.first().unwrap().0;
        let extractor_bar_id = extractors.get(1).unwrap().0;

        let time = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_millis(0))
            .unwrap();

        let result: StepResult<json::Value> = verifier
            .step(
                vec![
                    (extractor_foo_id, json::json!(false)),
                    (extractor_bar_id, json::json!(true)),
                ],
                time,
            )
            .unwrap();

        let (name, value) = result.properties.first().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(matches!(value, ltl::Value::True));
    }

    #[test]
    fn test_property_evaluation_implies() {
        let mut verifier = verifier(
            r#"
            import { actions, extract, now } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);
            const bar = extract((state) => state.bar);

            export const my_prop = now(() => foo.current).implies(() => bar.current);
            "#,
        );

        let extractors = verifier.extractors().unwrap();
        let extractor_foo_id = extractors.first().unwrap().0;
        let extractor_bar_id = extractors.get(1).unwrap().0;

        let time = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_millis(0))
            .unwrap();

        let result: StepResult<json::Value> = verifier
            .step(
                vec![
                    (extractor_foo_id, json::json!(false)),
                    (extractor_bar_id, json::json!(false)),
                ],
                time,
            )
            .unwrap();

        let (name, value) = result.properties.first().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(matches!(value, ltl::Value::True));
    }

    #[test]
    fn test_property_evaluation_next() {
        let mut verifier = verifier(
            r#"
            import { actions, extract, next } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = next(() => foo.current === 1);
            "#,
        );

        let extractor_id = verifier.extractors().unwrap().first().unwrap().0;

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        for i in 0..=1 {
            let time = time_at(i);
            let result: StepResult<json::Value> = verifier
                .step(vec![(extractor_id, json::json!(i))], time)
                .unwrap();

            let (name, value) = result.properties.first().unwrap();
            assert_eq!(*name, "my_prop");

            if i == 1 {
                assert!(matches!(value, ltl::Value::True));
            } else {
                match value {
                    ltl::Value::Residual(residual) => {
                        match stop_default(residual, time) {
                            Some(StopDefault::True) => {}
                            _ => panic!("should have a true stop default"),
                        }
                    }
                    _ => panic!("should be residual but was: {:?}", value),
                }
            }
        }
    }

    #[test]
    fn test_property_evaluation_always() {
        let mut verifier = verifier(
            r#"
            import { extract, always, actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = always(() => foo.current < 100);
            "#,
        );

        let extractor_id = verifier.extractors().unwrap().first().unwrap().0;

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        for i in 0..=100 {
            let time = time_at(0);
            let result: StepResult<json::Value> = verifier
                .step(vec![(extractor_id, json::json!(i))], time)
                .unwrap();

            let (name, value) = result.properties.first().unwrap();
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
                        match stop_default(residual, time) {
                            Some(StopDefault::True) => {}
                            _ => panic!("should have a true stop default"),
                        }
                    }
                    _ => panic!("should be residual"),
                }
            }
        }
    }

    #[test]
    fn test_property_evaluation_always_bounded() {
        let mut verifier = verifier(
            r#"
            import { extract, always, actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = always(() => foo.current < 4).within(3, "milliseconds");
            "#,
        );

        let extractor_id = verifier.extractors().unwrap().first().unwrap().0;

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        for i in 0..10 {
            let time = time_at(i);
            let result: StepResult<json::Value> = verifier
                .step(vec![(extractor_id, json::json!(i))], time)
                .unwrap();

            let (name, value) = result.properties.first().unwrap();
            assert_eq!(*name, "my_prop");

            if i < 4 {
                match value {
                    ltl::Value::Residual(residual) => {
                        match stop_default(residual, time) {
                            Some(StopDefault::True) => {}
                            _ => panic!("should have a true stop default"),
                        }
                    }
                    other => panic!("should be residual but was: {:?}", other),
                }
            } else {
                assert!(matches!(value, ltl::Value::True));
            }
        }
    }

    #[test]
    fn test_property_evaluation_eventually() {
        let mut verifier = verifier(
            r#"
            import { actions, extract, eventually } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = eventually(() => foo.current === 9);
            "#,
        );

        let extractor_id = verifier.extractors().unwrap().first().unwrap().0;

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        for i in 0..10 {
            let time = time_at(i);
            let result: StepResult<json::Value> = verifier
                .step(vec![(extractor_id, json::json!(i))], time)
                .unwrap();

            let (name, value) = result.properties.first().unwrap();
            assert_eq!(*name, "my_prop");

            if i == 9 {
                assert!(matches!(value, ltl::Value::True));
            } else {
                match value {
                    ltl::Value::Residual(residual) => {
                        match stop_default(residual, time) {
                            Some(StopDefault::False(_)) => {}
                            _ => panic!("should have a false stop default"),
                        }
                    }
                    _ => panic!("should be residual"),
                }
            }
        }
    }

    #[test]
    fn test_property_evaluation_eventually_bounded() {
        let mut verifier = verifier(
            r#"
            import { actions, extract, eventually } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = eventually(() => foo.current === 9).within(3, "milliseconds");
            "#,
        );

        let extractor_id = verifier.extractors().unwrap().first().unwrap().0;

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        for i in 0..10 {
            let time = time_at(i);
            let result: StepResult<json::Value> = verifier
                .step(vec![(extractor_id, json::json!(i))], time)
                .unwrap();

            let (name, value) = result.properties.first().unwrap();
            assert_eq!(*name, "my_prop");

            if i < 4 {
                match value {
                    ltl::Value::Residual(residual) => {
                        match stop_default(residual, time) {
                            Some(StopDefault::False(_)) => {}
                            _ => panic!("should have a false stop default"),
                        }
                    }
                    other => panic!("should be residual but was: {:?}", other),
                }
            } else {
                assert!(matches!(value, ltl::Value::False(_)));
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
                import { extract } from "@antithesishq/bombadil";
                const example = extract((state) => state.example);
                "#
                .as_bytes(),
            )
            .unwrap();

        let verifier = verifier(&format!(
            r#"
            import {{ actions }} from "@antithesishq/bombadil";
            export const _actions = actions(() => []);
            export * from "{}";
            "#,
            imported_file.path().display(),
        ));

        let extractors = verifier.extractors().unwrap();
        let (_, name) = extractors.first().unwrap();
        assert_eq!(name, "(state) => state.example");
    }
}
