use std::collections::HashMap;

use crate::specification::js::{BombadilExports, Extractors, RuntimeFunction};
use crate::specification::ltl::{Evaluator, Formula, Residual};
use crate::specification::result::Result;
use crate::specification::syntax::Syntax;
use crate::specification::{ltl, result::SpecificationError};
use crate::tree::Tree;
use bit_set::BitSet;
use boa_engine::{
    Context, JsString, NativeFunction, Source,
    context::ContextBuilder,
    js_string,
    object::builtins::{JsArray, JsUint8Array},
    property::PropertyKey,
};
use boa_engine::{JsError, JsObject, JsValue};
use serde::{Deserialize, Serialize};
use serde_json as json;

#[derive(Clone)]
pub struct StepResult<A> {
    pub properties: Vec<(String, ltl::Value<RuntimeFunction>)>,
    pub actions: Tree<A>,
    pub has_pending: bool,
}

pub struct Verifier {
    context: Context,
    bombadil_exports: BombadilExports,
    properties: HashMap<String, Property>,
    action_generators: HashMap<String, ActionGenerator>,
    extractors: Extractors,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub name: Option<String>,
    pub value: json::Value,
}

const RANDOM_BYTES_COUNT_MAX: usize = 4096;

#[derive(Clone)]
pub struct Specification {
    pub module_specifier: String,
}

