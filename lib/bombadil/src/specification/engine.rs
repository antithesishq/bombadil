use std::collections::HashSet;

use rquickjs::{
    Array, CaughtError, Coerced, Context, Ctx, Exception, Function, Object,
    Runtime, Type, Value,
    function::{Rest, This},
};
use serde_json as json;

pub const MAX_STACK_SIZE: usize = 512 * 1024;
pub const MAX_JSON_DEPTH: usize = 1024;

pub struct Engine {
    pub context: Context,
    pub runtime: Runtime,
}

impl Engine {
    pub fn new() -> rquickjs::Result<Engine> {
        let runtime = Runtime::new()?;
        runtime.set_max_stack_size(MAX_STACK_SIZE);
        let context = Context::full(&runtime)?;
        context.with(|ctx| install_console(&ctx))?;
        Ok(Engine { context, runtime })
    }
}

fn install_console<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<()> {
    let console = Object::new(ctx.clone())?;
    console.set(
        "log",
        Function::new(ctx.clone(), |args: Rest<Value<'js>>| {
            log::info!("{}", format_console_args(&args.0));
        })?,
    )?;
    console.set(
        "warn",
        Function::new(ctx.clone(), |args: Rest<Value<'js>>| {
            log::warn!("{}", format_console_args(&args.0));
        })?,
    )?;
    console.set(
        "error",
        Function::new(ctx.clone(), |args: Rest<Value<'js>>| {
            log::error!("{}", format_console_args(&args.0));
        })?,
    )?;
    ctx.globals().set("console", console)?;
    Ok(())
}

fn js_string_to_rust<'js>(
    ctx: &Ctx<'js>,
    text: &rquickjs::String<'js>,
) -> rquickjs::Result<String> {
    match text.to_string() {
        Ok(text) => Ok(text),
        Err(_) => {
            let string_constructor: Object = ctx.globals().get("String")?;
            let prototype: Object = string_constructor.get("prototype")?;
            let to_well_formed: Function = prototype.get("toWellFormed")?;
            let well_formed: rquickjs::String =
                to_well_formed.call((This(text.clone()),))?;
            well_formed.to_string()
        }
    }
}

pub fn to_json<'js>(
    ctx: &Ctx<'js>,
    value: Value<'js>,
) -> rquickjs::Result<Option<json::Value>> {
    if value.is_undefined() {
        return Ok(None);
    }
    walk(ctx, value).map(Some)
}

enum Frame<'js> {
    Array {
        identity: Value<'js>,
        source: Array<'js>,
        length: usize,
        next: usize,
        items: Vec<json::Value>,
    },
    Object {
        identity: Value<'js>,
        source: Object<'js>,
        keys: std::vec::IntoIter<rquickjs::String<'js>>,
        name: Option<String>,
        entries: json::Map<String, json::Value>,
    },
}

impl<'js> Frame<'js> {
    fn open(value: &Value<'js>) -> rquickjs::Result<Option<Self>> {
        if let Some(source) = value.as_array() {
            // TODO: use Array::len once https://github.com/DelSkayn/rquickjs/pull/810 is released.
            let length = source.as_object().get::<_, f64>("length")? as usize;
            return Ok(Some(Frame::Array {
                identity: value.clone(),
                source: source.clone(),
                length,
                next: 0,
                items: Vec::new(),
            }));
        }
        if let Some(source) = value.as_object() {
            let keys = source
                .keys::<rquickjs::String>()
                .collect::<rquickjs::Result<Vec<_>>>()?;
            return Ok(Some(Frame::Object {
                identity: value.clone(),
                source: source.clone(),
                keys: keys.into_iter(),
                name: None,
                entries: json::Map::new(),
            }));
        }
        Ok(None)
    }

