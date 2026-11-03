use anyhow::anyhow;
use std::collections::HashMap;
use std::fmt;

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
    Block, Instrumented, block_id, source_id::SourceId, source_map::SourceMap,
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
/// The ids of the blocks run since the last state capture.
pub const HITS: &str = "hits";

const LOCATION_PREVIOUS: &str = "previous";

const PRELUDE: &str = formatcp!(
    "window.{NAMESPACE} = window.{NAMESPACE} || {{
        {EDGES_PREVIOUS}: new Uint8Array({EDGE_MAP_SIZE}),
        {EDGES_CURRENT}: new Uint8Array({EDGE_MAP_SIZE}),
        {LOCATION_PREVIOUS}: 0,
        {HITS}: new Set(),
    }};"
);

pub fn instrument_source_code(
    source_id: SourceId,
    source_text: &str,
    source_type: SourceType,
) -> InstrumentationResult<Instrumented> {
    instrument_source_code_with_map(source_id, source_text, source_type, None)
}

/// As [`instrument_source_code`], with each block resolved to its original
/// file and line through the script's source map when there is one.
pub fn instrument_source_code_with_map(
    source_id: SourceId,
    source_text: &str,
    source_type: SourceType,
    source_map: Option<&SourceMap>,
) -> InstrumentationResult<Instrumented> {
    let allocator = Allocator::default();
    let mut program = parse(&allocator, source_text, source_type)?;
    let mut blocks =
        instrument_program(&allocator, &mut program, source_id, source_text)?;
    if let Some(map) = source_map {
        for block in &mut blocks {
            if let Some(original) = map.lookup(block.line, block.column) {
                block.source = Some(original.source);
                block.source_line = Some(original.line);
                block.vendor = original.ignored;
            }
        }
    }

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
    let mut instrumenter = Instrumenter {
        source_id,
        line_starts: std::iter::once(0)
            .chain(source_text.match_indices('\n').map(|(i, _)| i as u32 + 1))
            .collect(),
        names: Vec::new(),
        named: Vec::new(),
        ordinals: HashMap::new(),
        blocks: Vec::new(),
        spans: Vec::new(),
    };
    traverse_mut(&mut instrumenter, allocator, program, scopes, ());
    // Columns in UTF-16 units, as source maps count them.
    for (block, start) in
        instrumenter.blocks.iter_mut().zip(&instrumenter.spans)
    {
        let line_start =
            instrumenter.line_starts[block.line as usize - 1] as usize;
        block.column = source_text
            .get(line_start..*start as usize)
            .map(|text| text.encode_utf16().count() as u32)
            .unwrap_or(0);
    }

    Ok(instrumenter.blocks)
}

struct Instrumenter {
    source_id: SourceId,
    line_starts: Vec<u32>,
    names: Vec<String>,
    /// Per enclosing class or function, whether it pushed onto `names`.
    named: Vec<bool>,
    /// Per function path.
    ordinals: HashMap<String, u32>,
    blocks: Vec<Block>,
    /// Byte offset of each block's start, parallel to `blocks`.
    spans: Vec<u32>,
}

/// The name a function or class gets from where it's defined, when it has
/// none of its own: `const f = () => …`, `{ f() {…} }`, `{ f: function () {…} }`,
/// `class { f() {…} }`, `class { f = () => … }`, `x.f = function () {…}`.
fn name_from_parent(ctx: &TraverseCtx<'_, ()>) -> Option<String> {
    match ctx.parent() {
        Ancestor::VariableDeclaratorInit(declarator) => declarator
            .id()
            .get_identifier_name()
            .map(|name| name.to_string()),
        Ancestor::MethodDefinitionValue(method) => {
            method.key().static_name().map(|name| name.to_string())
        }
        Ancestor::ObjectPropertyValue(property) => {
            property.key().static_name().map(|name| name.to_string())
        }
        Ancestor::PropertyDefinitionValue(property) => {
            property.key().static_name().map(|name| name.to_string())
        }
        Ancestor::AssignmentExpressionRight(assignment) => assignment
            .left()
            .get_identifier_name()
            .map(|name| name.to_string()),
        _ => None,
    }
}

impl Instrumenter {
    fn enter_scope(&mut self, name: Option<String>) {
        self.named.push(name.is_some());
        self.names.extend(name);
    }

    fn exit_scope(&mut self) {
        if self.named.pop() == Some(true) {
            self.names.pop();
        }
    }

