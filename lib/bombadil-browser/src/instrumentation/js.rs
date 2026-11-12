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
    Block, BlockIdPrefix, Instrumented,
    source_id::SourceId,
    source_map::{Original, SourceMap},
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
/// The id last added to the hits, per slot (the id modulo the size), or -1.
/// A block that finds its own id there skips the set: a loop pays a typed
/// array read per iteration instead of a hash insert. Whoever clears the
/// hits has to fill this with -1 as well.
pub const HITS_CACHE: &str = "hits_cache";
pub const HITS_CACHE_SIZE: usize = 64 * 1024;

const LOCATION_PREVIOUS: &str = "previous";

const PRELUDE: &str = formatcp!(
    "window.{NAMESPACE} = window.{NAMESPACE} || {{
        {EDGES_PREVIOUS}: new Uint8Array({EDGE_MAP_SIZE}),
        {EDGES_CURRENT}: new Uint8Array({EDGE_MAP_SIZE}),
        {LOCATION_PREVIOUS}: 0,
        {HITS}: new Set(),
        {HITS_CACHE}: new Int32Array({HITS_CACHE_SIZE}).fill(-1),
    }};"
);

/// With a `source_map`, a block that the map resolves to an original source
/// gets its ID from that source and the original names of its functions
/// instead of from `source_id` and the names in `source_text`.
pub fn instrument_source_code(
    source_id: SourceId,
    source_text: &str,
    source_type: SourceType,
    source_map: Option<&SourceMap>,
) -> InstrumentationResult<Instrumented> {
    let allocator = Allocator::default();
    let mut program = parse(&allocator, source_text, source_type)?;
    let blocks = instrument_program(
        &allocator,
        &mut program,
        source_id,
        source_text,
        source_map,
    )?;

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
    source_text: &'a str,
    source_map: Option<&'a SourceMap>,
) -> InstrumentationResult<Vec<Block>> {
    let semantic = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(program);

    if !semantic.errors.is_empty() {
        let errors = semantic.errors.to_vec();
        return Err(InstrumentationError::SemanticErrors(errors));
    }
    let scopes = semantic.semantic.into_scoping();
    let top: Arc<str> = TOP.into();
    let mut instrumenter = Instrumenter {
        positions: Positions::new(source_text),
        functions: Functions::new(source_id.0, top.clone()),
        originals: source_map.map(|source_map| Originals {
            source_map,
            sources: std::iter::repeat_with(|| None)
                .take(source_map.sources_count())
                .collect(),
            top,
        }),
        scopes: vec![Scope {
            function: 0,
            original: None,
        }],
        name: Name::default(),
        path: String::new(),
        blocks: Vec::new(),
    };
    traverse_mut(&mut instrumenter, allocator, program, scopes, ());

    Ok(instrumenter.blocks)
}

const TOP: &str = "<top>";

/// Turns byte offsets in a script into lines and columns.
struct Positions<'s> {
    source_text: &'s [u8],
    line_starts: Vec<u32>,
    /// The [`surplus`] of the text before every `SURPLUS_INTERVAL` bytes
    /// and of all of it, or nothing for ASCII text. A line of a minified
    /// bundle is too long to count from its start for every position.
    surpluses: Vec<u32>,
}

const SURPLUS_INTERVAL: usize = 64;

/// How many more bytes than UTF-16 units the characters of `bytes` have:
/// one for each continuation byte, less one for a character of four bytes
/// (two units). It wraps, as `bytes` may start or end inside a character;
/// the surpluses of adjacent slices still add up.
fn surplus(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0, |surplus: u32, &byte| {
        let continuation = u32::from(byte & 0xC0 == 0x80);
        let four_bytes = u32::from(byte >= 0xF0);
        surplus.wrapping_add(continuation).wrapping_sub(four_bytes)
    })
}

impl<'s> Positions<'s> {
    fn new(source_text: &'s str) -> Self {
        let mut surpluses = Vec::new();
        if !source_text.is_ascii() {
            let chunks = source_text.as_bytes().chunks(SURPLUS_INTERVAL);
            surpluses.reserve_exact(chunks.len() + 1);
            let mut before = 0u32;
            for chunk in chunks {
                surpluses.push(before);
                before = before.wrapping_add(surplus(chunk));
            }
            surpluses.push(before);
        }
        Positions {
            source_text: source_text.as_bytes(),
            line_starts: line_starts(source_text),
            surpluses,
        }
    }

    fn surplus_before(&self, offset: usize) -> u32 {
        let chunk = offset / SURPLUS_INTERVAL;
        let counted = chunk * SURPLUS_INTERVAL;
        self.surpluses[chunk]
            .wrapping_add(surplus(&self.source_text[counted..offset]))
    }

    /// The 1-based line and 0-based column (UTF-16 units) of `offset`.
    fn line_column(&self, offset: u32) -> (u32, u32) {
        let line = self.line_starts.partition_point(|&start| start <= offset);
        let start = self.line_starts[line - 1];
        let (from, to) = (start as usize, offset as usize);
        let surplus = if self.surpluses.is_empty() {
            0
        } else if to - from <= SURPLUS_INTERVAL {
            surplus(&self.source_text[from..to])
        } else {
            self.surplus_before(to)
                .wrapping_sub(self.surplus_before(from))
        };
        (line as u32, offset - start - surplus)
    }
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

struct Instrumenter<'m> {
    positions: Positions<'m>,
    /// The functions of the script, by their names in it.
    functions: Functions,
    originals: Option<Originals<'m>>,
    /// Each enclosing class or function, the innermost last. An anonymous
    /// one has the entries of its parent.
    scopes: Vec<Scope>,
    name: Name,
    /// Scratch space for a function path that is looked up.
    path: String,
    blocks: Vec<Block>,
}