    fn identity(&self) -> &Value<'js> {
        match self {
            Frame::Array { identity, .. } | Frame::Object { identity, .. } => {
                identity
            }
        }
    }

    fn next_child(
        &mut self,
        ctx: &Ctx<'js>,
    ) -> rquickjs::Result<Option<Value<'js>>> {
        match self {
            Frame::Array {
                source,
                length,
                next,
                ..
            } => {
                if *next >= *length {
                    return Ok(None);
                }
                let child = source.get::<Value<'js>>(*next)?;
                *next += 1;
                Ok(Some(child))
            }
            Frame::Object {
                source, keys, name, ..
            } => {
                let Some(key) = keys.next() else {
                    return Ok(None);
                };
                let child = source.get::<_, Value<'js>>(key.clone())?;
                *name = Some(js_string_to_rust(ctx, &key)?);
                Ok(Some(child))
            }
        }
    }

    fn accept(&mut self, value: Option<json::Value>) {
        match self {
            Frame::Array { items, .. } => {
                items.push(value.unwrap_or(json::Value::Null));
            }
            Frame::Object { name, entries, .. } => {
                if let (Some(name), Some(value)) = (name.take(), value) {
                    entries.insert(name, value);
                }
            }
        }
    }

    fn finish(self) -> json::Value {
        match self {
            Frame::Array { items, .. } => json::Value::Array(items),
            Frame::Object { entries, .. } => json::Value::Object(entries),
        }
    }
}

fn walk<'js>(
    ctx: &Ctx<'js>,
    root: Value<'js>,
) -> rquickjs::Result<json::Value> {
    let mut on_path: HashSet<Value<'js>> = HashSet::new();
    let mut stack: Vec<Frame<'js>> = Vec::new();
    let mut value = root;
    loop {
        let mut finished: Option<Option<json::Value>> =
            match Frame::open(&value)? {
                Some(frame) => {
                    if stack.len() >= MAX_JSON_DEPTH {
                        return Err(Exception::throw_range(
                            ctx,
                            "Object reference chain is too long",
                        ));
                    }
                    if !on_path.insert(value.clone()) {
                        return Err(Exception::throw_type(
                            ctx,
                            "cyclic object value",
                        ));
                    }
                    stack.push(frame);
                    None
                }
                None => Some(scalar_to_json(ctx, &value)?),
            };
        loop {
            let Some(frame) = stack.last_mut() else {
                return Ok(finished.flatten().unwrap_or(json::Value::Null));
            };
            if let Some(done) = finished.take() {
                frame.accept(done);
            }
            match frame.next_child(ctx)? {
                Some(child) => {
                    value = child;
                    break;
                }
                None => {
                    if let Some(closed) = stack.pop() {
                        on_path.remove(closed.identity());
                        finished = Some(Some(closed.finish()));
                    }
                }
            }
        }
    }
}

fn scalar_to_json<'js>(
    ctx: &Ctx<'js>,
    value: &Value<'js>,
) -> rquickjs::Result<Option<json::Value>> {
    if value.is_undefined() {
        return Ok(None);
    }
    if value.is_null() {
        return Ok(Some(json::Value::Null));
    }
    if let Some(flag) = value.as_bool() {
        return Ok(Some(json::Value::Bool(flag)));
    }
    if let Some(text) = value.as_string() {
        return Ok(Some(json::Value::String(js_string_to_rust(ctx, text)?)));
    }
    if value.is_symbol() {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert symbol to JSON",
        ));
    }
    if value.is_big_int() {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert bigint to JSON",
        ));
    }
    if let Some(int) = value.as_int() {
        return Ok(Some(match u64::try_from(int) {
            Ok(unsigned) => json::Value::from(unsigned),
            Err(_) => json::Value::from(i64::from(int)),
        }));
    }
    if let Some(float) = value.as_float() {
        return Ok(Some(float_to_json(float)));
    }
    Err(Exception::throw_type(ctx, "cannot convert value to JSON"))
}

fn float_to_json(float: f64) -> json::Value {
    if !float.is_finite() {
        return json::Value::Null;
    }
    if float == 0.0 {
        return json::Value::from(0_u64);
    }
    if float.fract() == 0.0 {
        if (0.0..18_446_744_073_709_551_616.0).contains(&float) {
            return json::Value::from(float as u64);
        }
        if (-9_223_372_036_854_775_808.0..0.0).contains(&float) {
            return json::Value::from(float as i64);
        }
    }
    json::Value::from(float)
}

