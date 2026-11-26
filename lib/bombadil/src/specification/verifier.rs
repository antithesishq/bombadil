use std::collections::HashMap;

use crate::driver::FromGeneratedAction;
use crate::specification::engine::{Engine, display_value, to_json, type_of};
use crate::specification::js::{
    BombadilExports, Extractors, RuntimeFunction, syntax_from_value,
};
use crate::specification::result::{JsResultExt, Result, SpecificationError};
use crate::specification::snapshots::with_snapshot_tracking;
use crate::tree::Tree;
use bombadil_ltl::eval::{self, Evaluator};
use bombadil_ltl::formula::Formula;
use bombadil_ltl::syntax::Syntax;
use rquickjs::{
    Ctx, Function, Persistent, Symbol, Value, function::This, object::Filter,
};
use serde_json as json;

use crate::specification::domain::{BombadilDomain, Snapshot, UniqueSnapshots};

#[derive(Clone)]
pub struct StepResult<'a, A> {
    properties: &'a HashMap<String, Property>,
    pub actions: Tree<A>,
    pub all_definite: bool,
}

impl<A> StepResult<'_, A> {
    pub fn properties(
        &self,
    ) -> impl Iterator<Item = (&String, &eval::Value<BombadilDomain<RuntimeFunction>>)>
    {
        self.properties
            .values()
            .filter_map(|property| match &property.state {
                PropertyState::Evaluated(value) => {
                    Some((&property.name, value))
                }
                _ => None,
            })
    }
}

type LoadedSpecification = (
    HashMap<String, Property>,
    HashMap<String, ActionGenerator>,
    Extractors,
    BombadilExports,
);

pub struct Verifier {
    properties: HashMap<String, Property>,
    action_generators: HashMap<String, ActionGenerator>,
    extractors: Extractors,
    bombadil_exports: BombadilExports,
    engine: Engine,
}

#[derive(Clone)]
pub struct Specification {
    pub module_specifier: String,
}

impl Verifier {
    pub fn new(bundle_code: &str) -> Result<Self> {
        let engine = Engine::new()
            .map_err(|error| SpecificationError::JS(error.to_string()))?;

        let (properties, action_generators, extractors, bombadil_exports) =
            engine.context.with(
                |ctx| -> Result<LoadedSpecification> {
                    let specification_exports_value =
                        ctx.eval::<Value, _>(bundle_code).js(&ctx)?;
                    let specification_exports = specification_exports_value
                        .as_object()
                        .ok_or(SpecificationError::OtherError(
                            "specification exports is not an object".to_string(),
                        ))?;

                    let require_value =
                        ctx.globals().get::<_, Value>("__bombadilRequire").js(&ctx)?;
                    let require_fn = require_value.as_function().ok_or(
                        SpecificationError::OtherError(
                            "__bombadilRequire is not a function".to_string(),
                        ),
                    )?;

                    let bombadil_exports_value = require_fn
                        .call::<_, Value>(("@antithesishq/bombadil",))
                        .js(&ctx)?;
                    let bombadil_exports_obj = bombadil_exports_value
                        .as_object()
                        .ok_or(SpecificationError::OtherError(
                            "bombadil exports is not an object".to_string(),
                        ))?;

                    let bombadil_exports =
                        BombadilExports::from_object(bombadil_exports_obj, &ctx)?;

                    let formula_class =
                        bombadil_exports.formula.clone().restore(&ctx).js(&ctx)?;
                    let action_generator_class = bombadil_exports
                        .action_generator
                        .clone()
                        .restore(&ctx)
                        .js(&ctx)?;
                    let instance_of = ctx
                        .eval::<Function, _>(
                            "(value, ctor) => value instanceof ctor",
                        )
                        .js(&ctx)?;

                    let mut properties: HashMap<String, Property> =
                        HashMap::new();
                    let mut action_generators: HashMap<String, ActionGenerator> =
                        HashMap::new();
                    for key in specification_exports
                        .own_keys::<Value>(Filter::new().string().symbol())
                    {
                        let key = key.js(&ctx)?;
                        let name = key_name(&key);
                        let value =
                            specification_exports.get::<_, Value>(key.clone()).js(&ctx)?;
                        if is_instance(&instance_of, &value, &formula_class, &ctx)? {
                            let syntax =
                                syntax_from_value(&value, &bombadil_exports, &ctx)?;
                            let formula = syntax.nnf();
                            properties.insert(
                                name.clone(),
                                Property {
                                    name,
                                    state: PropertyState::Initial(formula),
                                },
                            );
                        } else if is_instance(
                            &instance_of,
                            &value,
                            &action_generator_class,
                            &ctx,
                        )? {
                            let object = value.as_object().ok_or(
                                SpecificationError::OtherError(format!(
                                    "action generator {} is not an object, it is {}",
                                    name,
                                    type_of(&value)
                                )),
                            )?;
                            let generate =
                                object.get::<_, Value>("generate").js(&ctx)?;
                            let function = generate.as_function().ok_or(
                                SpecificationError::OtherError(format!(
                                    "action {} is not a function, it is {}",
                                    name,
                                    type_of(&value)
                                )),
                            )?;
                            action_generators.insert(
                                name.clone(),
                                ActionGenerator {
                                    name,
                                    this: Persistent::save(&ctx, value.clone()),
                                    function: Persistent::save(
                                        &ctx,
                                        function.clone(),
                                    ),
                                },
                            );
                        } else if symbol_description(&key).is_some_and(|description| {
                            IGNORED_SYMBOL_EXPORTS.contains(&description.as_str())
                        }) || IGNORED_STRING_EXPORTS.contains(&name.as_str()) {
                            continue;
                        } else {
                            return Err(SpecificationError::OtherError(format!(
                                "export {:?} is of unknown type ({}): {}",
                                name,
                                type_of(&value),
                                display_value(&value)
                            )));
                        }
                    }

                    if action_generators.is_empty() {
                        return Err(SpecificationError::OtherError(
                            "specification exports no action generators".to_string(),
                        ));
                    }

                    let mut extractors = Extractors::default();

                    let runtime =
                        bombadil_exports.runtime.clone().restore(&ctx).js(&ctx)?;
                    let extractors_value =
                        runtime.get::<_, Value>("extractors").js(&ctx)?;
                    let extractors_object = extractors_value.as_object().ok_or(
                        SpecificationError::OtherError(format!(
                            "extractors is not an object, it is {}",
                            type_of(&extractors_value)
                        )),
                    )?;
                    let extractors_array = extractors_object
                        .clone()
                        .into_array()
                        .ok_or(SpecificationError::OtherError(format!(
                            "extractors is not an array, it is {}",
                            type_of(&extractors_value)
                        )))?;
                    for i in 0..extractors_array.len() {
                        let extractor =
                            extractors_array.get::<Value>(i).js(&ctx)?;
                        let extractor = extractor.as_object().ok_or(
                            SpecificationError::OtherError(
                                "extractor is not an object".to_string(),
                            ),
                        )?;
                        extractors.register(Persistent::save(
                            &ctx,
                            extractor.clone(),
                        ));
                    }

                    Ok((
                        properties,
                        action_generators,
                        extractors,
                        bombadil_exports,
                    ))
                },
            )?;

        Ok(Verifier {
            properties,
            action_generators,
            extractors,
            bombadil_exports,
            engine,
        })
    }

