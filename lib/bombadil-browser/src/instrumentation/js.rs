use anyhow::anyhow;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use const_format::formatcp;
use oxc::allocator;
use oxc::ast::ast::{
    AssignmentOperator, AssignmentTarget, Expression, Statement,
};
use oxc::codegen::Codegen;
use oxc::semantic::SemanticBuilder;
use oxc::{
    allocator::{Allocator, CloneIn, TakeIn},
    ast::ast::{self},
    parser::Parser,
    span::{GetSpan, SPAN, SourceType, Span},
};
use oxc_traverse::{Ancestor, Traverse, TraverseCtx, traverse_mut};

use crate::instrumentation::{
    Block, BlockIdPrefix, Instrumented, source_id::SourceId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstrumentationError {
    ParseErrors(Vec<oxc::diagnostics::OxcDiagnostic>),
    SemanticErrors(Vec<oxc::diagnostics::OxcDiagnostic>),
}

impl From<InstrumentationError> for anyhow::Error {
    fn from(value: InstrumentationError) -> Self {
        anyhow!(value)
    }
}

impl fmt::Display for InstrumentationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstrumentationError::ParseErrors(errors) => {
                write!(f, "Parse errors: {:?}", errors)
            }
            InstrumentationError::SemanticErrors(errors) => {
                write!(f, "Semantic errors: {:?}", errors)
            }
        }
    }
}

pub type InstrumentationResult<T> = Result<T, InstrumentationError>;

pub const NAMESPACE: &str = "__bombadil__";

pub const EDGES_PREVIOUS: &str = "edges_previous";
pub const EDGES_CURRENT: &str = "edges_current";
pub const EDGE_MAP_SIZE: usize = 64 * 1024;

const LOCATION_PREVIOUS: &str = "previous";

const PRELUDE: &str = formatcp!(
    "window.{NAMESPACE} = window.{NAMESPACE} || {{
        {EDGES_PREVIOUS}: new Uint8Array({EDGE_MAP_SIZE}),
        {EDGES_CURRENT}: new Uint8Array({EDGE_MAP_SIZE}),
        {LOCATION_PREVIOUS}: 0,
    }};"
);

pub fn instrument_source_code(
    source_id: SourceId,
    source_text: &str,
    source_type: SourceType,
) -> InstrumentationResult<Instrumented> {
    let allocator = Allocator::default();
    let mut program = parse(&allocator, source_text, source_type)?;
    let blocks =
        instrument_program(&allocator, &mut program, source_id, source_text)?;

    let prelude = parse(&allocator, PRELUDE, source_type)?;
    program.body.splice(0..0, prelude.body);

    Ok(Instrumented {
        text: Codegen::new().build(&program).code,
        blocks,
    })
}

fn parse<'a>(
    allocator: &'a Allocator,
    source_text: &'a str,
    source_type: SourceType,
) -> InstrumentationResult<ast::Program<'a>> {
    let parser = Parser::new(allocator, source_text, source_type);
    let result = parser.parse();
    if result.panicked {
        return Err(InstrumentationError::ParseErrors(result.errors.to_vec()));
    }
    Ok(result.program)
}

fn instrument_program<'a>(
    allocator: &'a Allocator,
    program: &mut ast::Program<'a>,
    source_id: SourceId,
    source_text: &str,
) -> InstrumentationResult<Vec<Block>> {
    let semantic = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(program);

    if !semantic.errors.is_empty() {
        let errors = semantic.errors.to_vec();
        return Err(InstrumentationError::SemanticErrors(errors));
    }
    let scopes = semantic.semantic.into_scoping();
    let top = Function::new(&source_id, "<top>".into());
    let mut instrumenter = Instrumenter {
        source_id,
        line_starts: line_starts(source_text),
        function_indexes: HashMap::from([(top.path.clone(), 0)]),
        functions: vec![top],
        scopes: vec![0],
        blocks: Vec::new(),
    };
    traverse_mut(&mut instrumenter, allocator, program, scopes, ());

    Ok(instrumenter.blocks)
}

/// Byte offset of each line start. Lines end at LF, CR, CRLF, U+2028 or
/// U+2029, as in ECMAScript.
fn line_starts(source_text: &str) -> Vec<u32> {
    let bytes = source_text.as_bytes();
    let mut starts = vec![0];
    for (index, byte) in bytes.iter().enumerate() {
        let terminator_length = match byte {
            b'\n' => 1,
            b'\r' if bytes.get(index + 1) != Some(&b'\n') => 1,
            // U+2028 and U+2029 in UTF-8.
            0xE2 if matches!(
                bytes.get(index + 1..index + 3),
                Some([0x80, 0xA8 | 0xA9])
            ) =>
            {
                3
            }
            _ => continue,
        };
        starts.push((index + terminator_length) as u32);
    }
    starts
}