pub fn type_of(value: &Value<'_>) -> &'static str {
    match value.type_of() {
        Type::Uninitialized | Type::Undefined => "undefined",
        Type::Bool => "boolean",
        Type::Int | Type::Float => "number",
        Type::String => "string",
        Type::Symbol => "symbol",
        Type::BigInt => "bigint",
        Type::Function | Type::Constructor => "function",
        Type::Null
        | Type::Array
        | Type::Object
        | Type::Promise
        | Type::Exception
        | Type::Proxy
        | Type::Module
        | Type::Unknown => "object",
    }
}

fn discard<T>(ctx: &Ctx<'_>, result: rquickjs::Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            if error.is_exception() {
                let _ = ctx.catch();
            }
            None
        }
    }
}

fn is_plain_data<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> bool {
    if value.is_array() {
        return true;
    }
    let Some(object) = value.as_object() else {
        return false;
    };
    if value.is_function() {
        return false;
    }
    match object.get_prototype() {
        None => true,
        Some(prototype) => {
            discard(
                ctx,
                ctx.globals().get::<_, Object>("Object").and_then(
                    |constructor| constructor.get::<_, Object>("prototype"),
                ),
            )
            .is_some_and(|object_prototype| object_prototype == prototype)
        }
    }
}

pub fn display_value(value: &Value<'_>) -> String {
    const UNPRINTABLE: &str = "<unprintable>";
    let ctx = value.ctx();

    if let Some(text) = value.as_string() {
        return discard(ctx, js_string_to_rust(ctx, text))
            .unwrap_or_else(|| UNPRINTABLE.to_string());
    }

    if let Some(symbol) = value.as_symbol() {
        return match discard(ctx, symbol.description()) {
            Some(description) if !description.is_undefined() => {
                let text = description
                    .as_string()
                    .and_then(|text| discard(ctx, js_string_to_rust(ctx, text)))
                    .unwrap_or_else(|| UNPRINTABLE.to_string());
                format!("Symbol({text})")
            }
            _ => "Symbol()".to_string(),
        };
    }

    if is_plain_data(ctx, value)
        && let Some(Some(json)) = discard(ctx, to_json(ctx, value.clone()))
        && let Ok(text) = json::to_string_pretty(&json)
    {
        return text;
    }

    match discard(ctx, value.get::<Coerced<String>>()) {
        Some(Coerced(text)) => text,
        None => UNPRINTABLE.to_string(),
    }
}