    pub fn properties(&self) -> Vec<String> {
        self.properties.keys().cloned().collect()
    }

    #[hotpath::measure]
    pub fn step<A: FromGeneratedAction>(
        &mut self,
        snapshots: &[Snapshot],
        time: bombadil_schema::Time,
    ) -> Result<StepResult<'_, A>> {
        let Verifier {
            properties,
            action_generators,
            extractors,
            bombadil_exports,
            engine,
        } = self;
        let (action_tree, all_definite) =
            engine.context.with(|ctx| -> Result<(Tree<A>, bool)> {
                extractors.update_from_snapshots(snapshots, &ctx)?;
                let mut generator_branches: Vec<(u16, Tree<A>)> = Vec::new();

                let mut evaluate_thunk = |function: &RuntimeFunction,
                                          negated: bool|
                 -> Result<(
                    Formula<BombadilDomain<RuntimeFunction>>,
                    UniqueSnapshots,
                )> {
                    let (indices, value) = with_snapshot_tracking(
                        &ctx,
                        bombadil_exports,
                        |ctx| {
                            let function = function
                                .function
                                .clone()
                                .restore(ctx)
                                .js(ctx)?;
                            function.call::<_, Value>(()).js(ctx)
                        },
                    )?;
                    let accessed_snapshots: UniqueSnapshots = indices
                        .iter()
                        .filter_map(|&index| {
                            snapshots.get(index).cloned().map(|snapshot| {
                                ((snapshot.index, snapshot.time), snapshot)
                            })
                        })
                        .collect();
                    let syntax =
                        syntax_from_value(&value, bombadil_exports, &ctx)?;
                    Ok((
                        (if negated {
                            Syntax::Not(Box::new(syntax))
                        } else {
                            syntax
                        })
                        .nnf(),
                        accessed_snapshots,
                    ))
                };
                let mut evaluator = Evaluator::new(&mut evaluate_thunk);

                for property in properties.values_mut() {
                    let value = match &property.state {
                        PropertyState::Initial(formula) => {
                            evaluator.evaluate(formula, time)?
                        }
                        PropertyState::Evaluated(
                            eval::Value::Residual(residual)
                            | eval::Value::False(_, Some(residual)),
                        ) => evaluator.step(residual, time)?,
                        PropertyState::Evaluated(_)
                        | PropertyState::Settled => {
                            property.state = PropertyState::Settled;
                            continue;
                        }
                    };
                    property.state = PropertyState::Evaluated(match value {
                        eval::Value::True(_) => {
                            eval::Value::True(UniqueSnapshots::default())
                        }
                        value => value,
                    });
                }

                for action_generator in action_generators.values() {
                    // All exported generators are weighted equally.
                    generator_branches
                        .push((1, action_generator.generate(&ctx)?));
                }

                let action_tree = Tree::Branch {
                    branches: generator_branches,
                };

                let all_definite = properties.values().all(|p| {
                    matches!(
                        &p.state,
                        PropertyState::Settled
                            | PropertyState::Evaluated(
                                eval::Value::True(_)
                                    | eval::Value::False(_, None)
                            )
                    )
                });

                Ok((action_tree, all_definite))
            })?;