fn format_console_args(args: &[JsValue]) -> String {
    args.iter()
        .map(|v| match v.as_string() {
            Some(s) => s.to_std_string_escaped(),
            None => v.display().to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Verifier {
    pub fn new(bundle_code: &str) -> Result<Self> {
        let mut context = ContextBuilder::default()
            .build()
            .map_err(|error| SpecificationError::JS(error.to_string()))?;

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

        // Add console object for compatibility with libraries that use console
        let console_obj =
            boa_engine::object::ObjectInitializer::new(&mut context)
                .function(
                    NativeFunction::from_copy_closure(
                        |_this, args, _context| {
                            log::info!("{}", format_console_args(args));
                            Ok(JsValue::undefined())
                        },
                    ),
                    js_string!("log"),
                    0,
                )
                .function(
                    NativeFunction::from_copy_closure(
                        |_this, args, _context| {
                            log::warn!("{}", format_console_args(args));
                            Ok(JsValue::undefined())
                        },
                    ),
                    js_string!("warn"),
                    0,
                )
                .function(
                    NativeFunction::from_copy_closure(
                        |_this, args, _context| {
                            log::error!("{}", format_console_args(args));
                            Ok(JsValue::undefined())
                        },
                    ),
                    js_string!("error"),
                    0,
                )
                .build();
        context
            .register_global_property(
                js_string!("console"),
                console_obj,
                boa_engine::property::Attribute::all(),
            )
            .map_err(|e| {
                SpecificationError::JS(format!(
                    "Failed to register console: {}",
                    e
                ))
            })?;

        let specification_exports_value =
            context.eval(Source::from_bytes(bundle_code))?;
        let specification_exports_obj = specification_exports_value
            .as_object()
            .ok_or(SpecificationError::OtherError(
                "specification exports is not an object".to_string(),
            ))?;

        let require_fn = context
            .global_object()
            .get(js_string!("__bombadilRequire"), &mut context)?
            .as_callable()
            .ok_or(SpecificationError::OtherError(
                "__bombadilRequire is not a function".to_string(),
            ))?;

        let bombadil_exports_value = require_fn.call(
            &JsValue::undefined(),
            &[js_string!("@antithesishq/bombadil").into()],
            &mut context,
        )?;
        let bombadil_exports_obj = bombadil_exports_value.as_object().ok_or(
            SpecificationError::OtherError(
                "bombadil exports is not an object".to_string(),
            ),
        )?;

        let bombadil_exports =
            BombadilExports::from_object(&bombadil_exports_obj, &mut context)?;

        let specification_export_keys =
            specification_exports_obj.own_property_keys(&mut context)?;

        let mut properties: HashMap<String, Property> = HashMap::new();
        let mut action_generators: HashMap<String, ActionGenerator> =
            HashMap::new();
        for key in specification_export_keys {
            let value =
                specification_exports_obj.get(key.clone(), &mut context)?;
            if value.instance_of(&bombadil_exports.formula, &mut context)? {
                let syntax = Syntax::from_value(
                    &value,
                    &bombadil_exports,
                    &mut context,
                )?;
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
            } else if let PropertyKey::Symbol(ref symbol) = key
                && let Some(description) = symbol.description()
                && IGNORED_SYMBOL_EXPORTS.contains(&description)
            {
                continue;
            } else if IGNORED_STRING_EXPORTS.contains(&key.to_string().as_str())
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
            .runtime
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

        Ok(Verifier {
            context,
            properties,
            action_generators,
            bombadil_exports,
            extractors,
        })
    }

    pub fn properties(&self) -> Vec<String> {
        self.properties.keys().cloned().collect()
    }

    pub fn step<A: serde::de::DeserializeOwned>(
        &mut self,
        snapshots: &[Snapshot],
        time: ltl::Time,
    ) -> Result<StepResult<A>> {
        self.extractors.update_from_snapshots(
            snapshots,
            time,
            &mut self.context,
        )?;
        let mut result_properties = Vec::with_capacity(self.properties.len());
        let mut generator_branches: Vec<(u16, Tree<A>)> = Vec::new();

        let runtime_obj = self.bombadil_exports.runtime.clone();
        let start_tracking = runtime_obj
            .get(js_string!("startTracking"), &mut self.context)?
            .as_callable()
            .ok_or(SpecificationError::OtherError(
                "startTracking is not callable".to_string(),
            ))?;
        let stop_tracking = runtime_obj
            .get(js_string!("stopTracking"), &mut self.context)?
            .as_callable()
            .ok_or(SpecificationError::OtherError(
                "stopTracking is not callable".to_string(),
            ))?;

        let context = &mut self.context;
        let mut evaluate_thunk = |function: &RuntimeFunction,
                                  negated: bool|
         -> Result<(
            Formula<RuntimeFunction>,
            BitSet,
        )> {
            start_tracking.call(
                &JsValue::from(runtime_obj.clone()),
                &[],
                context,
            )?;
            let value =
                function.object.call(&JsValue::undefined(), &[], context)?;
            let accesses_js = stop_tracking.call(
                &JsValue::from(runtime_obj.clone()),
                &[],
                context,
            )?;
            let accessed = crate::specification::js::js_value_to_extractor_set(
                &accesses_js,
                context,
            )?;
            let syntax =
                Syntax::from_value(&value, &self.bombadil_exports, context)?;
            Ok((
                (if negated {
                    Syntax::Not(Box::new(syntax))
                } else {
                    syntax
                })
                .nnf(),
                accessed,
            ))
        };
        let mut evaluator = Evaluator::new(&mut evaluate_thunk, snapshots);

        for property in self.properties.values_mut() {
            let value = match &property.state {
                PropertyState::Initial(formula) => {
                    evaluator.evaluate(formula, time)?
                }
                PropertyState::Residual(residual) => {
                    evaluator.step(residual, time)?
                }
                PropertyState::DefinitelyTrue
                | PropertyState::DefinitelyFalse => continue,
            };
            result_properties.push((
                property.name.clone(),
                match value {
                    ltl::Value::True(_) => {
                        property.state = PropertyState::DefinitelyTrue;
                        ltl::Value::True(vec![])
                    }
                    ltl::Value::False(violation, continuation) => {
                        property.state = match continuation {
                            Some(residual) => PropertyState::Residual(residual),
                            None => PropertyState::DefinitelyFalse,
                        };
                        ltl::Value::False(violation, None)
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

        let has_pending = self.properties.values().any(|p| {
            matches!(
                &p.state,
                PropertyState::Initial(_) | PropertyState::Residual(_)
            )
        });

        Ok(StepResult {
            properties: result_properties,
            actions: action_tree,
            has_pending,
        })
    }
}

const IGNORED_SYMBOL_EXPORTS: &[JsString] = &[js_string!("Symbol.toStringTag")];
const IGNORED_STRING_EXPORTS: &[&str] = &["__esModule"];

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
    DefinitelyFalse,
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

    use tempfile::NamedTempFile;

    use crate::specification::stop::{StopDefault, stop_default};

    use super::*;

    fn verifier(specification: &str) -> Verifier {
        use crate::specification::bundler::bundle;

        let mut specification_file = NamedTempFile::with_suffix(".ts").unwrap();
        specification_file
            .write_all(specification.as_bytes())
            .unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let bundle_code = rt
            .block_on(bundle(
                ".",
                &specification_file.path().display().to_string(),
            ))
            .unwrap();

        Verifier::new(&bundle_code).unwrap()
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
    fn test_property_evaluation_not() {
        let mut verifier = verifier(
            r#"
            import { actions, extract, now } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = now(() => foo.current).not();
            "#,
        );

        let time = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_millis(0))
            .unwrap();

        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    name: None,
                    value: json::json!(false),
                }],
                time,
            )
            .unwrap();

        let (name, value) = result.properties.first().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(matches!(value, ltl::Value::True(_)));
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

        let time = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_millis(0))
            .unwrap();

        let result: StepResult<Snapshot> = verifier
            .step(
                &[
                    Snapshot {
                        name: None,
                        value: json::json!(true),
                    },
                    Snapshot {
                        name: None,
                        value: json::json!(true),
                    },
                ],
                time,
            )
            .unwrap();

        let (name, value) = result.properties.first().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(matches!(value, ltl::Value::True(_)));
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

        let time = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_millis(0))
            .unwrap();

        let result: StepResult<Snapshot> = verifier
            .step(
                &[
                    Snapshot {
                        name: None,
                        value: json::json!(false),
                    },
                    Snapshot {
                        name: None,
                        value: json::json!(true),
                    },
                ],
                time,
            )
            .unwrap();

        let (name, value) = result.properties.first().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(matches!(value, ltl::Value::True(_)));
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

        let time = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_millis(0))
            .unwrap();

        let result: StepResult<Snapshot> = verifier
            .step(
                &[
                    Snapshot {
                        name: None,
                        value: json::json!(false),
                    },
                    Snapshot {
                        name: None,
                        value: json::json!(false),
                    },
                ],
                time,
            )
            .unwrap();

        let (name, value) = result.properties.first().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(matches!(value, ltl::Value::True(_)));
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

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        for i in 0..=1 {
            let time = time_at(i);
            let result: StepResult<Snapshot> = verifier
                .step(
                    &[Snapshot {
                        name: None,
                        value: json::json!(i),
                    }],
                    time,
                )
                .unwrap();

            let (name, value) = result.properties.first().unwrap();
            assert_eq!(*name, "my_prop");

            if i == 1 {
                assert!(matches!(value, ltl::Value::True(_)));
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

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        for i in 0..=100 {
            let time = time_at(0);
            let result: StepResult<Snapshot> = verifier
                .step(
                    &[Snapshot {
                        name: None,
                        value: json::json!(i),
                    }],
                    time,
                )
                .unwrap();

            let (name, value) = result.properties.first().unwrap();
            assert_eq!(*name, "my_prop");

            if i == 100 {
                assert!(matches!(
                    value,
                    ltl::Value::False(
                        ltl::Violation::Always {
                            violation: _,
                            subformula: _,
                            ..
                        },
                        _
                    )
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

        // After the violation at i=100, the property should reset to
        // Residual when given a passing value.
        let time = time_at(0);
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    name: None,
                    value: json::json!(0),
                }],
                time,
            )
            .unwrap();
        let (name, value) = result.properties.first().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(
            matches!(value, ltl::Value::Residual(_)),
            "expected Residual after reset, got: {:?}",
            value,
        );
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

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        for i in 0..10 {
            let time = time_at(i);
            let result: StepResult<Snapshot> = verifier
                .step(
                    &[Snapshot {
                        name: None,
                        value: json::json!(i),
                    }],
                    time,
                )
                .unwrap();

            if let Some((name, value)) = result.properties.first() {
                assert_eq!(*name, "my_prop");

                if i < 4 {
                    match value {
                        ltl::Value::Residual(residual) => {
                            match stop_default(residual, time) {
                                Some(StopDefault::True) => {}
                                _ => {
                                    panic!("should have a true stop default")
                                }
                            }
                        }
                        other => {
                            panic!("should be residual but was: {:?}", other)
                        }
                    }
                } else {
                    assert!(matches!(value, ltl::Value::True(_)));
                }
            } else {
                assert!(i > 4, "property should still be pending at i={}", i);
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

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        for i in 0..10 {
            let time = time_at(i);
            let result: StepResult<Snapshot> = verifier
                .step(
                    &[Snapshot {
                        name: None,
                        value: json::json!(i),
                    }],
                    time,
                )
                .unwrap();

            let (name, value) = result.properties.first().unwrap();
            assert_eq!(*name, "my_prop");

            if i == 9 {
                assert!(matches!(value, ltl::Value::True(_)));
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
    fn test_format_console_args() {
        assert_eq!(format_console_args(&[]), "");
        assert_eq!(
            format_console_args(&[JsValue::from(js_string!("hello"))]),
            "hello"
        );
        assert_eq!(
            format_console_args(&[
                JsValue::from(js_string!("count:")),
                JsValue::from(42),
                JsValue::from(true),
            ]),
            "count: 42 true"
        );
        assert_eq!(format_console_args(&[JsValue::undefined()]), "undefined");
        assert_eq!(format_console_args(&[JsValue::null()]), "null");
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

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        for i in 0..10 {
            let time = time_at(i);
            let result: StepResult<Snapshot> = verifier
                .step(
                    &[Snapshot {
                        name: None,
                        value: json::json!(i),
                    }],
                    time,
                )
                .unwrap();

            if let Some((name, value)) = result.properties.first() {
                assert_eq!(*name, "my_prop");

                if i < 4 {
                    match value {
                        ltl::Value::Residual(residual) => {
                            match stop_default(residual, time) {
                                Some(StopDefault::False(_)) => {}
                                _ => {
                                    panic!("should have a false stop default")
                                }
                            }
                        }
                        other => {
                            panic!("should be residual but was: {:?}", other)
                        }
                    }
                } else {
                    assert!(matches!(value, ltl::Value::False(_, _)));
                }
            } else {
                assert!(i > 4, "property should still be pending at i={}", i);
            }
        }
    }

    #[test]
    fn test_always_resets_after_violation() {
        let mut verifier = verifier(
            r#"
            import { extract, always, actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = always(() => foo.current < 5);
            "#,
        );

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        // Steps 0-4: Residual (passing)
        for i in 0..5 {
            let result: StepResult<Snapshot> = verifier
                .step(
                    &[Snapshot {
                        name: None,
                        value: json::json!(i),
                    }],
                    time_at(0),
                )
                .unwrap();
            let (_, value) = result.properties.first().unwrap();
            assert!(
                matches!(value, ltl::Value::Residual(_)),
                "expected Residual at i={}, got: {:?}",
                i,
                value,
            );
        }

        // Step with value 5: False (violation)
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    name: None,
                    value: json::json!(5),
                }],
                time_at(0),
            )
            .unwrap();
        let (_, value) = result.properties.first().unwrap();
        assert!(
            matches!(value, ltl::Value::False(_, _)),
            "expected False at value=5, got: {:?}",
            value,
        );

        // Step with value 0: should reset to Residual (not repeat False)
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    name: None,
                    value: json::json!(0),
                }],
                time_at(0),
            )
            .unwrap();
        let (_, value) = result.properties.first().unwrap();
        assert!(
            matches!(value, ltl::Value::Residual(_)),
            "expected Residual after reset, got: {:?}",
            value,
        );

        // Step with value 5 again: should produce a new False
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    name: None,
                    value: json::json!(5),
                }],
                time_at(0),
            )
            .unwrap();
        let (_, value) = result.properties.first().unwrap();
        assert!(
            matches!(value, ltl::Value::False(_, _)),
            "expected new False at value=5, got: {:?}",
            value,
        );
    }

    #[test]
    fn test_now_false_is_terminal() {
        let mut verifier = verifier(
            r#"
            import { extract, now, actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = now(() => foo.current);
            "#,
        );

        let time = SystemTime::UNIX_EPOCH;

        // First step: False (no continuation)
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    name: None,
                    value: json::json!(false),
                }],
                time,
            )
            .unwrap();
        let (name, value) = result.properties.first().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(
            matches!(value, ltl::Value::False(_, None)),
            "expected terminal False, got: {:?}",
            value,
        );

        // Subsequent step: property is settled, no result emitted
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    name: None,
                    value: json::json!(true),
                }],
                time,
            )
            .unwrap();
        assert!(
            result.properties.is_empty(),
            "expected no properties after terminal False, got: {:?}",
            result.properties,
        );
        assert!(!result.has_pending);
    }

    #[test]
    fn test_always_bounded_continues_after_violation() {
        let mut verifier = verifier(
            r#"
            import { extract, always, actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = always(() => foo.current < 10).within(5, "milliseconds");
            "#,
        );

        let time_at = |i: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(i))
                .unwrap()
        };

        // At time 0, value 0: Residual
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    name: None,
                    value: json::json!(0),
                }],
                time_at(0),
            )
            .unwrap();
        let (_, value) = result.properties.first().unwrap();
        assert!(matches!(value, ltl::Value::Residual(_)));

        // At time 3ms, value 10: fails the always, but has continuation
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    name: None,
                    value: json::json!(10),
                }],
                time_at(3),
            )
            .unwrap();
        let (_, value) = result.properties.first().unwrap();
        assert!(
            matches!(value, ltl::Value::False(_, _)),
            "expected False at time 3ms, got: {:?}",
            value,
        );

        // At time 4ms, value 0: should be Residual (reset via continuation)
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    name: None,
                    value: json::json!(0),
                }],
                time_at(4),
            )
            .unwrap();
        let (_, value) = result.properties.first().unwrap();
        assert!(
            matches!(value, ltl::Value::Residual(_)),
            "expected Residual at time 4ms after reset, got: {:?}",
            value,
        );

        // At time 6ms (past the 5ms bound): should resolve to True
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    name: None,
                    value: json::json!(0),
                }],
                time_at(6),
            )
            .unwrap();
        let (_, value) = result.properties.first().unwrap();
        assert!(
            matches!(value, ltl::Value::True(_)),
            "expected True past the bound, got: {:?}",
            value,
        );
    }
}