    fn new_block(&mut self, span: Span) -> u32 {
        let function = if self.names.is_empty() {
            "<top>".to_string()
        } else {
            self.names.join(".")
        };
        let ordinal = self.ordinals.entry(function.clone()).or_default();
        let id = block_id(&self.source_id.0, &function, *ordinal);
        *ordinal += 1;
        let line = self
            .line_starts
            .partition_point(|&start| start <= span.start)
            as u32;
        self.spans.push(span.start);
        self.blocks.push(Block {
            id,
            script: self.source_id.0.clone(),
            function,
            line,
            column: 0,
            source: None,
            source_line: None,
            vendor: false,
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

        let hit = ctx.ast.expression_call(
            SPAN,
            ctx.ast
                .member_expression_static(
                    SPAN,
                    antithesis_member(HITS),
                    ctx.ast.identifier_name(SPAN, "add"),
                    false,
                )
                .into(),
            None::<
                oxc::allocator::Box<'_, ast::TSTypeParameterInstantiation<'_>>,
            >,
            ctx.ast.vec1(ast::Argument::from(
                branch_id.clone_in_with_semantic_ids(ctx.ast.allocator),
            )),
            false,
        );

        ctx.ast
            .vec_from_array([edge_addition, location_previous_update, hit])
    }

    /// Adds the following two statements to the start of block, or wraps a single statement
    /// in a block with these two at the start:
    ///
    /// ```not_rust
    /// antithesis.coverage[((<id> ^ antithesis.previous) >>> 0) % 65536] += 1;
    /// antithesis.previous = <id> >> 1;
    /// ```
    ///
    /// The <id> is a random integer identifying branch.
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
            .or_else(|| name_from_parent(ctx));
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
        self.enter_scope(name_from_parent(ctx));
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
            .or_else(|| name_from_parent(ctx));
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
    use crate::instrumentation::BLOCK_ID_BITS;
    use boa_engine::{Context, Source, object::builtins::JsUint8Array};
    use hegel::{
        Generator, TestCase,
        generators::{booleans, integers, sampled_from, vecs},
        tuples,
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
        let source_id = SourceId(format!("/{}.js", tc.draw(integers::<u64>())));
        let first = tc.draw(booleans());
        let second = tc.draw(booleans());
        let source = format!(
            "if ({first}) {{}} else {{}}\nif ({second}) {{}} else {{}}"
        );
        assert_eq!(coverage_hits(source_id, &source), 2);
    }

    /// A one-line statement with a known number of blocks.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Branching {
        If,
        IfElse,
        Ternary,
        For,
        ForIn,
        ForOf,
        /// Not instrumented.
        While,
        Switch {
            cases: usize,
        },
        /// A branch in an anonymous function, which belongs to the
        /// enclosing named one.
        Callback,
        ArrowCallback,
    }

    impl Branching {
        const ALL: [Branching; 13] = [
            Branching::If,
            Branching::IfElse,
            Branching::Ternary,
            Branching::For,
            Branching::ForIn,
            Branching::ForOf,
            Branching::While,
            Branching::Switch { cases: 0 },
            Branching::Switch { cases: 1 },
            Branching::Switch { cases: 2 },
            Branching::Switch { cases: 3 },
            Branching::Callback,
            Branching::ArrowCallback,
        ];

        fn source(self) -> String {
            match self {
                Branching::If => "if (x) {}".into(),
                Branching::IfElse => "if (x) {} else {}".into(),
                Branching::Ternary => "x ? 1 : 2;".into(),
                Branching::For => "for (; x; ) {}".into(),
                Branching::ForIn => "for (const k in x) {}".into(),
                Branching::ForOf => "for (const v of x) {}".into(),
                Branching::While => "while (x) {}".into(),
                Branching::Switch { cases } => format!(
                    "switch (x) {{ {} }}",
                    (0..cases)
                        .map(|case| format!("case {case}: break;"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                Branching::Callback => {
                    "[].forEach(function () { if (x) {} });".into()
                }
                Branching::ArrowCallback => {
                    "[].forEach(() => { if (x) {} });".into()
                }
            }
        }

        fn blocks_count(self) -> usize {
            match self {
                Branching::If
                | Branching::IfElse
                | Branching::Ternary
                | Branching::Callback
                | Branching::ArrowCallback => 2,
                Branching::For | Branching::ForIn | Branching::ForOf => 1,
                Branching::While => 0,
                Branching::Switch { cases } => cases,
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
                    path,
                ),
                Form::Assignment => {
                    (format!("app.{name} = function () {{"), "};", path)
                }
            }
        }
    }

    /// Top-level statements followed by distinctly named functions, one
    /// statement per line.
    #[derive(Clone, Debug)]
    struct Program {
        top: Vec<Branching>,
        functions: Vec<(Form, Vec<Branching>)>,
    }

    impl Program {
        /// The source, and the function path and line of every block the
        /// instrumentation should find in it, in source order.
        fn render(&self) -> (String, Vec<(String, u32)>) {
            let mut lines: Vec<String> = Vec::new();
            let mut expected = Vec::new();
            let mut body =
                |lines: &mut Vec<String>,
                 path: &str,
                 statements: &[Branching]| {
                    for statement in statements {
                        lines.push(statement.source());
                        for _ in 0..statement.blocks_count() {
                            expected
                                .push((path.to_string(), lines.len() as u32));
                        }
                    }
                };
            body(&mut lines, "<top>", &self.top);
            for (index, (form, statements)) in self.functions.iter().enumerate()
            {
                let (before, after, path) = form.wrap(&format!("f{index}"));
                lines.push(before);
                body(&mut lines, &path, statements);
                lines.push(after.to_string());
            }
            (lines.join("\n"), expected)
        }

        fn blocks(&self) -> Vec<Block> {
            instrument(&self.render().0, SourceType::unambiguous()).blocks
        }
    }

    #[hegel::composite]
    fn bodies(tc: &TestCase) -> Vec<Branching> {
        tc.draw_silent(
            vecs(sampled_from(Branching::ALL.as_slice())).max_size(4),
        )
    }

    #[hegel::composite]
    fn programs(tc: &TestCase) -> Program {
        let function = tuples!(sampled_from(Form::ALL.as_slice()), bodies());
        Program {
            top: tc.draw_silent(bodies()),
            functions: tc.draw_silent(vecs(function).max_size(4)),
        }
    }

    fn ids_by_function(blocks: &[Block]) -> BTreeMap<String, Vec<u32>> {
        let mut ids: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        for block in blocks {
            ids.entry(block.function.clone())
                .or_default()
                .push(block.id);
        }
        ids
    }

    #[hegel::test]
    fn block_table_matches_the_program(tc: TestCase) {
        let program = tc.draw(programs().print_as_debug());
        let (source, mut expected) = program.render();
        tc.note(&source);
        let instrumented = instrument(&source, SourceType::unambiguous());

        let mut found: Vec<(String, u32)> = instrumented
            .blocks
            .iter()
            .map(|block| (block.function.clone(), block.line))
            .collect();
        found.sort();
        expected.sort();
        assert_eq!(found, expected);

        for block in &instrumented.blocks {
            assert_eq!(block.script, "/test.js");
            assert!(block.id < 1 << BLOCK_ID_BITS);
        }
    }

    #[hegel::test]
    fn block_table_ids_are_the_ids_in_the_code(tc: TestCase) {
        let program = tc.draw(programs().print_as_debug());
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

    #[hegel::test]
    fn block_ids_survive_edits_in_other_functions(tc: TestCase) {
        let before = tc.draw(programs().print_as_debug());
        // The edited body: a function's, or the top level's.
        let edited =
            tc.draw(integers::<usize>().max_value(before.functions.len()));
        let body = tc.draw(bodies().print_as_debug());

        let mut after = before.clone();
        let path = match after.functions.get_mut(edited) {
            Some((form, statements)) => {
                *statements = body;
                form.wrap(&format!("f{edited}")).2
            }
            None => {
                after.top = body;
                "<top>".to_string()
            }
        };

        let mut ids_before = ids_by_function(&before.blocks());
        let mut ids_after = ids_by_function(&after.blocks());
        ids_before.remove(&path);
        ids_after.remove(&path);
        assert_eq!(ids_before, ids_after);
    }

    #[hegel::test]
    fn block_ids_survive_reordering_functions(tc: TestCase) {
        let before = tc.draw(programs().print_as_debug());
        let ids_before = ids_by_function(&before.blocks());

        // Reverse the functions in the source, keeping their names.
        let (source, _) = before.render();
        let top_lines = before.top.len();
        let lines: Vec<&str> = source.lines().collect();
        let (top, functions) = lines.split_at(top_lines);
        let mut chunks: Vec<&[&str]> = Vec::new();
        let mut rest = functions;
        for (_, statements) in &before.functions {
            let (chunk, tail) = rest.split_at(statements.len() + 2);
            chunks.push(chunk);
            rest = tail;
        }
        chunks.reverse();
        let reordered: Vec<&str> = top
            .iter()
            .copied()
            .chain(chunks.into_iter().flatten().copied())
            .collect();
        let after =
            instrument(&reordered.join("\n"), SourceType::unambiguous()).blocks;

        assert_eq!(ids_before, ids_by_function(&after));
    }

    #[hegel::test]
    fn block_ids_differ_between_scripts(tc: TestCase) {
        let program = tc.draw(programs().print_as_debug());
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
    fn hits_record_the_blocks_run() {
        let source =
            "function f(a) { if (a) { return 1; } return 2; }\nf(true);";
        let Instrumented { text: code, blocks } =
            instrument(source, SourceType::cjs());
        let ids: Vec<u32> = blocks.iter().map(|block| block.id).collect();
        let mut context = Context::default();
        context
            .eval(Source::from_bytes("var window = globalThis;"))
            .unwrap();
        context.eval(Source::from_bytes(&code)).unwrap();
        let hits = context
            .eval(Source::from_bytes(
                "JSON.stringify(Array.from(__bombadil__.hits))",
            ))
            .unwrap()
            .to_string(&mut context)
            .unwrap()
            .to_std_string_escaped();
        // The `if` consequent ran; its (empty) else branch did not.
        assert_eq!(hits, format!("[{}]", ids[0]));
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