#[derive(Clone, Copy)]
struct Scope {
    /// Entry in the script's functions.
    function: usize,
    /// With a source map: the original source of the innermost named scope
    /// that the map resolves, and its entry in that source's functions.
    original: Option<(u32, usize)>,
}

struct Function {
    path: Arc<str>,
    id_prefix: BlockIdPrefix,
    blocks_count: u32,
}

/// The functions of a script or of an original source, one per function
/// path, `<top>` first. Functions with the same path share an entry.
struct Functions {
    /// What the block IDs are made from: the script's source ID or the
    /// source's key.
    key: Arc<str>,
    functions: Vec<Function>,
    /// Without `<top>`.
    indexes: HashMap<Arc<str>, usize>,
}

impl Functions {
    fn new(key: Arc<str>, top: Arc<str>) -> Self {
        let top = Function {
            id_prefix: BlockIdPrefix::new(&key, &top),
            path: top,
            blocks_count: 0,
        };
        Functions {
            key,
            functions: vec![top],
            indexes: HashMap::new(),
        }
    }

    /// The entry of the function called `name` in the function at `parent`.
    fn child(&mut self, parent: usize, name: &str, path: &mut String) -> usize {
        let path: &str = match parent {
            0 => name,
            _ => {
                path.clear();
                path.push_str(&self.functions[parent].path);
                path.push('.');
                path.push_str(name);
                path
            }
        };
        if path == TOP {
            return 0;
        }
        if let Some(&index) = self.indexes.get(path) {
            return index;
        }
        let path: Arc<str> = path.into();
        let index = self.functions.len();
        self.indexes.insert(path.clone(), index);
        self.functions.push(Function {
            id_prefix: BlockIdPrefix::new(&self.key, &path),
            path,
            blocks_count: 0,
        });
        index
    }
}

/// The functions of the original sources of a script with a source map.
struct Originals<'m> {
    source_map: &'m SourceMap,
    /// By `Original::source_index`, made when a source is first seen.
    sources: Vec<Option<Functions>>,
    top: Arc<str>,
}

impl Originals<'_> {
    fn functions(&mut self, original: &Original) -> &mut Functions {
        self.sources[original.source_index as usize].get_or_insert_with(|| {
            Functions::new(original.source.clone(), self.top.clone())
        })
    }
}

/// The name of a class or function: as it is in the script and, when the
/// source map has names, as it was in the original source.
#[derive(Default)]
struct Name {
    script: String,
    original: String,
}

struct NameWriter<'i> {
    name: &'i mut Name,
    /// With a source map that has names.
    originals: Option<(&'i SourceMap, &'i Positions<'i>)>,
}

impl NameWriter<'_> {
    fn text(&mut self, text: &str) {
        self.name.script.push_str(text);
        if self.originals.is_some() {
            self.name.original.push_str(text);
        }
    }

    fn identifier(&mut self, span: Span, name: &str) {
        self.name.script.push_str(name);
        if let Some((source_map, positions)) = self.originals {
            let (line, column) = positions.line_column(span.start);
            let original = source_map.name(line, column).unwrap_or(name);
            self.name.original.push_str(original);
        }
    }

    fn mark(&self) -> (usize, usize) {
        (self.name.script.len(), self.name.original.len())
    }

    fn reset(&mut self, (script, original): (usize, usize)) {
        self.name.script.truncate(script);
        self.name.original.truncate(original);
    }
}

// The functions below write a name and say whether there is one. Without
// one, they leave what was written before them.

fn key_name(key: &ast::PropertyKey<'_>, out: &mut NameWriter) -> bool {
    match key {
        ast::PropertyKey::PrivateIdentifier(identifier) => {
            out.text("#");
            out.text(&identifier.name);
        }
        ast::PropertyKey::StaticIdentifier(identifier) => {
            out.identifier(identifier.span, &identifier.name);
        }
        _ => match key.static_name() {
            Some(name) => out.text(&name),
            None => return false,
        },
    }
    true
}

/// `a.b.c` for static member chains rooted at an identifier or `this`.
fn member_path(expression: &Expression<'_>, out: &mut NameWriter) -> bool {
    match expression {
        Expression::Identifier(identifier) => {
            out.identifier(identifier.span, &identifier.name);
        }
        Expression::ThisExpression(_) => out.text("this"),
        Expression::StaticMemberExpression(member) => {
            if !member_path(&member.object, out) {
                return false;
            }
            out.text(".");
            out.identifier(member.property.span, &member.property.name);
        }
        _ => return false,
    }
    true
}

fn target_name(target: &AssignmentTarget<'_>, out: &mut NameWriter) -> bool {
    match target {
        AssignmentTarget::StaticMemberExpression(member) => {
            if member_path(&member.object, out) {
                out.text(".");
            }
            out.identifier(member.property.span, &member.property.name);
        }
        AssignmentTarget::AssignmentTargetIdentifier(identifier) => {
            out.identifier(identifier.span, &identifier.name);
        }
        _ => match target.get_identifier_name() {
            Some(name) => out.text(name),
            None => return false,
        },
    }
    true
}