struct Instrumenter {
    source_id: SourceId,
    line_starts: Vec<u32>,
    /// One per function path, `<top>` first.
    functions: Vec<Function>,
    function_indexes: HashMap<Arc<str>, usize>,
    /// Index into `functions` for each enclosing scope, innermost last.
    /// Anonymous scopes repeat their parent's.
    scopes: Vec<usize>,
    blocks: Vec<Block>,
}

struct Function {
    path: Arc<str>,
    id_prefix: BlockIdPrefix,
    blocks_count: u32,
}

impl Function {
    fn new(source_id: &SourceId, path: Arc<str>) -> Self {
        Function {
            id_prefix: BlockIdPrefix::new(&source_id.0, &path),
            path,
            blocks_count: 0,
        }
    }
}

fn key_name(key: &ast::PropertyKey<'_>) -> Option<String> {
    match key {
        ast::PropertyKey::PrivateIdentifier(identifier) => {
            Some(format!("#{}", identifier.name))
        }
        _ => key.static_name().map(|name| name.into_owned()),
    }
}

/// `a.b.c` for static member chains rooted at an identifier or `this`.
fn member_path(expression: &Expression<'_>) -> Option<String> {
    match expression {
        Expression::Identifier(identifier) => Some(identifier.name.to_string()),
        Expression::ThisExpression(_) => Some("this".to_string()),
        Expression::StaticMemberExpression(member) => {
            let object = member_path(&member.object)?;
            Some(format!("{object}.{}", member.property.name))
        }
        _ => None,
    }
}

fn target_name(target: &AssignmentTarget<'_>) -> Option<String> {
    match target {
        AssignmentTarget::StaticMemberExpression(member) => {
            let property = member.property.name.as_str();
            Some(match member_path(&member.object) {
                Some(object) => format!("{object}.{property}"),
                None => property.to_string(),
            })
        }
        _ => target.get_identifier_name().map(|name| name.to_string()),
    }
}

/// The name a function or class expression gets from its position.
/// `ancestors` starts at its parent. Object literal values are prefixed
/// with the object's name.
fn name_from_ancestors<'a, 't>(
    mut ancestors: impl Iterator<Item = Ancestor<'a, 't>>,
) -> Option<String> {
    match ancestors.next()? {
        Ancestor::VariableDeclaratorInit(declarator) => declarator
            .id()
            .get_identifier_name()
            .map(|name| name.to_string()),
        Ancestor::MethodDefinitionValue(method) => {
            let name = key_name(method.key())?;
            Some(match method.kind() {
                ast::MethodDefinitionKind::Get => format!("get {name}"),
                ast::MethodDefinitionKind::Set => format!("set {name}"),
                _ => name,
            })
        }
        Ancestor::ObjectPropertyValue(property) => {
            let name = key_name(property.key())?;
            let name = match property.kind() {
                ast::PropertyKind::Get => format!("get {name}"),
                ast::PropertyKind::Set => format!("set {name}"),
                ast::PropertyKind::Init => name,
            };
            let object = match ancestors.next() {
                Some(Ancestor::ObjectExpressionProperties(_)) => {
                    name_from_ancestors(ancestors)
                }
                _ => None,
            };
            Some(match object {
                Some(object) => format!("{object}.{name}"),
                None => name,
            })
        }
        Ancestor::PropertyDefinitionValue(property) => key_name(property.key()),
        Ancestor::AssignmentExpressionRight(assignment) => {
            target_name(assignment.left())
        }
        _ => None,
    }
}

impl Instrumenter {
    fn enter_scope(&mut self, name: Option<String>) {
        let parent = *self.scopes.last().expect("no <top> scope");
        let index = match name {
            None => parent,
            Some(name) => {
                let path = if parent == 0 {
                    name
                } else {
                    format!("{}.{name}", self.functions[parent].path)
                };
                match self.function_indexes.get(path.as_str()) {
                    Some(&index) => index,
                    None => {
                        let function =
                            Function::new(&self.source_id, path.into());
                        let index = self.functions.len();
                        self.function_indexes
                            .insert(function.path.clone(), index);
                        self.functions.push(function);
                        index
                    }
                }
            }
        };
        self.scopes.push(index);
    }

    fn exit_scope(&mut self) {
        self.scopes.pop();
    }

    fn new_block(&mut self, span: Span) -> u32 {
        let scope = *self.scopes.last().expect("no <top> scope");
        let function = &mut self.functions[scope];
        let id = function.id_prefix.id(function.blocks_count);
        function.blocks_count += 1;
        let line = self
            .line_starts
            .partition_point(|&start| start <= span.start)
            as u32;
        self.blocks.push(Block {
            id,
            script: self.source_id.0.clone(),
            function: function.path.clone(),
            line,
        });
        id
    }

