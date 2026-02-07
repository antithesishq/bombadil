use anyhow::anyhow;
use std::fmt;
use std::hash::{Hash, Hasher};

use const_format::{formatcp, str_replace};
use oxc::allocator;
use oxc::ast::ast::{
    AssignmentOperator, AssignmentTarget, Expression, FormalParameterRest,
    Statement, TSTypeAnnotation, TSTypeParameterDeclaration,
    TSTypeParameterInstantiation,
};
use oxc::codegen::Codegen;
use oxc::semantic::SemanticBuilder;
use oxc::{
    allocator::{Allocator, Box, CloneIn, TakeIn},
    ast::ast::{self},
    parser::Parser,
    span::{SourceType, SPAN},
};
use oxc_traverse::{traverse_mut, Traverse, TraverseCtx};

use crate::instrumentation::source_id::SourceId;

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

const PRELUDE: &str = str_replace!(
    formatcp!(
        "window.{NAMESPACE} = window.{NAMESPACE} || {{
            {EDGES_PREVIOUS}: new Uint8Array({EDGE_MAP_SIZE}),
            {EDGES_CURRENT}: new Uint8Array({EDGE_MAP_SIZE}),
            {LOCATION_PREVIOUS}: 0,
        }};"
    ),
    "        ", // indent of the block above (hacky, but it's covered by snapshot tests)
    ""
);

pub fn instrument_source_code(
    source_id: SourceId,
    source_text: &str,
    source_type: SourceType,
) -> InstrumentationResult<String> {
    let allocator = Allocator::default();
    let mut program = parse(&allocator, source_text, source_type)?;
    instrument_program(&allocator, &mut program, source_id)?;

    let program_codegen = Codegen::new().build(&program);

    let code = format!("{PRELUDE}\n{}", program_codegen.code);
    Ok(code)
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
) -> InstrumentationResult<()> {
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
        next_block_id: 0,
    };
    traverse_mut(&mut instrumenter, allocator, program, scopes, ());

    Ok(())
}

struct Instrumenter {
    source_id: SourceId,
    next_block_id: u64,
}

impl<'a> Instrumenter {
    fn coverage_hooks<'b>(
        &mut self,
        ctx: &mut TraverseCtx<'b, ()>,
    ) -> allocator::Vec<'b, Statement<'b>> {
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

        let mut hasher = std::hash::DefaultHasher::new();
        (self.source_id.0, self.next_block_id).hash(&mut hasher);
        let id = hasher.finish();
        self.next_block_id += 1;

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
                branch_id.clone_in_with_semantic_ids(ctx.ast.allocator),
                ast::BinaryOperator::BitwiseXOR,
                antithesis_member(LOCATION_PREVIOUS),
            ),
            ast::BinaryOperator::Remainder,
            ctx.ast.expression_numeric_literal(
                SPAN,
                (64 * 1024u32) as f64,
                None,
                ast::NumberBase::Decimal,
            ),
        );

        let edge_addition: Statement = ctx.ast.statement_expression(
            SPAN,
            ctx.ast.expression_assignment(
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
            ),
        );

        let location_previous_update = ctx.ast.statement_expression(
            SPAN,
            ctx.ast.expression_assignment(
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
            ),
        );

        ctx
            .ast
            .vec_from_array([edge_addition, location_previous_update])
    }

    /// Adds the following two statements to the start of block, or wraps a single statement
    /// in a block with these two at the start:
    ///
    /// ```not_rust
    /// antithesis.coverage[(<id> ^ antithesis.previous) % 65536] += 1;
    /// antithesis.previous = <id> >> 1;
    /// ```
    ///
    /// The <id> is a random integer identifying branch.
    fn insert_coverage_hook<'b>(
        &mut self,
        ctx: &mut TraverseCtx<'b, ()>,
        statement: &'_ mut Statement<'b>,
    ) {
        let mut statements = self.coverage_hooks(ctx);
        if let Statement::BlockStatement(block_statement) = statement {
            block_statement.body.splice(0..0, statements);
        } else {
            statements.push(statement.take_in(ctx.ast.allocator));
            *statement = ctx.ast.statement_block(SPAN, statements);
        }
    }

    fn wrap_iife_coverage_hook<'b>(
        &mut self,
        ctx: &mut TraverseCtx<'b, ()>,
        expression: &'_ mut Expression<'b>,
    ) {
        let mut statements = self.coverage_hooks(ctx);
        let expression_old = expression.take_in(ctx.ast.allocator);
        let return_expression = ctx.ast.statement_return(
            SPAN,
            Some(ctx.ast.expression_parenthesized(SPAN, expression_old)),
        );
        statements.push(return_expression);
        let function_body =
            ctx.ast.function_body(SPAN, ctx.ast.vec(), statements);
        *expression = ctx.ast.expression_call(
            SPAN,
            ctx.ast.expression_parenthesized(SPAN,
                ctx.ast.expression_arrow_function(
                    SPAN,
                    false,
                    false,
                    None::<TSTypeParameterDeclaration<'b>>,
                    ctx.ast
                    .formal_parameters::<Option<Box<'b, FormalParameterRest<'b>>>>(
                        SPAN,
                        ast::FormalParameterKind::ArrowFormalParameters,
                        ctx.ast.vec(),
                        None,
                    ),
                    None::<TSTypeAnnotation<'b>>,
                    function_body,
                )),
                None::<TSTypeParameterInstantiation<'b>>,
                ctx.ast.vec(),
                false,
                );
    }
}