/// The name a function or class expression gets from its position.
/// `ancestors` starts at its parent. Object literal values are prefixed
/// with the object's name.
fn name_from_ancestors<'a, 't>(
    mut ancestors: impl Iterator<Item = Ancestor<'a, 't>>,
    out: &mut NameWriter,
) -> bool {
    let mark = out.mark();
    let named = match ancestors.next() {
        Some(Ancestor::VariableDeclaratorInit(declarator)) => {
            match declarator.id().get_binding_identifier() {
                Some(identifier) => {
                    out.identifier(identifier.span, &identifier.name);
                    true
                }
                None => false,
            }
        }
        Some(Ancestor::MethodDefinitionValue(method)) => {
            match method.kind() {
                ast::MethodDefinitionKind::Get => out.text("get "),
                ast::MethodDefinitionKind::Set => out.text("set "),
                _ => {}
            }
            key_name(method.key(), out)
        }
        Some(Ancestor::ObjectPropertyValue(property)) => {
            let in_object = matches!(
                ancestors.next(),
                Some(Ancestor::ObjectExpressionProperties(_))
            );
            if in_object && name_from_ancestors(ancestors, out) {
                out.text(".");
            }
            match property.kind() {
                ast::PropertyKind::Get => out.text("get "),
                ast::PropertyKind::Set => out.text("set "),
                ast::PropertyKind::Init => {}
            }
            key_name(property.key(), out)
        }
        Some(Ancestor::PropertyDefinitionValue(property)) => {
            key_name(property.key(), out)
        }
        Some(Ancestor::AssignmentExpressionRight(assignment)) => {
            target_name(assignment.left(), out)
        }
        _ => false,
    };
    if !named {
        out.reset(mark);
    }
    named
}

fn declared_name(
    identifier: Option<&ast::BindingIdentifier<'_>>,
    out: &mut NameWriter,
) -> bool {
    match identifier {
        Some(identifier) => {
            out.identifier(identifier.span, &identifier.name);
            true
        }
        None => false,
    }
}