pub fn format_console_args(args: &[Value<'_>]) -> String {
    args.iter().map(display_value).collect::<Vec<_>>().join(" ")
}

pub fn caught_to_string(error: CaughtError<'_>) -> String {
    match error {
        CaughtError::Exception(exception) => {
            let stack = exception.stack();
            let thrown = exception.into_object().into_value();
            let mut text =
                discard(thrown.ctx(), thrown.get::<Coerced<String>>())
                    .map(|Coerced(text)| text)
                    .unwrap_or_else(|| "unknown exception".to_string());
            if let Some(stack) = stack.filter(|stack| !stack.is_empty()) {
                text.push('\n');
                text.push_str(&stack);
            }
            text
        }
        CaughtError::Value(value) => display_value(&value),
        CaughtError::Error(error) => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rquickjs::CatchResultExt;
    use serde_json::json;

    fn format_sources(ctx: &Ctx<'_>, sources: &[&str]) -> String {
        let args: Vec<Value> = sources
            .iter()
            .map(|source| ctx.eval::<Value, _>(*source).unwrap())
            .collect();
        format_console_args(&args)
    }

    #[test]
    fn format_console_args_prints_like_string_conversion() {
        let engine = Engine::new().unwrap();
        engine.context.with(|ctx| {
            assert_eq!(format_sources(&ctx, &["\"hello\""]), "hello");
            assert_eq!(
                format_sources(&ctx, &["\"count:\"", "42", "true"]),
                "count: 42 true"
            );
            assert_eq!(format_sources(&ctx, &["undefined"]), "undefined");
            assert_eq!(format_sources(&ctx, &["null"]), "null");
            assert_eq!(format_sources(&ctx, &[]), "");
        });
    }

    #[test]
    fn type_of_matches_js_typeof() {
        let engine = Engine::new().unwrap();
        engine.context.with(|ctx| {
            let cases = [
                ("undefined", "undefined"),
                ("null", "object"),
                ("true", "boolean"),
                ("1.5", "number"),
                ("1", "number"),
                ("\"s\"", "string"),
                ("Symbol('x')", "symbol"),
                ("1n", "bigint"),
                ("() => 1", "function"),
                ("(class A {})", "function"),
                ("(function f() {})", "function"),
                ("[]", "object"),
                ("({})", "object"),
                ("new Error('e')", "object"),
            ];
            for (source, expected) in cases {
                let value: Value = ctx.eval(source).unwrap();
                assert_eq!(type_of(&value), expected, "for {source}");
            }
        });
    }

    #[test]
    fn deep_nesting_overflows_and_engine_recovers() {
        let engine = Engine::new().unwrap();
        let source = "[".repeat(100_000) + &"]".repeat(100_000);
        engine.context.with(|ctx| {
            let error = match ctx.eval::<Value, _>(source).catch(&ctx) {
                Err(error) => caught_to_string(error),
                Ok(_) => panic!("expected deep nesting to fail"),
            };
            assert!(
                error.contains("Maximum call stack size exceeded"),
                "unexpected error: {error}"
            );
            let two: i32 = ctx.eval("1 + 1").unwrap();
            assert_eq!(two, 2);
        });
    }

    #[test]
    fn display_value_clears_exceptions_thrown_while_formatting() {
        let engine = Engine::new().unwrap();
        engine.context.with(|ctx| {
            let value: Value = ctx
                .eval("({ get x() { throw 1 }, toString() { throw 2 } })")
                .unwrap();
            assert_eq!(display_value(&value), "<unprintable>");
            assert!(!ctx.has_exception());
            assert_eq!(ctx.catch().type_of(), Type::Uninitialized);
        });
    }

    #[test]
    fn display_value_prints_non_plain_objects_as_strings() {
        let engine = Engine::new().unwrap();
        engine.context.with(|ctx| {
            let cases = [
                ("new Error('boom')", "Error: boom"),
                ("new TypeError('bad')", "TypeError: bad"),
                ("/re/g", "/re/g"),
                ("new Map()", "[object Map]"),
                (
                    "new (class A { toString() { return 'custom' } })()",
                    "custom",
                ),
                ("({ a: [1] })", "{\n  \"a\": [\n    1\n  ]\n}"),
                (
                    "Object.assign(Object.create(null), { a: 1 })",
                    "{\n  \"a\": 1\n}",
                ),
                ("[1, 'a']", "[\n  1,\n  \"a\"\n]"),
            ];
            for (source, expected) in cases {
                let value: Value = ctx.eval(source).unwrap();
                assert_eq!(display_value(&value), expected, "for {source}");
            }
        });
    }

    #[test]
    fn to_json_matches_chrome_return_by_value() {
        let cases: Vec<(&str, Option<json::Value>)> = vec![
            ("new Date(0)", Some(json!({}))),
            ("({ toJSON() { return 7 } })", Some(json!({"toJSON": {}}))),
            ("({ d: new Date(0), n: 1 })", Some(json!({"d": {}, "n": 1}))),
            ("-0", Some(json!(0))),
            ("[-0, NaN, Infinity]", Some(json!([0, null, null]))),
            ("({ a: -0 })", Some(json!({"a": 0}))),
            ("(() => 1)", Some(json!({}))),
            (
                "({ f: () => 1, u: undefined, b: 2 })",
                Some(json!({"f": {}, "b": 2})),
            ),
            ("[undefined, 1]", Some(json!([null, 1]))),
            ("({ [Symbol('k')]: 1, x: 2 })", Some(json!({"x": 2}))),
            (
                "Object.defineProperty({ x: 1 }, 'h', { value: 2, enumerable: false })",
                Some(json!({"x": 1})),
            ),
            ("({ get g() { return 5 } })", Some(json!({"g": 5}))),
            (
                "(() => { const a = [1]; a[3] = 4; return a; })()",
                Some(json!([1, null, null, 4])),
            ),
            ("new Map([[1,2]])", Some(json!({}))),
            ("new Uint8Array([1,2])", Some(json!({"0": 1, "1": 2}))),
            ("Object.assign([1,2], { extra: 3 })", Some(json!([1, 2]))),
            ("new String('s')", Some(json!({"0": "s"}))),
            ("new Number(3)", Some(json!({}))),
            (
                "(() => { const x = { v: 1 }; return [x, x]; })()",
                Some(json!([{"v": 1}, {"v": 1}])),
            ),
            (r"'a\uD83Db'", Some(json!("a\u{FFFD}b"))),
            (r"({ ['k\uD83D']: 1 })", Some(json!({"k\u{FFFD}": 1}))),
            ("undefined", None),
        ];
        let engine = Engine::new().unwrap();
        engine.context.with(|ctx| {
            for (source, expected) in cases {
                let value: Value = ctx.eval(source).unwrap();
                let actual = to_json(&ctx, value)
                    .unwrap_or_else(|error| panic!("{source}: {error}"));
                assert_eq!(actual, expected, "for {source}");
            }
            let negative_zero: Value = ctx.eval("-0").unwrap();
            let value = to_json(&ctx, negative_zero).unwrap().unwrap();
            assert_eq!(serde_json::to_string(&value).unwrap(), "0");
        });
    }

    #[test]
    fn to_json_rejects_values_chrome_cannot_return() {
        let sources = [
            "(() => { const a = []; a.length = 2 ** 31; a[0] = Symbol(); return a; })()",
            "(() => { const a = []; a.length = 2 ** 32 - 1; a[0] = 1n; return a; })()",
            "Symbol('x')",
            "({ s: Symbol('x') })",
            "[Symbol('x')]",
            "1n",
            "({ b: 1n })",
            "(() => { const o = {}; o.o = o; return o; })()",
            "({ get g() { throw new Error('boom') } })",
        ];
        let engine = Engine::new().unwrap();
        engine.context.with(|ctx| {
            for source in sources {
                let value: Value = ctx.eval(source).unwrap();
                assert!(to_json(&ctx, value).is_err(), "for {source}");
                let _ = ctx.catch();
                assert!(!ctx.has_exception(), "for {source}");
                let two: i32 = ctx.eval("1 + 1").unwrap();
                assert_eq!(two, 2);
            }
        });
    }

    fn nested_source(depth: usize, wrap: &str) -> String {
        format!(
            "(() => {{ let v = 1; for (let i = 0; i < {depth}; i++) v = {wrap}; return v; }})()"
        )
    }

    #[test]
    fn to_json_accepts_values_up_to_max_depth_on_a_small_stack() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let engine = Engine::new().unwrap();
                engine.context.with(|ctx| {
                    for wrap in ["[v]", "({ c: v })"] {
                        let value: Value = ctx
                            .eval(nested_source(MAX_JSON_DEPTH, wrap))
                            .unwrap();
                        let json = to_json(&ctx, value).unwrap().unwrap();
                        assert!(json.is_array() || json.is_object());
                        drop(json);
                    }
                });
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn to_json_rejects_values_deeper_than_max_depth() {
        let engine = Engine::new().unwrap();
        engine.context.with(|ctx| {
            for depth in [MAX_JSON_DEPTH + 1, 100_000] {
                for wrap in ["[v]", "({ c: v })"] {
                    let value: Value =
                        ctx.eval(nested_source(depth, wrap)).unwrap();
                    let error = match to_json(&ctx, value).catch(&ctx) {
                        Err(error) => caught_to_string(error),
                        Ok(_) => panic!("expected depth {depth} to fail"),
                    };
                    assert!(
                        error.contains("Object reference chain is too long"),
                        "{error}"
                    );
                    assert!(!ctx.has_exception());
                }
            }
        });
    }
}