    fn coverage_hooks<'b>(
        &mut self,
        ctx: &mut TraverseCtx<'b, ()>,
        span: Span,
    ) -> allocator::Vec<'b, Expression<'b>> {
        let antithesis_member = |name: &'static str| -> Expression {
            ctx.ast
                .member_expression_static(
                    SPAN,
                    ctx.ast.expression_identifier(SPAN, NAMESPACE),
                    ctx.ast.identifier_name(SPAN, name),
                    false,
                )
                .into()
        };

        let id = self.new_block(span);

        let branch_id = ctx.ast.expression_numeric_literal(
            SPAN,
            id as f64,
            None,
            ast::NumberBase::Decimal,
        );

        let edge_index = ctx.ast.expression_binary(
            SPAN,
            ctx.ast.expression_binary(
                SPAN,
                ctx.ast.expression_binary(
                    SPAN,
                    branch_id.clone_in_with_semantic_ids(ctx.ast.allocator),
                    ast::BinaryOperator::BitwiseXOR,
                    antithesis_member(LOCATION_PREVIOUS),
                ),
                ast::BinaryOperator::ShiftRightZeroFill,
                ctx.ast.expression_numeric_literal(
                    SPAN,
                    0.0,
                    None,
                    ast::NumberBase::Decimal,
                ),
            ),
            ast::BinaryOperator::Remainder,
            ctx.ast.expression_numeric_literal(
                SPAN,
                EDGE_MAP_SIZE as f64,
                None,
                ast::NumberBase::Decimal,
            ),
        );

        let edge_addition = ctx.ast.expression_assignment(
            SPAN,
            AssignmentOperator::Addition,
            AssignmentTarget::ComputedMemberExpression(
                ctx.ast.alloc_computed_member_expression(
                    SPAN,
                    antithesis_member(EDGES_CURRENT),
                    edge_index,
                    false,
                ),
            ),
            ctx.ast.expression_numeric_literal(
                SPAN,
                1.0,
                None,
                ast::NumberBase::Decimal,
            ),
        );

        let location_previous_update = ctx.ast.expression_assignment(
            SPAN,
            AssignmentOperator::Assign,
            AssignmentTarget::StaticMemberExpression(
                ctx.ast.alloc_static_member_expression(
                    SPAN,
                    ctx.ast.expression_identifier(SPAN, NAMESPACE),
                    ctx.ast.identifier_name(SPAN, LOCATION_PREVIOUS),
                    false,
                ),
            ),
            ctx.ast.expression_binary(
                SPAN,
                branch_id.clone_in_with_semantic_ids(ctx.ast.allocator),
                ast::BinaryOperator::ShiftRight,
                ctx.ast.expression_numeric_literal(
                    SPAN,
                    1.0,
                    None,
                    ast::NumberBase::Decimal,
                ),
            ),
        );

        ctx.ast
            .vec_from_array([edge_addition, location_previous_update])
    }

    /// Adds the following two statements to the start of block, or wraps a single statement
    /// in a block with these two at the start:
    ///
    /// ```not_rust
    /// __bombadil__.edges_current[((<id> ^ __bombadil__.previous) >>> 0) % 65536] += 1;
    /// __bombadil__.previous = <id> >> 1;
    /// ```
    ///
    /// The <id> is the block's structural 31-bit ID.
    fn insert_coverage_hook<'b>(
        &mut self,
        ctx: &mut TraverseCtx<'b, ()>,
        statement: &'_ mut Statement<'b>,
        span: Span,
    ) {
        let hook_expressions = self.coverage_hooks(ctx, span);
        let mut statements =
            ctx.ast.vec_with_capacity(hook_expressions.len() + 1);
        for expression in hook_expressions {
            statements.push(ctx.ast.statement_expression(SPAN, expression));
        }
        if let Statement::BlockStatement(block_statement) = statement {
            block_statement.body.splice(0..0, statements);
        } else {
            statements.push(statement.take_in(ctx.ast.allocator));
            *statement = ctx.ast.statement_block(SPAN, statements);
        }
    }

    fn wrap_expression_with_coverage_hook<'b>(
        &mut self,
        ctx: &mut TraverseCtx<'b, ()>,
        expression: &'_ mut Expression<'b>,
    ) {
        let span = expression.span();
        let mut expressions = self.coverage_hooks(ctx, span);

        let expression_old = expression.take_in(ctx.ast.allocator);
        expressions.push(expression_old);

        *expression = ctx.ast.expression_sequence(SPAN, expressions);
    }
}

impl<'a> Traverse<'a, ()> for Instrumenter {
    fn enter_function(
        &mut self,
        function: &mut ast::Function<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        let name = function
            .name()
            .map(|name| name.to_string())
            .or_else(|| name_from_ancestors(ctx.ancestors()));
        self.enter_scope(name);
    }

    fn exit_function(
        &mut self,
        _function: &mut ast::Function<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.exit_scope();
    }

    fn enter_arrow_function_expression(
        &mut self,
        _function: &mut ast::ArrowFunctionExpression<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.enter_scope(name_from_ancestors(ctx.ancestors()));
    }

    fn exit_arrow_function_expression(
        &mut self,
        _function: &mut ast::ArrowFunctionExpression<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.exit_scope();
    }

    fn enter_class(
        &mut self,
        class: &mut ast::Class<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        let name = class
            .name()
            .map(|name| name.to_string())
            .or_else(|| name_from_ancestors(ctx.ancestors()));
        self.enter_scope(name);
    }