impl Instrumenter<'_> {
    /// Enters the class or function at the offset `start` that `name`
    /// names. In the original source its path starts over when the map
    /// resolves it to another source than its parent, and it adds no name
    /// when the map does not resolve it: what a bundler wraps a module in
    /// is not part of the module's function paths.
    fn enter_scope(
        &mut self,
        start: u32,
        name: impl FnOnce(&mut NameWriter) -> bool,
    ) {
        let parent = *self.scopes.last().expect("no <top> scope");
        self.name.script.clear();
        self.name.original.clear();
        let source_map_with_names = self
            .originals
            .as_ref()
            .map(|originals| originals.source_map)
            .filter(|source_map| source_map.has_names());
        let named = name(&mut NameWriter {
            name: &mut self.name,
            originals: source_map_with_names
                .map(|source_map| (source_map, &self.positions)),
        });
        if !named {
            self.scopes.push(parent);
            return;
        }
        let function = self.functions.child(
            parent.function,
            &self.name.script,
            &mut self.path,
        );
        let mut original = parent.original;
        if let Some(originals) = &mut self.originals {
            let (line, column) = self.positions.line_column(start);
            if let Some(position) = originals.source_map.lookup(line, column) {
                let source = position.source_index;
                let parent = match parent.original {
                    Some((parent_source, index)) if parent_source == source => {
                        index
                    }
                    _ => 0,
                };
                let name = match source_map_with_names {
                    Some(_) => &self.name.original,
                    None => &self.name.script,
                };
                let functions = originals.functions(&position);
                let index = functions.child(parent, name, &mut self.path);
                original = Some((source, index));
            }
        }
        self.scopes.push(Scope { function, original });
    }

    fn exit_scope(&mut self) {
        self.scopes.pop();
    }

    fn new_block(&mut self, span: Span) -> u32 {
        let scope = *self.scopes.last().expect("no <top> scope");
        let (line, column) = self.positions.line_column(span.start);
        let function = &mut self.functions.functions[scope.function];
        let ordinal = function.blocks_count;
        function.blocks_count += 1;
        let script = self.functions.key.clone();
        let resolved = self.originals.as_mut().and_then(|originals| {
            let original = originals.source_map.lookup(line, column)?;
            Some((original, originals.functions(&original)))
        });
        let block = match resolved {
            None => Block {
                id: function.id_prefix.id(ordinal),
                script,
                function: function.path.clone(),
                line,
                column,
                source: None,
                source_line: None,
                vendor: false,
            },
            Some((original, functions)) => {
                // A block from another source than its function (inlined
                // code, or a module in a wrapper) is at the top of its own.
                let index = match scope.original {
                    Some((source, index))
                        if source == original.source_index =>
                    {
                        index
                    }
                    _ => 0,
                };
                let function = &mut functions.functions[index];
                let id = function.id_prefix.id(function.blocks_count);
                function.blocks_count += 1;
                Block {
                    id,
                    script,
                    function: function.path.clone(),
                    line,
                    column,
                    source: Some(original.source.clone()),
                    source_line: Some(original.line),
                    vendor: original.vendor,
                }
            }
        };
        let id = block.id;
        self.blocks.push(block);
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

        let hits_cache_slot = || {
            ctx.ast.alloc_computed_member_expression(
                SPAN,
                antithesis_member(HITS_CACHE),
                ctx.ast.expression_numeric_literal(
                    SPAN,
                    (id as usize % HITS_CACHE_SIZE) as f64,
                    None,
                    ast::NumberBase::Decimal,
                ),
                false,
            )
        };
        let hit_add = ctx.ast.expression_call(
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
        let hit = ctx.ast.expression_logical(
            SPAN,
            ctx.ast.expression_binary(
                SPAN,
                Expression::ComputedMemberExpression(hits_cache_slot()),
                ast::BinaryOperator::StrictEquality,
                branch_id.clone_in_with_semantic_ids(ctx.ast.allocator),
            ),
            ast::LogicalOperator::Or,
            ctx.ast.expression_sequence(
                SPAN,
                ctx.ast.vec_from_array([
                    ctx.ast.expression_assignment(
                        SPAN,
                        AssignmentOperator::Assign,
                        AssignmentTarget::ComputedMemberExpression(
                            hits_cache_slot(),
                        ),
                        branch_id.clone_in_with_semantic_ids(ctx.ast.allocator),
                    ),
                    hit_add,
                ]),
            ),
        );

        ctx.ast
            .vec_from_array([edge_addition, location_previous_update, hit])
    }

    /// Adds the following three statements to the start of block, or wraps a single statement
    /// in a block with these two at the start:
    ///
    /// ```not_rust
    /// __bombadil__.edges_current[((<id> ^ __bombadil__.previous) >>> 0) % 65536] += 1;
    /// __bombadil__.previous = <id> >> 1;
    /// __bombadil__.hits_cache[<id> % 65536] === <id>
    ///     || (__bombadil__.hits_cache[<id> % 65536] = <id>, __bombadil__.hits.add(<id>));
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

impl<'a> Traverse<'a, ()> for Instrumenter<'_> {
    fn enter_function(
        &mut self,
        function: &mut ast::Function<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.enter_scope(function.span.start, |out| {
            declared_name(function.id.as_ref(), out)
                || name_from_ancestors(ctx.ancestors(), out)
        });
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
        function: &mut ast::ArrowFunctionExpression<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.enter_scope(function.span.start, |out| {
            name_from_ancestors(ctx.ancestors(), out)
        });
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
        self.enter_scope(class.span.start, |out| {
            declared_name(class.id.as_ref(), out)
                || name_from_ancestors(ctx.ancestors(), out)
        });
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
    use crate::instrumentation::block_id;
    use crate::instrumentation::source_map::tests::{MapModel, SegmentModel};
    use boa_engine::{Context, Source, object::builtins::JsUint8Array};
    use hegel::{
        Generator, TestCase,
        generators::{
            booleans, integers, permutations, sampled_from, text, tuples2,
            tuples3, vecs,
        },
    };
    use insta::assert_snapshot;
    use oxc::span::ContentEq;
    use std::collections::{BTreeMap, BTreeSet};
    use url::Url;

    fn instrument(source: &str, source_type: SourceType) -> Instrumented {
        instrument_source_code(
            SourceId("/test.js".into()),
            source,
            source_type,
            None,
        )
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
                // The code has a large ID as JavaScript prints it: 1105401e3.
                let number = rest.split(' ').next().unwrap();
                number.parse::<f64>().expect(number) as u32
            })
            .collect()
    }

    fn coverage_hits(source_id: SourceId, source: &str) -> u32 {
        let code =
            instrument_source_code(source_id, source, SourceType::cjs(), None)
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

    /// Line and UTF-16 column of the end of `prefix`, one character at a
    /// time.
    fn position_naive(prefix: &str) -> (u32, u32) {
        let (mut line, mut column) = (1, 0);
        let mut characters = prefix.chars().peekable();
        while let Some(character) = characters.next() {
            match character {
                '\r' => {
                    characters.next_if_eq(&'\n');
                    (line, column) = (line + 1, 0);
                }
                '\n' | '\u{2028}' | '\u{2029}' => {
                    (line, column) = (line + 1, 0);
                }
                _ => column += character.len_utf16() as u32,
            }
        }
        (line, column)
    }

    #[hegel::test]
    fn block_columns_count_utf16_units_from_the_line_start(tc: TestCase) {
        let statements = tc.draw(
            vecs(tuples2(
                text(),
                sampled_from(vec![" ", "\n", "\r", "\r\n", "\u{2028}"]),
            ))
            .max_size(6),
        );
        let mut source = String::new();
        let mut expected = Vec::new();
        for (literal, separator) in statements {
            source.push_str(&serde_json::to_string(&literal).unwrap());
            // The inner branches get their blocks before the outer ones.
            for part in ["; c ? ", "d ? ", "1 : ", "2 : "] {
                source.push_str(part);
                expected.push(position_naive(&source));
            }
            source.push_str("3;");
            source.push_str(separator);
        }
        tc.note(&format!("{source:?}"));
        let mut found: Vec<(u32, u32)> =
            instrument(&source, SourceType::unambiguous())
                .blocks
                .into_iter()
                .map(|block| (block.line, block.column))
                .collect();
        found.sort_unstable();
        assert_eq!(found, expected);
    }

    #[hegel::test]
    fn positions_count_utf16_units_however_long_the_line(tc: TestCase) {
        // Any text between runs of one character that are long enough to
        // be counted an interval at a time.
        let pieces = tc.draw(
            vecs(tuples3(
                text(),
                sampled_from(vec!["a", "é", "日", "𝄞", "\n"]),
                integers::<usize>().max_value(2 * SURPLUS_INTERVAL),
            ))
            .max_size(4),
        );
        let mut source = String::new();
        for (text, character, count) in pieces {
            source.push_str(&text);
            source.push_str(&character.repeat(count));
        }
        // The start of a character, and not inside a CRLF.
        let mut offset = tc.draw(integers::<usize>().max_value(source.len()));
        while !source.is_char_boundary(offset)
            || (source[..offset].ends_with('\r')
                && source[offset..].starts_with('\n'))
        {
            offset -= 1;
        }
        assert_eq!(
            Positions::new(&source).line_column(offset as u32),
            position_naive(&source[..offset])
        );
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
                None,
            )
            .unwrap()
            .blocks
            .into_iter()
            .map(|block| block.id)
            .collect()
        };
        assert!(ids("/a.js").is_disjoint(&ids("/b.js")));
    }

    /// The path, line and ID of every block of the program in a script or
    /// source called `key`, in the order the instrumentation adds them.
    fn blocks_keyed(key: &str, program: &Program) -> Vec<(String, u32, u32)> {
        blocks_keyed_with(key, program, str::to_string)
    }

    /// As `blocks_keyed`, with `path` applied to each function path.
    fn blocks_keyed_with(
        key: &str,
        program: &Program,
        path: impl Fn(&str) -> String,
    ) -> Vec<(String, u32, u32)> {
        let mut ordinals: BTreeMap<String, u32> = BTreeMap::new();
        let blocks = program.render().1.into_iter();
        blocks
            .map(|(function, line)| {
                let path = path(&function);
                let ordinal = ordinals.entry(path.clone()).or_default();
                let id = block_id(key, &path, *ordinal);
                *ordinal += 1;
                (path, line, id)
            })
            .collect()
    }

    #[hegel::test]
    fn block_ids_without_a_map_are_made_from_the_script_and_function(
        tc: TestCase,
    ) {
        let program = tc.draw(programs(false).print_as_debug());
        let found: Vec<(String, u32, u32)> = program
            .blocks()
            .into_iter()
            .map(|block| (block.function.to_string(), block.line, block.id))
            .collect();
        assert_eq!(found, blocks_keyed("/test.js", &program));
    }

    /// A source file of a bundle.
    #[derive(Clone, Debug)]
    struct Module {
        source: String,
        program: Program,
        /// Whether the bundle's map says where its lines come from.
        mapped: bool,
        /// Whether the map has the source on an ignore list.
        vendor: bool,
    }

    const DIRECTORIES: [&str; 3] = ["", "src/", "node_modules/pkg/"];

    /// Modules with different sources.
    #[hegel::composite]
    fn modules(tc: &TestCase, mapped_all: bool) -> Vec<Module> {
        let count =
            tc.draw_silent(integers::<usize>().min_value(1).max_value(3));
        (0..count)
            .map(|index| {
                let directory =
                    tc.draw_silent(sampled_from(DIRECTORIES.as_slice()));
                Module {
                    source: format!("{directory}m{index}.js"),
                    program: tc.draw_silent(programs(false)),
                    mapped: mapped_all || tc.draw_silent(booleans()),
                    vendor: tc.draw_silent(booleans()),
                }
            })
            .collect()
    }

    /// How a bundle and its source map are made from modules.
    #[derive(Debug)]
    struct Bundler {
        /// What the map's sources have before their keys, module by module
        /// and around again.
        source_prefixes: Vec<&'static str>,
        /// The identifiers that get another name in the bundle.
        renames: Vec<Rename>,
        /// Whether the name in the bundle of an identifier that the map
        /// names is in upper case, line by line and around again: a
        /// function path in the bundle is not one in the source.
        renames_upper: Vec<bool>,
        /// Whether a vendor source is on the ignore list of the standard.
        vendors_standard: bool,
        /// Whether the lines of a module that is not mapped have a source
        /// that the map lists as `null`, instead of none.
        sources_null: bool,
        /// Whether the bundle is one line.
        one_line: bool,
        /// What each module is in, module by module and around again.
        wrappers: Vec<Wrapper>,
        /// Whether a wrapper in a bundle of one line has a segment without
        /// a source. Without one, the map resolves the wrapper to where
        /// the code before it comes from.
        wrappers_marked: bool,
        /// The original column of each segment, around again.
        columns_original: Vec<u32>,
        /// A comment at the start of each line, line by line and around
        /// again: where a column in UTF-16 units is not one in bytes.
        comments: Vec<&'static str>,
    }

    const COMMENTS: [&str; 4] = ["", "/*é*/", "/*日本*/", "/*𝄞*/"];

    #[derive(Clone, Copy, Debug)]
    struct Rename {
        original: &'static str,
        minified: &'static str,
        /// Whether the map has the original name where the identifier is.
        named: bool,
    }

    /// What a bundler puts around a module. The map does not cover it.
    #[derive(Clone, Copy, Debug)]
    enum Wrapper {
        /// A function without a name.
        Anonymous,
        /// A function in a table of modules, under the key the bundler
        /// gave the module.
        Keyed(&'static str),
    }

    impl Wrapper {
        const ALL: [Wrapper; 5] = [
            Wrapper::Anonymous,
            Wrapper::Keyed("12"),
            Wrapper::Keyed("345"),
            Wrapper::Keyed("main"),
            Wrapper::Keyed("\"./src/main.js\""),
        ];

        /// The lines before and after the module.
        fn lines(self) -> (String, &'static str) {
            match self {
                Wrapper::Anonymous => ("(() => {".to_string(), "})();"),
                Wrapper::Keyed(key) => {
                    (format!("var wrapper = {{ {key}: (p) => {{"), "} };")
                }
            }
        }
    }

    const SOURCE_PREFIXES: [&str; 5] =
        ["", "./", "../../", "/", "webpack://app/./"];
    /// Every identifier of a program that names a function.
    const RENAMED: [&str; 10] = [
        "f", "g", "h", "k", "object_f", "object_g", "object_h", "object_k",
        "app", "run",
    ];
    const MINIFIED: [&str; 10] =
        ["a", "b", "c", "d", "e", "i", "n", "o", "r", "t"];

    /// With `named_all`, the map names every identifier the bundle renames.
    #[hegel::composite]
    fn bundlers(tc: &TestCase, named_all: bool) -> Bundler {
        let minified = tc.draw_silent(permutations(MINIFIED.to_vec()));
        let renames = RENAMED
            .into_iter()
            .zip(minified)
            .filter(|_| tc.draw_silent(booleans()))
            .map(|(original, minified)| Rename {
                original,
                minified,
                named: named_all || tc.draw_silent(booleans()),
            })
            .collect();
        Bundler {
            source_prefixes: tc.draw_silent(
                vecs(sampled_from(SOURCE_PREFIXES.as_slice()))
                    .min_size(1)
                    .max_size(3),
            ),
            renames,
            renames_upper: tc
                .draw_silent(vecs(booleans()).min_size(1).max_size(5)),
            one_line: tc.draw_silent(booleans()),
            wrappers: tc.draw_silent(
                vecs(sampled_from(Wrapper::ALL.as_slice()))
                    .min_size(1)
                    .max_size(3),
            ),
            wrappers_marked: tc.draw_silent(booleans()),
            vendors_standard: tc.draw_silent(booleans()),
            sources_null: tc.draw_silent(booleans()),
            columns_original: tc
                .draw_silent(vecs(integers::<u32>()).min_size(1).max_size(5)),
            comments: tc.draw_silent(
                vecs(sampled_from(COMMENTS.as_slice()))
                    .min_size(1)
                    .max_size(5),
            ),
        }
    }

    impl Bundler {
        /// The modules one after the other, each in a wrapper of its own.
        /// A line that is not mapped (a wrapper's, or of a module that is
        /// not) has a segment without a source, or in a bundle of many
        /// lines no segment.
        fn bundle(&self, modules: &[Module]) -> (String, SourceMap) {
            self.bundle_at(modules, None)
        }

        /// As `bundle`, with the map at the URL `base`.
        fn bundle_at(
            &self,
            modules: &[Module],
            base: Option<&Url>,
        ) -> (String, SourceMap) {
            let mut text = String::new();
            let mut line_start = 0;
            let mut segments = Vec::new();
            let mut upper = self.renames_upper.iter().cycle();
            let mut columns_original = self.columns_original.iter().cycle();
            let mut comments = self.comments.iter().cycle();
            let mut model = MapModel {
                names: self
                    .renames
                    .iter()
                    .map(|rename| Some(rename.original.to_string()))
                    .collect(),
                vendors: modules.iter().map(|module| module.vendor).collect(),
                vendors_standard: vec![self.vendors_standard; modules.len()],
                ..MapModel::default()
            };
            for (index, module) in modules.iter().enumerate() {
                let prefix =
                    self.source_prefixes[index % self.source_prefixes.len()];
                let listed = module.mapped || !self.sources_null;
                model
                    .sources
                    .push(listed.then(|| format!("{prefix}{}", module.source)));
                let source = module.program.render().0;
                let lines = source.lines().enumerate().map(|(line, text)| {
                    let original = (module.mapped || self.sources_null)
                        .then_some((index as u32, line as u32));
                    (original, text, false)
                });
                let (before, after) =
                    self.wrappers[index % self.wrappers.len()].lines();
                let lines = std::iter::once((None, before.as_str(), true))
                    .chain(lines)
                    .chain([(None, after, true)]);
                for (original, mut rest, wrapper) in lines {
                    let upper = *upper.next().unwrap();
                    if !text.is_empty() && self.one_line {
                        text.push(' ');
                    } else if !text.is_empty() {
                        text.push('\n');
                        line_start = text.len();
                        model.lines.push(std::mem::take(&mut segments));
                    }
                    text.push_str(comments.next().unwrap());
                    let mut segment = |text: &str, name: Option<usize>| {
                        let column = text[line_start..].encode_utf16().count();
                        segments.push(SegmentModel {
                            column: column as u32,
                            original,
                            column_original: *columns_original.next().unwrap(),
                            name: name.map(|name| name as u32),
                        });
                    };
                    let marked = !wrapper || self.wrappers_marked;
                    if original.is_some() || (self.one_line && marked) {
                        segment(&text, None);
                    }
                    while !rest.is_empty() {
                        let length = rest
                            .bytes()
                            .take_while(|byte| {
                                byte.is_ascii_alphanumeric() || *byte == b'_'
                            })
                            .count();
                        let (word, after) = rest.split_at(length.max(1));
                        let rename = self
                            .renames
                            .iter()
                            .position(|rename| rename.original == word);
                        match rename.map(|index| (index, self.renames[index])) {
                            Some((index, rename)) => {
                                if original.is_some() && rename.named {
                                    segment(&text, Some(index));
                                }
                                match rename.named && upper {
                                    true => text.push_str(
                                        &rename.minified.to_uppercase(),
                                    ),
                                    false => text.push_str(rename.minified),
                                }
                            }
                            None => text.push_str(word),
                        }
                        rest = after;
                    }
                }
            }
            model.lines.push(segments);
            (text, SourceMap::parse(&model.json(), base).unwrap())
        }

        /// A function path of a module as the map gives it back: with the
        /// bundle's name for an identifier that the map does not name.
        fn path_original(&self, path: &str) -> String {
            let mut original = String::new();
            for (index, identifier) in path.split('.').enumerate() {
                if index > 0 {
                    original.push('.');
                }
                let rename = self.renames.iter().find(|rename| {
                    rename.original == identifier && !rename.named
                });
                original.push_str(rename.map_or(identifier, |r| r.minified));
            }
            original
        }

        /// The path, source line and ID of every block of a mapped module,
        /// in the order the instrumentation adds them.
        fn blocks(&self, module: &Module) -> Vec<(String, u32, u32)> {
            blocks_keyed_with(&module.source, &module.program, |path| {
                self.path_original(path)
            })
        }
    }

    fn instrument_bundle(
        script: &str,
        text: &str,
        source_map: &SourceMap,
    ) -> Instrumented {
        instrument_source_code(
            SourceId(script.into()),
            text,
            SourceType::unambiguous(),
            Some(source_map),
        )
        .unwrap()
    }

    #[hegel::test(test_cases = 1000)]
    fn block_ids_with_a_map_are_made_from_the_source_and_original_function(
        tc: TestCase,
    ) {
        let modules = tc.draw(modules(true).print_as_debug());
        let bundler = tc.draw(bundlers(false).print_as_debug());
        let (text, source_map) = bundler.bundle(&modules);
        tc.note(&text);
        let found: Vec<(String, bool, String, u32, u32)> =
            instrument_bundle("/bundle.js", &text, &source_map)
                .blocks
                .into_iter()
                .map(|block| {
                    (
                        block.source.as_deref().unwrap_or_default().to_string(),
                        block.vendor,
                        block.function.to_string(),
                        block.source_line.unwrap_or_default(),
                        block.id,
                    )
                })
                .collect();
        let expected: Vec<(String, bool, String, u32, u32)> = modules
            .iter()
            .flat_map(|module| {
                bundler.blocks(module).into_iter().map(|(path, line, id)| {
                    (module.source.clone(), module.vendor, path, line, id)
                })
            })
            .collect();
        assert_eq!(found, expected);
    }

    fn ids_by_source_and_function(
        blocks: &[Block],
    ) -> BTreeMap<(String, String), Vec<u32>> {
        let mut ids: BTreeMap<_, Vec<u32>> = BTreeMap::new();
        for block in blocks {
            let source = block.source.as_deref().unwrap_or_default();
            ids.entry((source.to_string(), block.function.to_string()))
                .or_default()
                .push(block.id);
        }
        ids
    }

    #[hegel::test]
    fn block_ids_with_a_map_survive_another_bundle(tc: TestCase) {
        let modules = tc.draw(modules(true).print_as_debug());
        // Some of the modules, in any order.
        let mut chunk = tc.draw(permutations(modules.clone()).print_as_debug());
        chunk.truncate(tc.draw(integers::<usize>().max_value(chunk.len())));
        let ids = |script: &str, modules: &[Module]| {
            let bundler = tc.draw(bundlers(true).print_as_debug());
            let (text, source_map) = bundler.bundle(modules);
            tc.note(&text);
            let blocks = instrument_bundle(script, &text, &source_map).blocks;
            ids_by_source_and_function(&blocks)
        };
        let mut ids_main = ids("/main.js", &modules);
        ids_main.retain(|(source, _), _| {
            chunk.iter().any(|module| module.source == *source)
        });
        assert_eq!(ids_main, ids("/assets/chunk.js", &chunk));
    }

    #[hegel::test]
    fn block_ids_with_a_map_differ_between_sources(tc: TestCase) {
        let modules = tc.draw(modules(true).print_as_debug());
        let bundler = tc.draw(bundlers(false).print_as_debug());
        let ids = |modules: &[Module]| -> BTreeSet<u32> {
            let (text, source_map) = bundler.bundle(modules);
            instrument_bundle("/bundle.js", &text, &source_map)
                .blocks
                .into_iter()
                .map(|block| block.id)
                .collect()
        };
        let others: Vec<Module> = modules
            .iter()
            .map(|module| Module {
                source: format!("other/{}", module.source),
                ..module.clone()
            })
            .collect();
        assert!(ids(&modules).is_disjoint(&ids(&others)));
    }

    #[hegel::test]
    fn block_ids_with_a_map_differ_between_directories_of_relative_sources(
        tc: TestCase,
    ) {
        let modules = tc.draw(modules(true).print_as_debug());
        let mut bundler = tc.draw(bundlers(false).print_as_debug());
        bundler.source_prefixes = vec!["", "./"];
        let directories = tc.draw(permutations(vec!["a", "b", "a/b"]));
        let ids = |directory: &str| -> BTreeSet<u32> {
            let base = format!("https://example.com/{directory}/index.js.map");
            let (text, source_map) =
                bundler.bundle_at(&modules, Some(&Url::parse(&base).unwrap()));
            instrument_bundle("/bundle.js", &text, &source_map)
                .blocks
                .into_iter()
                .map(|block| block.id)
                .collect()
        };
        assert!(ids(directories[0]).is_disjoint(&ids(directories[1])));
    }

    #[hegel::test]
    fn blocks_the_map_does_not_resolve_are_as_without_a_map(tc: TestCase) {
        let modules = tc.draw(modules(false).print_as_debug());
        let bundler = tc.draw(bundlers(false).print_as_debug());
        let (text, source_map) = bundler.bundle(&modules);
        tc.note(&text);
        let mapped = instrument_bundle("/test.js", &text, &source_map).blocks;
        let plain = instrument(&text, SourceType::unambiguous()).blocks;
        assert_eq!(mapped.len(), plain.len());

        let mut blocks = mapped.iter().zip(&plain);
        for module in &modules {
            for (path, line, id) in bundler.blocks(module) {
                let (block, plain) = blocks.next().unwrap();
                match module.mapped {
                    true => assert_eq!(
                        (
                            block.source.as_deref(),
                            block.vendor,
                            &*block.function,
                            block.source_line,
                            block.id
                        ),
                        (
                            Some(&*module.source),
                            module.vendor,
                            &*path,
                            Some(line),
                            id
                        )
                    ),
                    false => assert_eq!(block, plain),
                }
            }
        }
        assert!(blocks.next().is_none());
    }

    #[hegel::test]
    fn a_bundle_the_map_resolves_nothing_of_is_as_without_a_map(tc: TestCase) {
        let mut modules = tc.draw(modules(false).print_as_debug());
        for module in &mut modules {
            module.mapped = false;
        }
        let bundler = tc.draw(bundlers(false).print_as_debug());
        let (text, source_map) = bundler.bundle(&modules);
        tc.note(&text);
        assert_eq!(
            instrument_bundle("/test.js", &text, &source_map),
            instrument(&text, SourceType::unambiguous())
        );
    }

    #[hegel::test(test_cases = 1000)]
    fn block_table_ids_with_a_map_are_the_ids_in_the_code(tc: TestCase) {
        // Sources that may have the same key, under different names.
        let mut modules = tc.draw(modules(false).print_as_debug());
        for module in &mut modules {
            module.source = tc.draw(sampled_from(vec!["a.js", "b.js"])).into();
        }
        let bundler = tc.draw(bundlers(false).print_as_debug());
        let (text, source_map) = bundler.bundle(&modules);
        tc.note(&text);
        let instrumented = instrument_bundle("/bundle.js", &text, &source_map);

        let table: BTreeSet<u32> =
            instrumented.blocks.iter().map(|block| block.id).collect();
        assert_eq!(table.len(), instrumented.blocks.len(), "duplicate ids");
        let hooks = hook_ids(&instrumented.text);
        assert_eq!(hooks.len(), instrumented.blocks.len());
        assert_eq!(hooks.into_iter().collect::<BTreeSet<u32>>(), table);
    }

    /// A bundle of a vendor module and an app module, minified, in the
    /// wrapper a bundler adds (which the map does not cover).
    #[test]
    fn test_instrument_source_code_with_source_map() {
        let bundle = concat!(
            r#"(()=>{var x=self.x?1:0;function t(n){return n>1?"clicks":"click"}"#,
            r#"var e={o:function(n){if(n){t(n)}}};e.o(x)})();"#,
        );
        let source_map = SourceMap::parse(
            &serde_json::json!({
                "version": 3,
                "file": "bundle.js",
                "sources": [
                    "webpack://app/./node_modules/label/index.js",
                    "webpack://app/./src/app.ts",
                ],
                "names": ["label", "count", "handlers", "onClick"],
                "mappings": "uBAAA,SAASA,EAAMC,GACb,4BCCF,IAAMC,GACJC,cACE,MACEH,QAGNE,M",
                "ignoreList": [0],
            })
            .to_string(),
            None,
        )
        .unwrap();
        assert_snapshot!(
            instrument_bundle("/bundle.js", bundle, &source_map).snapshot()
        );
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

    /// Which blocks run when a program runs as a script, by position in
    /// [`Rendered::blocks`]. Each read of `x` draws a value and records it
    /// in `reads`; after `READS_MAX` every read is `""`, which ends any
    /// loop. Functions and callbacks are never called, and no `case`
    /// matches a string.
    struct Execution<'t> {
        tc: &'t TestCase,
        reads: Vec<&'static str>,
        blocks_count: usize,
        blocks_run: BTreeSet<usize>,
    }

    impl Execution<'_> {
        const READS_MAX: usize = 32;

        fn read(&mut self) -> &'static str {
            if self.reads.len() == Self::READS_MAX {
                return "";
            }
            let read = self
                .tc
                .draw_silent(sampled_from(["", "a", "ab"].as_slice()));
            self.reads.push(read);
            read
        }

        fn block(&mut self, run: bool) {
            if run {
                self.blocks_run.insert(self.blocks_count);
            }
            self.blocks_count += 1;
        }

        fn statements(&mut self, statements: &[Statement], live: bool) {
            for statement in statements {
                self.statement(statement, live);
            }
        }

        /// Walks `body` once per iteration, or once dead if there is none,
        /// and says whether it ran.
        fn iterate(
            &mut self,
            body: &[Statement],
            mut next: impl FnMut(&mut Self) -> bool,
        ) -> bool {
            let blocks_count = self.blocks_count;
            let mut run = false;
            while next(self) {
                self.blocks_count = blocks_count;
                self.statements(body, true);
                run = true;
            }
            if !run {
                self.statements(body, false);
            }
            run
        }

        fn statement(&mut self, statement: &Statement, live: bool) {
            match statement {
                Statement::If(body) => {
                    let taken = live && !self.read().is_empty();
                    self.statements(body, taken);
                    self.block(taken);
                    self.block(live && !taken);
                }
                Statement::IfElse(consequent, alternate) => {
                    let taken = live && !self.read().is_empty();
                    self.statements(consequent, taken);
                    self.statements(alternate, live && !taken);
                    self.block(taken);
                    self.block(live && !taken);
                }
                Statement::Ternary => {
                    let taken = live && !self.read().is_empty();
                    self.block(taken);
                    self.block(live && !taken);
                }
                Statement::Loop(Loop::For, body) => {
                    let run = self.iterate(body, |execution| {
                        live && !execution.read().is_empty()
                    });
                    self.block(run);
                }
                Statement::Loop(Loop::ForIn | Loop::ForOf, body) => {
                    let mut iterations =
                        if live { self.read().len() } else { 0 };
                    let run = self.iterate(body, |_| {
                        let more = iterations > 0;
                        iterations = iterations.saturating_sub(1);
                        more
                    });
                    self.block(run);
                }
                Statement::While(body) => {
                    self.iterate(body, |execution| {
                        live && !execution.read().is_empty()
                    });
                }
                Statement::Switch(cases) => {
                    if live {
                        self.read();
                    }
                    for case in cases {
                        self.statements(case, false);
                        self.block(false);
                    }
                }
                Statement::Callback { body, .. }
                | Statement::Function { body, .. } => {
                    self.statements(body, false);
                }
            }
        }
    }

    #[hegel::test]
    fn hits_are_the_blocks_run(tc: TestCase) {
        let program = tc.draw(programs(false).print_as_debug());
        let (source, _) = program.render();
        tc.note(&source);
        let Instrumented { text: code, blocks } =
            instrument(&source, SourceType::cjs());

        let mut execution = Execution {
            tc: &tc,
            reads: Vec::new(),
            blocks_count: 0,
            blocks_run: BTreeSet::new(),
        };
        execution.statements(&program.0, true);
        let reads = &execution.reads;
        tc.note(&format!("reads: {reads:?}"));
        let expected: BTreeSet<u32> = execution
            .blocks_run
            .iter()
            .map(|&index| blocks[index].id)
            .collect();

        let mut context = Context::default();
        let prelude = format!(
            "var window = globalThis; var app = {{}}; var reads = {};
            Object.defineProperty(globalThis, 'x', {{
                get: () => reads.length > 0 ? reads.shift() : '',
            }});",
            serde_json::to_string(&reads).unwrap()
        );
        context.eval(Source::from_bytes(&prelude)).unwrap();
        context.eval(Source::from_bytes(&code)).unwrap();
        let hits = context
            .eval(Source::from_bytes(formatcp!(
                "JSON.stringify(Array.from({NAMESPACE}.{HITS}))"
            )))
            .unwrap()
            .to_string(&mut context)
            .unwrap()
            .to_std_string_escaped();
        let hits: Vec<u32> = serde_json::from_str(&hits).unwrap();
        assert_eq!(hits.into_iter().collect::<BTreeSet<u32>>(), expected);
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