        Ok(StepResult {
            properties,
            actions: action_tree,
            all_definite,
        })
    }
}

fn is_instance<'js>(
    instance_of: &Function<'js>,
    value: &Value<'js>,
    class: &Value<'js>,
    ctx: &Ctx<'js>,
) -> Result<bool> {
    instance_of
        .call::<_, bool>((value.clone(), class.clone()))
        .js(ctx)
}

fn symbol_description(key: &Value<'_>) -> Option<String> {
    let symbol = key.get::<Symbol>().ok()?;
    symbol.description().ok()?.as_string()?.to_string().ok()
}

fn key_name(key: &Value<'_>) -> String {
    match key.as_string().and_then(|text| text.to_string().ok()) {
        Some(name) => name,
        None => {
            format!("Symbol({})", symbol_description(key).unwrap_or_default())
        }
    }
}

const IGNORED_SYMBOL_EXPORTS: &[&str] = &["Symbol.toStringTag"];
const IGNORED_STRING_EXPORTS: &[&str] = &["__esModule"];

#[derive(Debug, Clone)]
pub struct Property {
    pub name: String,
    state: PropertyState,
}

#[derive(Debug, Clone)]
enum PropertyState {
    Initial(Formula<BombadilDomain<RuntimeFunction>>),
    // Keep the current result here so callers can borrow it without cloning.
    // True and False without a continuation become Settled on the next step,
    // after callers have had a chance to report them.
    Evaluated(eval::Value<BombadilDomain<RuntimeFunction>>),
    // A terminal result from an earlier step; no longer emitted to callers.
    Settled,
}

#[derive(Debug, Clone)]
pub struct ActionGenerator {
    pub name: String,
    this: Persistent<Value<'static>>,
    function: Persistent<Function<'static>>,
}