    fn exit_class(
        &mut self,
        _class: &mut ast::Class<'a>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.exit_scope();
    }

    /// Add coverage hooks to ternary expression branches.
    fn exit_conditional_expression(
        &mut self,
        expression: &mut ast::ConditionalExpression<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.wrap_expression_with_coverage_hook(
            ctx,
            &mut expression.consequent,
        );
        self.wrap_expression_with_coverage_hook(ctx, &mut expression.alternate);
    }

    /// Add coverage hooks to if statement branches.
    fn exit_if_statement(
        &mut self,
        statement: &mut ast::IfStatement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        let span = statement.consequent.span();
        self.insert_coverage_hook(ctx, &mut statement.consequent, span);

        let alternate_span = statement
            .alternate
            .as_ref()
            .map_or(statement.span, |alternate| alternate.span());
        let empty_block = ctx.ast.statement_block(SPAN, ctx.ast.vec());
        if statement.alternate.is_none() {
            statement.alternate = Some(empty_block);
        }
        let alternate = statement.alternate.as_mut().unwrap();

        self.insert_coverage_hook(ctx, alternate, alternate_span);
    }

    fn exit_for_statement(
        &mut self,
        statement: &mut ast::ForStatement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        let span = statement.body.span();
        self.insert_coverage_hook(ctx, &mut statement.body, span);
    }

    fn exit_for_in_statement(
        &mut self,
        statement: &mut ast::ForInStatement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        let span = statement.body.span();
        self.insert_coverage_hook(ctx, &mut statement.body, span);
    }

    fn exit_for_of_statement(
        &mut self,
        statement: &mut ast::ForOfStatement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        let span = statement.body.span();
        self.insert_coverage_hook(ctx, &mut statement.body, span);
    }