impl<'a> Traverse<'a, ()> for Instrumenter {
    /// Add coverage hooks to ternary expression branches.
    fn exit_conditional_expression(
        &mut self,
        expression: &mut ast::ConditionalExpression<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.wrap_iife_coverage_hook(ctx, &mut expression.consequent);
        self.wrap_iife_coverage_hook(ctx, &mut expression.alternate);
    }

    /// Add coverage hooks to if statement branches.
    fn exit_if_statement(
        &mut self,
        statement: &mut ast::IfStatement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.insert_coverage_hook(ctx, &mut statement.consequent);

        let empty_block = ctx.ast.statement_block(SPAN, ctx.ast.vec());
        if statement.alternate.is_none() {
            statement.alternate = Some(empty_block);
        }
        let alternate = statement.alternate.as_mut().unwrap();

        self.insert_coverage_hook(ctx, alternate);
    }

    fn exit_for_statement(
        &mut self,
        statement: &mut ast::ForStatement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.insert_coverage_hook(ctx, &mut statement.body);
    }

    fn exit_for_in_statement(
        &mut self,
        statement: &mut ast::ForInStatement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.insert_coverage_hook(ctx, &mut statement.body);
    }

    fn exit_for_of_statement(
        &mut self,
        statement: &mut ast::ForOfStatement<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        self.insert_coverage_hook(ctx, &mut statement.body);
    }

    fn exit_switch_case(
        &mut self,
        node: &mut ast::SwitchCase<'a>,
        ctx: &mut TraverseCtx<'a, ()>,
    ) {
        let statements = self.coverage_hooks(ctx);
        node.consequent.splice(0..0, statements);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    #[test]
    fn test_instrument_source_code_ternary() {
        let source_text = r#"
            function example(a, b, c) {
                return a ? b : c;
            }
            console.log(example(true, 1, 2));
        "#;

        let code =
            instrument_source_code(SourceId(0), source_text, SourceType::cjs())
                .unwrap();
        assert_snapshot!(code);
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

        let code =
            instrument_source_code(SourceId(0), source_text, SourceType::cjs())
                .unwrap();
        assert_snapshot!(code);
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

        let code =
            instrument_source_code(SourceId(0), source_text, SourceType::cjs())
                .unwrap();
        assert_snapshot!(code);
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

        let code =
            instrument_source_code(SourceId(0), source_text, SourceType::cjs())
                .unwrap();
        assert_snapshot!(code);
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

        let code =
            instrument_source_code(SourceId(0), source_text, SourceType::cjs())
                .unwrap();
        assert_snapshot!(code);
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

        let code =
            instrument_source_code(SourceId(0), source_text, SourceType::cjs())
                .unwrap();
        assert_snapshot!(code);
    }
}