impl ActionGenerator {
    fn generate<'js, A: FromGeneratedAction>(
        &self,
        ctx: &Ctx<'js>,
    ) -> Result<Tree<A>> {
        let this = self.this.clone().restore(ctx).js(ctx)?;
        let function = self.function.clone().restore(ctx).js(ctx)?;
        let value = function.call::<_, Value>((This(this),)).js(ctx)?;
        let actions_json: json::Value = to_json(ctx, value).js(ctx)?.ok_or(
            SpecificationError::OtherError(format!(
                "action generator {} returned undefined",
                self.name
            )),
        )?;
        let value_tree: Tree<json::Value> = json::from_value(actions_json)
            .map_err(|error| {
                SpecificationError::OtherError(format!(
                    "action generator `{}` returned an invalid action tree: {}",
                    self.name, error,
                ))
            })?;
        value_tree.try_map(&mut |action_value| {
            A::from_generated(action_value).map_err(|error| {
                SpecificationError::OtherError(format!(
                    "failed to convert generated action from `{}`: {}",
                    self.name, error,
                ))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use bombadil_schema::Time;
    use tempfile::NamedTempFile;

    use bombadil_ltl::stop::{StopDefault, stop_default};

    use super::*;

    // These tests only exercise property evaluation, never actions, so a
    // pass-through conversion is enough to satisfy the `step` bound.
    impl FromGeneratedAction for Snapshot {
        fn from_generated(value: json::Value) -> anyhow::Result<Self> {
            Ok(json::from_value(value)?)
        }
    }

    fn time_from_millis(millis: u64) -> Time {
        Time::from_system_time(
            std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_millis(millis),
        )
    }

    fn verifier(specification: &str) -> Verifier {
        use crate::specification::bundler::bundle;

        let mut specification_file = NamedTempFile::with_suffix(".ts").unwrap();
        specification_file
            .write_all(specification.as_bytes())
            .unwrap();

        let bundle_code =
            bundle(".", &specification_file.path().display().to_string())
                .unwrap();

        Verifier::new(&bundle_code).unwrap()
    }

    #[test]
    fn test_property_names() {
        let verifier = verifier(
            r#"
            import { always, actions, extract } from "@antithesishq/bombadil";
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
    fn test_default_properties_reexported() {
        let verifier = verifier(
            r#"
            import { actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);
            export * from "@antithesishq/bombadil/browser/defaults/properties";
            "#,
        );
        let mut properties = verifier.properties();
        properties.sort();
        assert_eq!(
            properties,
            vec![
                "noConsoleErrors",
                "noHttpErrorCodes",
                "noUncaughtExceptions",
                "noUnhandledPromiseRejections",
            ]
        );
    }

    #[test]
    fn test_property_evaluation_not() {
        let mut verifier = verifier(
            r#"
            import { now,  actions, extract  } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = now(() => foo.current).not();
            "#,
        );

        let time = time_from_millis(0);

        let result: StepResult<'_, Snapshot> = verifier
            .step(
                &[Snapshot {
                    index: 0,
                    name: None,
                    value: json::json!(false),
                    time,
                }],
                time,
            )
            .unwrap();

        let (name, value) = result.properties().next().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(matches!(value, eval::Value::True(_)));
    }

    #[test]
    fn test_property_evaluation_and() {
        let mut verifier = verifier(
            r#"
            import { now,  actions, extract  } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);
            const bar = extract((state) => state.bar);

            export const my_prop = now(() => foo.current).and(() => bar.current);
            "#,
        );

        let time = time_from_millis(0);

        let result: StepResult<Snapshot> = verifier
            .step(
                &[
                    Snapshot {
                        index: 0,
                        name: None,
                        value: json::json!(true),
                        time,
                    },
                    Snapshot {
                        index: 1,
                        name: None,
                        value: json::json!(true),
                        time,
                    },
                ],
                time,
            )
            .unwrap();

        let (name, value) = result.properties().next().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(matches!(value, eval::Value::True(_)));
    }

    #[test]
    fn test_property_evaluation_or() {
        let mut verifier = verifier(
            r#"
            import { now, actions, extract  } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);
            const bar = extract((state) => state.bar);

            export const my_prop = now(() => foo.current).or(() => bar.current);
            "#,
        );

        let time = time_from_millis(0);

        let result: StepResult<Snapshot> = verifier
            .step(
                &[
                    Snapshot {
                        index: 0,
                        name: None,
                        value: json::json!(false),
                        time,
                    },
                    Snapshot {
                        index: 1,
                        name: None,
                        value: json::json!(true),
                        time,
                    },
                ],
                time,
            )
            .unwrap();

        let (name, value) = result.properties().next().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(matches!(value, eval::Value::True(_)));
    }

    #[test]
    fn test_property_evaluation_implies() {
        let mut verifier = verifier(
            r#"
            import { now, actions, extract } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);
            const bar = extract((state) => state.bar);

            export const my_prop = now(() => foo.current).implies(() => bar.current);
            "#,
        );

        let time = time_from_millis(0);

        let result: StepResult<Snapshot> = verifier
            .step(
                &[
                    Snapshot {
                        index: 0,
                        name: None,
                        value: json::json!(false),
                        time,
                    },
                    Snapshot {
                        index: 1,
                        name: None,
                        value: json::json!(false),
                        time,
                    },
                ],
                time,
            )
            .unwrap();

        let (name, value) = result.properties().next().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(matches!(value, eval::Value::True(_)));
    }

    #[test]
    fn test_property_evaluation_next() {
        let mut verifier = verifier(
            r#"
            import { next, actions, extract  } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = next(() => foo.current === 1);
            "#,
        );

        for i in 0..=1 {
            let time = time_from_millis(i);
            let result: StepResult<Snapshot> = verifier
                .step(
                    &[Snapshot {
                        index: 0,
                        name: None,
                        value: json::json!(i),
                        time,
                    }],
                    time,
                )
                .unwrap();

            let (name, value) = result.properties().next().unwrap();
            assert_eq!(*name, "my_prop");

            if i == 1 {
                assert!(matches!(value, eval::Value::True(_)));
            } else {
                match value {
                    eval::Value::Residual(residual) => {
                        match stop_default(residual, time) {
                            Some(StopDefault::True(_)) => {}
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
            import { always, actions, extract } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = always(() => foo.current < 100);
            "#,
        );

        for i in 0..=100 {
            let time = time_from_millis(0);
            let result: StepResult<Snapshot> = verifier
                .step(
                    &[Snapshot {
                        index: 0,
                        name: None,
                        value: json::json!(i),
                        time,
                    }],
                    time,
                )
                .unwrap();

            let (name, value) = result.properties().next().unwrap();
            assert_eq!(*name, "my_prop");

            if i == 100 {
                assert!(matches!(
                    value,
                    eval::Value::False(
                        bombadil_ltl::violation::Violation::Always { .. },
                        _
                    )
                ))
            } else {
                match value {
                    eval::Value::Residual(residual) => {
                        match stop_default(residual, time) {
                            Some(StopDefault::True(_)) => {}
                            _ => panic!("should have a true stop default"),
                        }
                    }
                    _ => panic!("should be residual"),
                }
            }
        }

        // After the violation at i=100, the property should reset to
        // Residual when given a passing value.
        let time = time_from_millis(0);
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    index: 0,
                    name: None,
                    value: json::json!(0),
                    time,
                }],
                time,
            )
            .unwrap();
        let (name, value) = result.properties().next().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(
            matches!(value, eval::Value::Residual(_)),
            "expected Residual after reset, got: {:?}",
            value,
        );
    }

    #[test]
    fn test_property_evaluation_always_bounded() {
        let mut verifier = verifier(
            r#"
            import { always, actions, extract } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = always(() => foo.current < 4).within(3, "milliseconds");
            "#,
        );

        for i in 0..10 {
            let time = time_from_millis(i);
            let result: StepResult<Snapshot> = verifier
                .step(
                    &[Snapshot {
                        index: 0,
                        name: None,
                        value: json::json!(i),
                        time,
                    }],
                    time,
                )
                .unwrap();

            if let Some((name, value)) = result.properties().next() {
                assert_eq!(*name, "my_prop");

                if i < 4 {
                    match value {
                        eval::Value::Residual(residual) => {
                            match stop_default(residual, time) {
                                Some(StopDefault::True(_)) => {}
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
                    assert!(matches!(value, eval::Value::True(_)));
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
            import { eventually, actions, extract  } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = eventually(() => foo.current === 9);
            "#,
        );

        for i in 0..10 {
            let time = time_from_millis(i);
            let result: StepResult<Snapshot> = verifier
                .step(
                    &[Snapshot {
                        index: 0,
                        name: None,
                        value: json::json!(i),
                        time,
                    }],
                    time,
                )
                .unwrap();

            let (name, value) = result.properties().next().unwrap();
            assert_eq!(*name, "my_prop");

            if i == 9 {
                assert!(matches!(value, eval::Value::True(_)));
            } else {
                match value {
                    eval::Value::Residual(residual) => {
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
            import { eventually, actions, extract  } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = eventually(() => foo.current === 9).within(3, "milliseconds");
            "#,
        );

        for i in 0..10 {
            let time = time_from_millis(i);
            let result: StepResult<Snapshot> = verifier
                .step(
                    &[Snapshot {
                        index: 0,
                        name: None,
                        value: json::json!(i),
                        time,
                    }],
                    time,
                )
                .unwrap();

            if let Some((name, value)) = result.properties().next() {
                assert_eq!(*name, "my_prop");

                if i < 4 {
                    match value {
                        eval::Value::Residual(residual) => {
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
                    assert!(matches!(value, eval::Value::False(_, _)));
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
            import { always, actions, extract } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = always(() => foo.current < 5);
            "#,
        );

        // Steps 0-4: Residual (passing)
        for i in 0..5 {
            let result: StepResult<Snapshot> = verifier
                .step(
                    &[Snapshot {
                        index: 0,
                        name: None,
                        value: json::json!(i),
                        time: time_from_millis(0),
                    }],
                    time_from_millis(0),
                )
                .unwrap();
            let (_, value) = result.properties().next().unwrap();
            assert!(
                matches!(value, eval::Value::Residual(_)),
                "expected Residual at i={}, got: {:?}",
                i,
                value,
            );
        }

        // Step with value 5: False (violation)
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    index: 0,
                    name: None,
                    value: json::json!(5),
                    time: time_from_millis(0),
                }],
                time_from_millis(0),
            )
            .unwrap();
        let (_, value) = result.properties().next().unwrap();
        assert!(
            matches!(value, eval::Value::False(_, _)),
            "expected False at value=5, got: {:?}",
            value,
        );

        // Step with value 0: should reset to Residual (not repeat False)
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    index: 0,
                    name: None,
                    value: json::json!(0),
                    time: time_from_millis(0),
                }],
                time_from_millis(0),
            )
            .unwrap();
        let (_, value) = result.properties().next().unwrap();
        assert!(
            matches!(value, eval::Value::Residual(_)),
            "expected Residual after reset, got: {:?}",
            value,
        );

        // Step with value 5 again: should produce a new False
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    index: 0,
                    name: None,
                    value: json::json!(5),
                    time: time_from_millis(0),
                }],
                time_from_millis(0),
            )
            .unwrap();
        let (_, value) = result.properties().next().unwrap();
        assert!(
            matches!(value, eval::Value::False(_, _)),
            "expected new False at value=5, got: {:?}",
            value,
        );
    }

    #[test]
    fn test_now_false_is_terminal() {
        let mut verifier = verifier(
            r#"
            import { now,  extract, actions  } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = now(() => foo.current);
            "#,
        );

        let time = time_from_millis(0);

        // First step: False (no continuation)
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    index: 0,
                    name: None,
                    value: json::json!(false),
                    time,
                }],
                time,
            )
            .unwrap();
        let (name, value) = result.properties().next().unwrap();
        assert_eq!(*name, "my_prop");
        assert!(
            matches!(value, eval::Value::False(_, None)),
            "expected terminal False, got: {:?}",
            value,
        );

        // Subsequent step: property is settled, no result emitted
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    index: 0,
                    name: None,
                    value: json::json!(true),
                    time,
                }],
                time,
            )
            .unwrap();
        assert!(
            result.properties().next().is_none(),
            "expected no properties after terminal False, got: {:?}",
            result.properties,
        );
        assert!(result.all_definite);
    }

    #[test]
    fn test_always_bounded_continues_after_violation() {
        let mut verifier = verifier(
            r#"
            import { always, actions, extract } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);

            const foo = extract((state) => state.foo);

            export const my_prop = always(() => foo.current < 10).within(5, "milliseconds");
            "#,
        );

        // At time 0, value 0: Residual
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    index: 0,
                    name: None,
                    value: json::json!(0),
                    time: time_from_millis(0),
                }],
                time_from_millis(0),
            )
            .unwrap();
        let (_, value) = result.properties().next().unwrap();
        assert!(matches!(value, eval::Value::Residual(_)));

        // At time 3ms, value 10: fails the always, but has continuation
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    index: 0,
                    name: None,
                    value: json::json!(10),
                    time: time_from_millis(3),
                }],
                time_from_millis(3),
            )
            .unwrap();
        let (_, value) = result.properties().next().unwrap();
        assert!(
            matches!(value, eval::Value::False(_, _)),
            "expected False at time 3ms, got: {:?}",
            value,
        );
        assert!(!result.all_definite);

        // At time 4ms, value 0: should be Residual (reset via continuation)
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    index: 0,
                    name: None,
                    value: json::json!(0),
                    time: time_from_millis(4),
                }],
                time_from_millis(4),
            )
            .unwrap();
        let (_, value) = result.properties().next().unwrap();
        assert!(
            matches!(value, eval::Value::Residual(_)),
            "expected Residual at time 4ms after reset, got: {:?}",
            value,
        );

        // At time 6ms (past the 5ms bound): should resolve to True
        let result: StepResult<Snapshot> = verifier
            .step(
                &[Snapshot {
                    index: 0,
                    name: None,
                    value: json::json!(0),
                    time: time_from_millis(6),
                }],
                time_from_millis(6),
            )
            .unwrap();
        let (_, value) = result.properties().next().unwrap();
        assert!(
            matches!(value, eval::Value::True(_)),
            "expected True past the bound, got: {:?}",
            value,
        );
        assert!(result.all_definite);
    }

    #[test]
    fn test_action_generator_transformations() {
        let mut verifier = verifier(
            r#"
            import { actions, weighted } from "@antithesishq/bombadil";

            const numbers = weighted([
              [3, actions(() => [1, 2, 3])],
              [1, actions(() => [4])],
            ]);

            export const transformed = numbers
              .filter((n) => n !== 2)
              .map((n) => n * 10)
              .flatMap((n) => (n === 40 ? [n, n + 1] : [n]));
            "#,
        );

        let time = time_from_millis(0);
        let result: StepResult<json::Value> = verifier.step(&[], time).unwrap();

        let leaf = |n: i64| Tree::Leaf {
            value: json::json!(n),
        };
        let singleton = |n: i64| Tree::Branch {
            branches: vec![(1, leaf(n))],
        };
        assert_eq!(
            result.actions.prune(),
            Some(Tree::Branch {
                branches: vec![(
                    1,
                    Tree::Branch {
                        branches: vec![
                            (
                                3,
                                Tree::Branch {
                                    branches: vec![
                                        (1, singleton(10)),
                                        (1, singleton(30)),
                                    ],
                                },
                            ),
                            (
                                1,
                                Tree::Branch {
                                    branches: vec![(
                                        1,
                                        Tree::Branch {
                                            branches: vec![
                                                (1, leaf(40)),
                                                (1, leaf(41)),
                                            ],
                                        },
                                    )],
                                },
                            ),
                        ],
                    },
                )],
            })
        );
    }

    #[test]
    fn test_action_generator_fractional_weight() {
        let mut verifier = verifier(
            r#"
            import { actions } from "@antithesishq/bombadil";

            const nested = { branches: [[0.5, { value: 2 }]] };
            const tree = { branches: [[1, { value: 1 }], [2, nested]] };

            export const fractional = actions(() => tree);
            "#,
        );

        let result: Result<StepResult<json::Value>> =
            verifier.step(&[], time_from_millis(0));
        assert_eq!(
            result.err().map(|error| error.to_string()).as_deref(),
            Some(
                "action generator `fractional` returned an invalid action \
                 tree: invalid weight 0.5, expected an integer between 0 and \
                 65535 inclusive"
            ),
        );
    }

    #[test]
    fn test_action_generator_when() {
        let mut verifier = verifier(
            r#"
            import { actions } from "@antithesishq/bombadil";

            export const enabled = actions(() => [1]).when(true);

            export const disabled = actions(() => {
              throw new Error("should not run when disabled");
            }).when(() => false);
            "#,
        );

        let time = time_from_millis(0);
        let result: StepResult<json::Value> = verifier.step(&[], time).unwrap();

        assert_eq!(
            result.actions.prune(),
            Some(Tree::Branch {
                branches: vec![(
                    1,
                    Tree::Branch {
                        branches: vec![(
                            1,
                            Tree::Leaf {
                                value: json::json!(1)
                            }
                        )],
                    },
                )],
            })
        );
    }

    #[test]
    fn test_action_generator_when_cell() {
        let mut verifier = verifier(
            r#"
            import { actions, extract } from "@antithesishq/bombadil";

            const enabled = extract((state) => state.enabled);

            export const gated = actions(() => [1]).when(enabled);
            "#,
        );

        let mut step = |enabled: bool, millis: u64| {
            let time = time_from_millis(millis);
            let result: StepResult<json::Value> = verifier
                .step(
                    &[Snapshot {
                        index: 0,
                        name: None,
                        value: json::json!(enabled),
                        time,
                    }],
                    time,
                )
                .unwrap();
            result.actions.prune()
        };

        assert!(step(true, 0).is_some());
        assert!(step(false, 1).is_none());
        assert!(step(true, 2).is_some());
    }

    #[test]
    fn test_is_action() {
        let mut verifier = verifier(
            r#"
            import { actions } from "@antithesishq/bombadil";
            import { isAction } from "@antithesishq/bombadil/actions";

            export const clicks = actions(() => [
              "Back",
              { Click: { x: 1 } },
              { TypeText: "a" },
            ]).filter((action) => isAction(action, "Click"));
            "#,
        );

        let time = time_from_millis(0);
        let result: StepResult<json::Value> = verifier.step(&[], time).unwrap();

        assert_eq!(
            result.actions.prune(),
            Some(Tree::Branch {
                branches: vec![(
                    1,
                    Tree::Branch {
                        branches: vec![(
                            1,
                            Tree::Leaf {
                                value: json::json!({ "Click": { "x": 1 } })
                            }
                        )],
                    },
                )],
            })
        );
    }

    #[test]
    fn test_charset_is() {
        let mut verifier = verifier(
            r#"
            import { actions } from "@antithesishq/bombadil";
            import { CharSet } from "@antithesishq/bombadil/actions";

            export const texts = actions(() => [
              {
                CharSet: CharSet.union(
                  CharSet.fromLiterals("\r", "\t"),
                  CharSet.fromRange(0x20, 0x7e),
                ),
              },
              { Regexp: "a" },
              "Email",
            ])
              .filter((text) => CharSet.is(text))
              .map((text) => ({
                CharSet: text.CharSet.filter((entry) =>
                  CharSet.Entry.is(entry, "Literal"),
                ),
              }));
            "#,
        );

        let time = time_from_millis(0);
        let result: StepResult<json::Value> = verifier.step(&[], time).unwrap();

        assert_eq!(
            result.actions.prune(),
            Some(Tree::Branch {
                branches: vec![(
                    1,
                    Tree::Branch {
                        branches: vec![(
                            1,
                            Tree::Leaf {
                                value: json::json!({ "CharSet": [
                                    { "Literal": "\r" },
                                    { "Literal": "\t" },
                                ] })
                            }
                        )],
                    },
                )],
            })
        );
    }

    fn step_error(specification: &str) -> String {
        let mut verifier = verifier(specification);
        match verifier.step::<json::Value>(&[], time_from_millis(0)) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("expected step to fail"),
        }
    }

    fn constructor_error(specification: &str) -> String {
        use crate::specification::bundler::bundle;

        let mut specification_file = NamedTempFile::with_suffix(".ts").unwrap();
        specification_file
            .write_all(specification.as_bytes())
            .unwrap();
        let bundle_code =
            bundle(".", &specification_file.path().display().to_string())
                .unwrap();
        match Verifier::new(&bundle_code) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("expected verifier construction to fail"),
        }
    }

    fn generated_values(specification: &str) -> Vec<json::Value> {
        let mut verifier = verifier(specification);
        let result: StepResult<json::Value> =
            verifier.step(&[], time_from_millis(0)).unwrap();
        result.actions.values().into_iter().cloned().collect()
    }

    #[test]
    fn test_action_generator_replaces_lone_surrogates() {
        let values = generated_values(
            r#"
            import { actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => [
              "😀".slice(0, 1),
              "😀",
              { [ "😀".slice(0, 1) ]: "😀".slice(0, 1) },
            ]);
            "#,
        );
        assert_eq!(
            values,
            vec![
                json::json!("\u{FFFD}"),
                json::json!("😀"),
                json::json!({ "\u{FFFD}": "\u{FFFD}" }),
            ]
        );
    }

    #[test]
    fn test_action_generator_floats_are_bit_exact() {
        let expected: [f64; 6] = [
            7.231233596801759e-16,
            7.073538409042359e-13,
            366.01340770721436,
            0.1 + 0.2,
            5e-324,
            1.7976931348623157e308,
        ];
        let values = generated_values(
            r#"
            import { actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => [
              7.231233596801759e-16,
              7.073538409042359e-13,
              366.01340770721436,
              0.1 + 0.2,
              5e-324,
              1.7976931348623157e308,
            ]);
            "#,
        );
        assert_eq!(values.len(), expected.len());
        for (value, expected) in values.iter().zip(expected) {
            let actual = value.as_f64().unwrap();
            assert_eq!(actual.to_bits(), expected.to_bits(), "{actual:e}");
        }
    }

    fn json_depth(value: &json::Value) -> usize {
        let mut deepest = 0;
        let mut stack = vec![(value, 1)];
        while let Some((value, depth)) = stack.pop() {
            deepest = deepest.max(depth);
            match value {
                json::Value::Array(items) => {
                    stack.extend(items.iter().map(|item| (item, depth + 1)));
                }
                json::Value::Object(entries) => {
                    stack.extend(
                        entries.values().map(|entry| (entry, depth + 1)),
                    );
                }
                _ => {}
            }
        }
        deepest
    }

    fn on_small_stack(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(test)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn test_action_generator_accepts_values_nested_1000_deep() {
        on_small_stack(|| {
            let values = generated_values(
                r#"
                import { actions } from "@antithesishq/bombadil";
                export const _actions = actions(() => {
                  let value = 1;
                  for (let i = 0; i < 1000; i++) {
                    value = [value];
                  }
                  return [value];
                });
                "#,
            );
            assert_eq!(values.len(), 1);
            assert_eq!(json_depth(&values[0]), 1001);
        });
    }

    #[test]
    fn test_action_generator_accepts_values_near_max_json_depth() {
        on_small_stack(|| {
            let values = generated_values(&format!(
                r#"
                import {{ actions }} from "@antithesishq/bombadil";
                export const _actions = actions(() => {{
                  let value = 1;
                  for (let i = 0; i < {}; i++) {{
                    value = [value];
                  }}
                  return [value];
                }});
                "#,
                crate::specification::engine::MAX_JSON_DEPTH - 16
            ));
            assert_eq!(
                json_depth(&values[0]),
                crate::specification::engine::MAX_JSON_DEPTH - 15
            );
        });
    }

    #[test]
    fn test_action_generator_rejects_values_beyond_max_json_depth() {
        on_small_stack(|| {
            let error = step_error(
                r#"
                import { actions } from "@antithesishq/bombadil";
                export const _actions = actions(() => {
                  let value = 1;
                  for (let i = 0; i < 100000; i++) {
                    value = [value];
                  }
                  return [value];
                });
                "#,
            );
            assert!(
                error.contains("Object reference chain is too long"),
                "{error}"
            );
        });
    }

    #[test]
    fn test_action_generator_integer_and_float_representations() {
        on_small_stack(|| {
            let values = generated_values(
                r#"
                import { actions } from "@antithesishq/bombadil";
                export const _actions = actions(() => [
                  0, 3, -4, 2 ** 53, 2 ** 64, 1.5, 1e21,
                ]);
                "#,
            );
            assert_eq!(values.len(), 7);
            assert!(values[0].is_u64() && values[0].as_u64() == Some(0));
            assert!(values[1].is_u64() && values[1].as_u64() == Some(3));
            assert!(values[2].is_i64() && values[2].as_i64() == Some(-4));
            assert!(
                values[3].is_u64()
                    && values[3].as_u64() == Some(9_007_199_254_740_992)
            );
            assert!(values[4].is_f64());
            assert_eq!(values[4].as_f64(), Some(18_446_744_073_709_551_616.0));
            assert!(values[5].is_f64() && values[5].as_f64() == Some(1.5));
            assert!(values[6].is_f64() && values[6].as_f64() == Some(1e21));
        });
    }

    #[test]
    fn test_action_generator_object_keys_and_values() {
        on_small_stack(|| {
            let values = generated_values(
                r#"
                import { actions } from "@antithesishq/bombadil";
                export const _actions = actions(() => [
                  { b: 1, a: 2, "1": 3 },
                ]);
                "#,
            );
            let entries = values[0].as_object().unwrap();
            let mut keys: Vec<&str> =
                entries.keys().map(String::as_str).collect();
            keys.sort();
            assert_eq!(keys, vec!["1", "a", "b"]);
            assert_eq!(entries["1"], json::json!(3));
            assert_eq!(entries["a"], json::json!(2));
            assert_eq!(entries["b"], json::json!(1));
        });
    }

    #[test]
    fn test_dropping_deeply_nested_json_value_does_not_overflow() {
        on_small_stack(|| {
            let mut value = json::Value::Null;
            for _ in 0..5000 {
                value = json::Value::Array(vec![value]);
            }
            assert_eq!(json_depth(&value), 5001);
            drop(value);
        });
    }

    #[test]
    fn test_thunk_error_reports_error_name_and_message() {
        let error = step_error(
            r#"
            import { always, actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);
            export const boom = always(() => { throw new TypeError("boom"); });
            "#,
        );
        assert!(error.starts_with("TypeError: boom"), "{error}");
    }

    #[test]
    fn test_thunk_error_without_message_reports_name() {
        let error = step_error(
            r#"
            import { always, actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);
            export const boom = always(() => { throw new Error(); });
            "#,
        );
        assert!(error.starts_with("Error"), "{error}");
    }

    #[test]
    fn test_unknown_export_number_is_described_by_typeof() {
        let error = constructor_error(
            r#"
            import { actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);
            export const five = 5;
            "#,
        );
        assert!(error.contains("(number): 5"), "{error}");
    }

    #[test]
    fn test_unknown_export_object_is_printed_as_json() {
        let error = constructor_error(
            r#"
            import { actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);
            export const object = { a: 1 };
            "#,
        );
        assert!(error.contains("\"a\": 1"), "{error}");
    }

    #[test]
    fn test_unknown_export_symbol_is_printed_with_description() {
        let error = constructor_error(
            r#"
            import { actions } from "@antithesishq/bombadil";
            export const _actions = actions(() => []);
            export const symbol = Symbol("x");
            "#,
        );
        assert!(error.contains("(symbol): Symbol(x)"), "{error}");
    }
}