    fn exit_switch_case(
        &mut self,
        node: &mut ast::SwitchCase<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        let expressions = self.coverage_hooks(ctx, node.span);
        let mut statements = ctx.ast.vec_with_capacity(expressions.len() + 1);
        for expression in expressions {
            statements.push(ctx.ast.statement_expression(SPAN, expression));
        }
        node.consequent.splice(0..0, statements);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::{Context, Source, object::builtins::JsUint8Array};
    use hegel::{
        Generator, TestCase,
        generators::{
            booleans, integers, permutations, sampled_from, text, tuples2, vecs,
        },
    };
    use insta::assert_snapshot;
    use oxc::span::ContentEq;
    use std::collections::{BTreeMap, BTreeSet};

    fn instrument(source: &str, source_type: SourceType) -> Instrumented {
        instrument_source_code(SourceId("/test.js".into()), source, source_type)
            .unwrap()
    }

    fn snapshot(source: &str) -> String {
        instrument(source, SourceType::cjs()).snapshot()
    }

    fn hook_ids(code: &str) -> Vec<u32> {
        let hook = formatcp!("{NAMESPACE}.{EDGES_CURRENT}[((");
        code.split(hook)
            .skip(1)
            .map(|rest| {
                let digits = rest.split(' ').next().unwrap();
                digits.parse().expect(digits)
            })
            .collect()
    }

    fn coverage_hits(source_id: SourceId, source: &str) -> u32 {
        let code = instrument_source_code(source_id, source, SourceType::cjs())
            .unwrap()
            .text;
        let mut context = Context::default();
        context
            .eval(Source::from_bytes("var window = globalThis;"))
            .unwrap();
        context.eval(Source::from_bytes(&code)).unwrap();
        let edges = context
            .eval(Source::from_bytes("__bombadil__.edges_current"))
            .unwrap();
        JsUint8Array::from_object(edges.as_object().unwrap())
            .unwrap()
            .to_vec(&mut context)
            .unwrap()
            .into_iter()
            .map(u32::from)
            .sum()
    }

    #[hegel::test(test_cases = 32)]
    fn generated_source_ids_record_coverage(tc: TestCase) {
        let source_id = SourceId(tc.draw(text()).into());
        let first = tc.draw(booleans());
        let second = tc.draw(booleans());
        let source = format!(
            "if ({first}) {{}} else {{}}\nif ({second}) {{}} else {{}}"
        );
        assert_eq!(coverage_hits(source_id, &source), 2);
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Loop {
        For,
        ForIn,
        ForOf,
    }

    #[derive(Clone, Debug)]
    enum Statement {
        If(Vec<Statement>),
        IfElse(Vec<Statement>, Vec<Statement>),
        Ternary,
        Loop(Loop, Vec<Statement>),
        /// Not instrumented.
        While(Vec<Statement>),
        Switch(Vec<Vec<Statement>>),
        /// Anonymous; its blocks belong to the enclosing named function.
        Callback {
            arrow: bool,
            body: Vec<Statement>,
        },
        Function {
            form: Form,
            name: &'static str,
            body: Vec<Statement>,
        },
    }

    impl Statement {
        /// Nested statement lists, excluding function bodies.
        fn blocks_mut(&mut self) -> Vec<&mut Vec<Statement>> {
            match self {
                Statement::If(body)
                | Statement::Loop(_, body)
                | Statement::While(body)
                | Statement::Callback { body, .. } => vec![body],
                Statement::IfElse(consequent, alternate) => {
                    vec![consequent, alternate]
                }
                Statement::Switch(cases) => cases.iter_mut().collect(),
                Statement::Ternary | Statement::Function { .. } => vec![],
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Form {
        Declaration,
        Arrow,
        Expression,
        Method,
        Property,
        Assignment,
    }

    impl Form {
        const ALL: [Form; 6] = [
            Form::Declaration,
            Form::Arrow,
            Form::Expression,
            Form::Method,
            Form::Property,
            Form::Assignment,
        ];

        /// The lines before and after the body, and the function path.
        fn wrap(self, name: &str) -> (String, &'static str, String) {
            let path = name.to_string();
            match self {
                Form::Declaration => {
                    (format!("function {name}() {{"), "}", path)
                }
                Form::Arrow => (format!("const {name} = () => {{"), "};", path),
                Form::Expression => {
                    (format!("const {name} = function () {{"), "};", path)
                }
                Form::Method => (
                    format!("class {name} {{ run() {{"),
                    "} }",
                    format!("{name}.run"),
                ),
                Form::Property => (
                    format!("const object_{name} = {{ {name}: function () {{"),
                    "} };",
                    format!("object_{name}.{name}"),
                ),
                Form::Assignment => (
                    format!("app.{name} = function () {{"),
                    "};",
                    format!("app.{name}"),
                ),
            }
        }
    }

    const TOP: &str = "<top>";

    /// Source lines and the expected blocks (function path, line), in
    /// instrumentation order: inner statements first.
    #[derive(Default)]
    struct Rendered {
        lines: Vec<String>,
        blocks: Vec<(String, u32)>,
    }

    impl Rendered {
        fn line(&mut self, line: impl Into<String>) -> u32 {
            self.lines.push(line.into());
            self.lines.len() as u32
        }

        fn block(&mut self, path: &str, line: u32) {
            self.blocks.push((path.to_string(), line));
        }

        fn statements(&mut self, path: &str, statements: &[Statement]) {
            for statement in statements {
                self.statement(path, statement);
            }
        }

        fn statement(&mut self, path: &str, statement: &Statement) {
            match statement {
                Statement::If(body) => {
                    let line = self.line("if (x) {");
                    self.statements(path, body);
                    self.line("}");
                    self.block(path, line);
                    self.block(path, line);
                }
                Statement::IfElse(consequent, alternate) => {
                    let line = self.line("if (x) {");
                    self.statements(path, consequent);
                    let line_else = self.line("} else {");
                    self.statements(path, alternate);
                    self.line("}");
                    self.block(path, line);
                    self.block(path, line_else);
                }
                Statement::Ternary => {
                    let line = self.line("x ? 1 : 2;");
                    self.block(path, line);
                    self.block(path, line);
                }
                Statement::Loop(kind, body) => {
                    let line = self.line(match kind {
                        Loop::For => "for (; x; ) {",
                        Loop::ForIn => "for (const k in x) {",
                        Loop::ForOf => "for (const v of x) {",
                    });
                    self.statements(path, body);
                    self.line("}");
                    self.block(path, line);
                }
                Statement::While(body) => {
                    self.line("while (x) {");
                    self.statements(path, body);
                    self.line("}");
                }
                Statement::Switch(cases) => {
                    self.line("switch (x) {");
                    for (index, case) in cases.iter().enumerate() {
                        let line = self.line(format!("case {index}:"));
                        self.statements(path, case);
                        self.line("break;");
                        self.block(path, line);
                    }
                    self.line("}");
                }
                Statement::Callback { arrow, body } => {
                    self.line(if *arrow {
                        "[].forEach(() => {"
                    } else {
                        "[].forEach(function () {"
                    });
                    self.statements(path, body);
                    self.line("});");
                }
                Statement::Function { form, name, body } => {
                    let (before, after, name) = form.wrap(name);
                    let path = if path == TOP {
                        name
                    } else {
                        format!("{path}.{name}")
                    };
                    self.line(before);
                    self.statements(&path, body);
                    self.line(after);
                }
            }
        }
    }

    #[derive(Clone, Debug)]
    struct Program(Vec<Statement>);

    impl Program {
        fn render(&self) -> (String, Vec<(String, u32)>) {
            self.render_with(|| "\n")
        }

        /// Like `render`, joining lines with terminators from `terminator`.
        fn render_with(
            &self,
            mut terminator: impl FnMut() -> &'static str,
        ) -> (String, Vec<(String, u32)>) {
            let mut rendered = Rendered::default();
            rendered.statements(TOP, &self.0);
            let mut source = String::new();
            for (index, line) in rendered.lines.iter().enumerate() {
                if index > 0 {
                    source.push_str(terminator());
                }
                source.push_str(line);
            }
            (source, rendered.blocks)
        }

        fn blocks(&self) -> Vec<Block> {
            instrument(&self.render().0, SourceType::unambiguous()).blocks
        }
    }

    const NAMES: [&str; 4] = ["f", "g", "h", "k"];

    /// `names` are the function names free to declare in this list.
    fn statements(
        tc: &TestCase,
        depth: usize,
        names: &mut Vec<&'static str>,
        paths_distinct: bool,
    ) -> Vec<Statement> {
        let count = tc.draw_silent(integers::<usize>().max_value(3));
        (0..count)
            .map(|_| statement(tc, depth, names, paths_distinct))
            .collect()
    }

    fn statement(
        tc: &TestCase,
        depth: usize,
        names: &mut Vec<&'static str>,
        paths_distinct: bool,
    ) -> Statement {
        let block = |names: &mut Vec<&'static str>| match depth {
            0 => vec![],
            _ => statements(tc, depth - 1, names, paths_distinct),
        };
        let kinds = if names.is_empty() { 9 } else { 10 };
        match tc.draw_silent(integers::<u8>().max_value(kinds - 1)) {
            0 => Statement::If(block(names)),
            1 => Statement::IfElse(block(names), block(names)),
            2 => Statement::Ternary,
            3 => Statement::Loop(Loop::For, block(names)),
            4 => Statement::Loop(Loop::ForIn, block(names)),
            5 => Statement::Loop(Loop::ForOf, block(names)),
            6 => Statement::While(block(names)),
            7 => {
                let cases = tc.draw_silent(integers::<usize>().max_value(3));
                Statement::Switch((0..cases).map(|_| block(names)).collect())
            }
            8 => Statement::Callback {
                arrow: tc.draw_silent(booleans()),
                // A callback may redeclare the enclosing function's names,
                // giving two functions the same path.
                body: match paths_distinct {
                    true => block(names),
                    false => block(&mut NAMES.to_vec()),
                },
            },
            _ => {
                let index = tc.draw_silent(
                    integers::<usize>().max_value(names.len() - 1),
                );
                Statement::Function {
                    form: tc.draw_silent(sampled_from(Form::ALL.as_slice())),
                    name: names.remove(index),
                    body: match depth {
                        0 => vec![],
                        _ => statements(
                            tc,
                            depth - 1,
                            &mut NAMES.to_vec(),
                            paths_distinct,
                        ),
                    },
                }
            }
        }
    }

    /// With `paths_distinct`, no two functions have the same path.
    #[hegel::composite]
    fn programs(tc: &TestCase, paths_distinct: bool) -> Program {
        Program(statements(tc, 2, &mut NAMES.to_vec(), paths_distinct))
    }

    fn ids_by_function(blocks: &[Block]) -> BTreeMap<Arc<str>, Vec<u32>> {
        let mut ids: BTreeMap<Arc<str>, Vec<u32>> = BTreeMap::new();
        for block in blocks {
            ids.entry(block.function.clone())
                .or_default()
                .push(block.id);
        }
        ids
    }

    #[hegel::test]
    fn block_table_matches_the_program(tc: TestCase) {
        let program = tc.draw(programs(false).print_as_debug());
        let (source, expected) = program.render();
        tc.note(&source);
        let found: Vec<(String, u32)> =
            instrument(&source, SourceType::unambiguous())
                .blocks
                .into_iter()
                .map(|block| (block.function.to_string(), block.line))
                .collect();
        assert_eq!(found, expected);
    }

    const LINE_TERMINATORS: [&str; 5] =
        ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"];

    #[hegel::test]
    fn block_lines_count_every_line_terminator(tc: TestCase) {
        let program = tc.draw(programs(false).print_as_debug());
        let (source, expected) = program.render_with(|| {
            tc.draw_silent(sampled_from(LINE_TERMINATORS.as_slice()))
        });
        tc.note(&format!("{source:?}"));
        let found: Vec<u32> = instrument(&source, SourceType::unambiguous())
            .blocks
            .into_iter()
            .map(|block| block.line)
            .collect();
        let expected: Vec<u32> =
            expected.into_iter().map(|(_, line)| line).collect();
        assert_eq!(found, expected);
    }

    #[hegel::test]
    fn line_starts_follow_every_line_terminator(tc: TestCase) {
        let lines = tc.draw(vecs(tuples2(
            text().exclude_characters("\n\r\u{2028}\u{2029}"),
            sampled_from(LINE_TERMINATORS.as_slice()),
        )));
        let mut source = String::new();
        let mut expected = vec![0];
        for (line, terminator) in lines {
            // An empty line between CR and LF would merge them into CRLF.
            if line.is_empty() && source.ends_with('\r') && terminator == "\n" {
                source.push(' ');
            }
            source.push_str(&line);
            source.push_str(terminator);
            expected.push(source.len() as u32);
        }
        source.push_str(
            &tc.draw(text().exclude_characters("\n\r\u{2028}\u{2029}")),
        );
        assert_eq!(line_starts(&source), expected);
    }

    #[hegel::test]
    fn block_table_ids_are_the_ids_in_the_code(tc: TestCase) {
        let program = tc.draw(programs(false).print_as_debug());
        let (source, _) = program.render();
        tc.note(&source);
        let instrumented = instrument(&source, SourceType::unambiguous());

        let table: BTreeSet<u32> =
            instrumented.blocks.iter().map(|block| block.id).collect();
        assert_eq!(table.len(), instrumented.blocks.len(), "duplicate ids");

        let hooks = hook_ids(&instrumented.text);
        assert_eq!(hooks.len(), instrumented.blocks.len());
        assert_eq!(hooks.into_iter().collect::<BTreeSet<u32>>(), table);
    }

    /// Inserts `statement` at the start of the `index`th function (source
    /// order) and returns its path, if it exists.
    fn insert_in_function(
        statements: &mut [Statement],
        path: &str,
        index: &mut usize,
        statement: &Statement,
    ) -> Option<String> {
        for nested in statements {
            if let Statement::Function { form, name, body } = nested {
                let name = form.wrap(name).2;
                let path = if path == TOP {
                    name
                } else {
                    format!("{path}.{name}")
                };
                if *index == 0 {
                    body.insert(0, statement.clone());
                    return Some(path);
                }
                *index -= 1;
                let found = insert_in_function(body, &path, index, statement);
                if found.is_some() {
                    return found;
                }
            }
            for block in nested.blocks_mut() {
                let found = insert_in_function(block, path, index, statement);
                if found.is_some() {
                    return found;
                }
            }
        }
        None
    }

    #[hegel::test]
    fn block_ids_survive_edits_in_other_functions(tc: TestCase) {
        let before = tc.draw(programs(false).print_as_debug());
        let mut index = tc.draw(integers::<usize>().max_value(8));
        // No names to declare, so it only adds blocks to its function.
        let statement = statement(&tc, 1, &mut vec![], true);
        tc.note(&format!("inserted: {statement:?}"));

        let mut after = before.clone();
        let path =
            insert_in_function(&mut after.0, TOP, &mut index, &statement)
                .unwrap_or_else(|| {
                    after.0.insert(0, statement);
                    TOP.to_string()
                });

        let mut ids_before = ids_by_function(&before.blocks());
        let mut ids_after = ids_by_function(&after.blocks());
        ids_before.remove(path.as_str());
        ids_after.remove(path.as_str());
        assert_eq!(ids_before, ids_after);
    }

    /// Permutes the functions of every statement list among themselves.
    fn reorder_functions(tc: &TestCase, statements: &mut [Statement]) {
        let slots: Vec<usize> = (0..statements.len())
            .filter(|&index| {
                matches!(statements[index], Statement::Function { .. })
            })
            .collect();
        let functions: Vec<Statement> =
            slots.iter().map(|&slot| statements[slot].clone()).collect();
        let reordered = tc.draw_silent(permutations(functions));
        for (slot, function) in slots.into_iter().zip(reordered) {
            statements[slot] = function;
        }
        for statement in statements {
            if let Statement::Function { body, .. } = statement {
                reorder_functions(tc, body);
            }
            for block in statement.blocks_mut() {
                reorder_functions(tc, block);
            }
        }
    }

    #[hegel::test]
    fn block_ids_survive_reordering_functions(tc: TestCase) {
        let before = tc.draw(programs(true).print_as_debug());
        let mut after = before.clone();
        reorder_functions(&tc, &mut after.0);
        tc.note(&after.render().0);
        assert_eq!(
            ids_by_function(&before.blocks()),
            ids_by_function(&after.blocks())
        );
    }

    #[hegel::test]
    fn block_ids_differ_between_scripts(tc: TestCase) {
        let program = tc.draw(programs(false).print_as_debug());
        let (source, _) = program.render();
        let ids = |script: &str| -> BTreeSet<u32> {
            instrument_source_code(
                SourceId(script.into()),
                &source,
                SourceType::unambiguous(),
            )
            .unwrap()
            .blocks
            .into_iter()
            .map(|block| block.id)
            .collect()
        };
        assert!(ids("/a.js").is_disjoint(&ids("/b.js")));
    }

    #[test]
    fn test_block_functions() {
        let source = indoc::indoc! {r#"
            if (a) {}
            function declared(x) { return x ? 1 : 2; }
            function outer() {
                if (a) {}
                function inner() { if (b) {} }
                const local = () => { if (c) {} };
                return () => { if (d) {} };
            }
            const arrow = (x) => { if (x) {} };
            const expression = function () { if (x) {} };
            const renamed = function original() { if (x) {} };
            let later; later = () => { if (x) {} };
            async function waits() { if (await x) {} }
            function* yields() { for (const y of ys) {} }
            function defaults(callback = () => { if (x) {} }) {}
            class Store {
                constructor() { if (x) {} }
                add(item) { for (const i of item) {} }
                static create() { if (x) {} }
                get size() { return x ? 1 : 2; }
                set size(value) { if (value) {} }
                #secret() { if (x) {} }
                [computed]() { if (x) {} }
                "quoted name"() { if (x) {} }
                remove = () => { [].forEach(function () { if (y) {} }); };
                static { if (x) {} }
            }
            const Widget = class { render() { if (x) {} } };
            const Outer = class Inner { render() { if (x) {} } };
            const handlers = {
                onClick: function () { if (z) {} },
                onKey: () => { if (z) {} },
                onDrag() { if (z) {} },
                get value() { return z ? 1 : 2; },
                [computed]: function () { if (z) {} },
                nested: { deep() { if (z) {} } },
            };
            const first = { render() { if (x) {} } };
            const second = { render() { if (x) {} } };
            app.render = function () { while (false) {} if (w) {} };
            exports.handler = () => { if (w) {} };
            a.b.c = function () { if (w) {} };
            [].forEach(function () { if (x) {} });
            [].forEach((x) => { if (x) {} });
            (function () { if (x) {} })();
            (() => { if (x) {} })();
            switch (x) { case 1: break; default: break; }
            export default function () { if (x) {} }
        "#};
        let lines: Vec<&str> = source.lines().collect();
        let mut blocks = instrument(source, SourceType::mjs()).blocks;
        blocks.sort_by_key(|block| block.line);
        let table: String = blocks
            .iter()
            .map(|block| {
                format!(
                    "{:>3}  {:<22}  {}\n",
                    block.line,
                    block.function,
                    lines[block.line as usize - 1].trim()
                )
            })
            .collect();
        assert_snapshot!(table);
    }

    #[test]
    fn instrumented_script_preserves_directive_prologue() {
        let source = "'custom'; 'use strict'; 0;";
        let allocator = Allocator::default();
        let source_type = SourceType::cjs();
        let original = parse(&allocator, source, source_type).unwrap();
        let prelude = parse(&allocator, PRELUDE, source_type).unwrap();
        let code = instrument(source, source_type).text;
        let actual = parse(&allocator, &code, source_type).unwrap();

        assert!(actual.directives.content_eq(&original.directives));
        assert_eq!(actual.body.len(), prelude.body.len() + original.body.len());
        assert!(
            actual
                .body
                .iter()
                .zip(prelude.body.iter().chain(&original.body))
                .all(|(actual, expected)| actual.content_eq(expected))
        );
    }

    #[test]
    fn test_instrument_source_code_ternary() {
        let source_text = r#"
            function example(a, b, c) {
                return a ? b : c;
            }
            console.log(example(true, 1, 2));
        "#;

        assert_snapshot!(snapshot(source_text));
    }

    #[test]
    fn test_instrument_source_code_if() {
        let source_text = r#"
            let x;
            function example(a, b) {
                if (a) {
                    x = b;
                }
            }
            console.log(example(true, 1));
        "#;

        assert_snapshot!(snapshot(source_text));
    }

    #[test]
    fn test_instrument_source_code_if_else() {
        let source_text = r#"
            function example(a, b, c) {
                if (a) {
                    return b;
                } else {
                    return c;
                }
            }
            console.log(example(true, 1, 2));
        "#;

        assert_snapshot!(snapshot(source_text));
    }

    #[test]
    fn test_instrument_source_code_ternary_assignment() {
        let source_text = r#"
            let x;
            function example(a, b, c) {
                return a ? (console.log(x), x = b) : (console.log(x), x = c);
            }
            console.log(example(true, 1, 2), x);
        "#;

        assert_snapshot!(snapshot(source_text));
    }

    #[test]
    fn test_instrument_source_code_ternary_comma_operator() {
        let source_text = r#"
            let x = 1;
            let y = 2;
            let z = 3;
            function example(a, b, c) {
                return a ? (x = y, b) : (y = z, c);
            }
            console.log(example(true, 1, 2), x, y, z);
        "#;

        assert_snapshot!(snapshot(source_text));
    }

    #[test]
    fn test_instrument_source_code_ternary_await() {
        let source_text = r#"
            async function example(a) {
                return a ? await bar() : await baz();
            }
        "#;

        assert_snapshot!(snapshot(source_text));
    }

    #[test]
    fn test_instrument_source_code_switch() {
        let source_text = r#"
            function foo() {
                let bar = get();
                while (true) {
                    switch (bar) {
                        case 1:
                            return bar;
                        case 2:
                            break;
                        case "foo":
                        case "bar":
                        case "baz":
                            continue;
                        default:
                            return no;
                    }
                }
            }
            "#;

        assert_snapshot!(snapshot(source_text));
    }

    #[test]
    fn test_instrument_source_code_ternary_assignment_with_await() {
        let source_text = r#"
            async function test() {
                return f(x) ? y = await z.instantiator(t) : f(y);
            }
        "#;

        assert_snapshot!(snapshot(source_text));
    }
}
