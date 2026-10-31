//! rustc_mir_build/src/thir/cx/block.rs: blocks and their statements.
use super::{Cx, Block, BlockId, BlockSafety, Stmt, StmtId, StmtKind};
use crate::ast;
use crate::mir::build::BuildResult;
use crate::region_scope::{Node, ScopeData};

impl Cx<'_> {
    pub(crate) fn mirror_block(&mut self, b: ast::BlockId) -> BuildResult<BlockId> {
        self.mirror_block_with(b, BlockSafety::Safe, false)
    }

    /// A block of the AST: `safety` for an `unsafe` block, `targeted_by_break` for a labeled one.
    pub(super) fn mirror_block_with(&mut self, b: ast::BlockId, safety: BlockSafety, targeted_by_break: bool) -> BuildResult<BlockId> {
        let ast = self.ast();
        let blk = *ast.block(b);
        let span = self.span(blk.lo, blk.hi);
        let region_scope = self.node_scope(Node::Block(b.0), span)?;
        let stmts = ast.list(blk.stmts);
        // The tail expression is the last statement without a `;`.
        let tail = match stmts.last().and_then(|s| ast.stmt_expr(s)) {
            Some((e, false)) => Some(e),
            _ => None,
        };
        let spine = stmts.len() - usize::from(tail.is_some());
        // We have to eagerly lower the "spine" of the statements in order to get the lexical scoping
        // correctly.
        let mut out: Vec<StmtId> = Vec::new();
        for (index, stmt) in stmts[..spine].iter().enumerate() {
            if let Some(id) = self.mirror_stmt(b, blk.stmts.lo, index, stmt)? {
                out.push(id);
            }
        }
        let expr = match tail {
            Some(e) => Some(self.mirror_expr(e)?),
            None => None,
        };
        let block = Block { targeted_by_break, region_scope, span, stmts: out.into_boxed_slice(), expr, safety_mode: safety };
        Ok(self.thir.alloc_block(block))
    }

    fn mirror_stmt(&mut self, b: ast::BlockId, first: u32, index: usize, stmt: &ast::Stmt) -> BuildResult<Option<StmtId>> {
        let ast = self.ast();
        let node = first + index as u32;
        match *stmt {
            // ignored for purposes of the MIR
            ast::Stmt::Item(_) | ast::Stmt::Empty => Ok(None),
            ast::Stmt::Expr(..) | ast::Stmt::Attrs(_) => {
                let (e, _) = ast.stmt_expr(stmt).expect("expression statement");
                let span = self.espan(e);
                let scope = self.node_scope(Node::Statement(node), span)?;
                let expr = self.mirror_expr(e)?;
                Ok(Some(self.thir.alloc_stmt(Stmt { kind: StmtKind::Expr { scope, expr } })))
            }
            ast::Stmt::Let(row) => {
                let ast::LetStmt { pat, ty, init, else_, .. } = *ast.row(row);
                let pat_span = self.pat_span(pat);
                let span = match init {
                    Some(init) => self.span(pat_span.lo, self.ast().expr(init).hi.max(pat_span.hi)),
                    None => pat_span,
                };
                let remainder_scope = self.node_scope_data(Node::Block(b.0), ScopeData::Remainder(index as u32), span)?;
                let init_scope = self.node_scope(Node::Statement(node), span)?;
                let else_block = match else_ {
                    Some(els) => Some(self.mirror_block(els)?),
                    None => None,
                };
                self.pat_hint = pat_span.lo;
                let pattern = self.lower_pattern_for_let(pat, ty.map(|t| (t, pat_span)))?;
                let initializer = match init {
                    Some(init) => Some(self.mirror_expr(init)?),
                    None => None,
                };
                let kind = StmtKind::Let { remainder_scope, init_scope, pattern: Box::new(pattern), initializer, else_block, node, span };
                Ok(Some(self.thir.alloc_stmt(Stmt { kind })))
            }
        }
    }

    fn node_scope_data(&self, node: Node, data: ScopeData, span: crate::mir::Span) -> BuildResult<crate::region_scope::ScopeId> {
        match self.scope_of(node, data) {
            Some(scope) => Ok(scope),
            None => self.unresolved(span, "region scope"),
        }
    }
}
