//! Recursive-descent parser for the read-only statement shapes this engine
//! runs, following SQLite's grammar and operator precedence
//! (<https://www.sqlite.org/lang_select.html> and lang_expr.html).
//!
//! Precedence, lowest binding first:
//! `OR`, `AND`, unary `NOT`, the equality family (`= == != <> IS IN LIKE GLOB
//! BETWEEN ISNULL NOTNULL`), the ordering comparisons (`< <= > >=`), the bit
//! operators (`& | << >>`), `+ -`, `* / %`, `||`, then `COLLATE` and the unary
//! signs. Anything outside the supported grammar is a clean `Error::Sql`.

use crate::error::{Error, Result};
use crate::sql::ast::*;
use crate::sql::lexer::{tokenize, ParamRef, Tok, Token};
use crate::value::Value;

/// Parse one statement (a trailing `;` is allowed).
pub fn parse(sql: &str) -> Result<Stmt> {
    let mut p = Parser::new(sql)?;
    if p.at_eof() {
        return Err(Error::sql("empty statement"));
    }
    let stmt = if p.peek().is_kw("SELECT") || p.peek().is_punct("(") {
        Stmt::Select(Box::new(p.select()?))
    } else if p.peek().is_kw("INSERT") || p.peek().is_kw("REPLACE") {
        p.insert()?
    } else if p.peek().is_kw("UPDATE") {
        p.update()?
    } else if p.peek().is_kw("DELETE") {
        p.delete()?
    } else if p.peek().is_kw("CREATE") {
        p.create()?
    } else if p.peek().is_kw("DROP") {
        p.drop_stmt()?
    } else if p.peek().is_kw("ALTER") {
        p.alter()?
    } else if p.peek().is_kw("BEGIN") {
        p.bump();
        let kind = if p.eat_kw("IMMEDIATE") {
            TxKind::Immediate
        } else if p.eat_kw("EXCLUSIVE") {
            TxKind::Exclusive
        } else {
            p.eat_kw("DEFERRED");
            TxKind::Deferred
        };
        p.eat_kw("TRANSACTION");
        Stmt::Begin(kind)
    } else if p.peek().is_kw("COMMIT") || p.peek().is_kw("END") {
        p.bump();
        p.eat_kw("TRANSACTION");
        Stmt::Commit
    } else if p.peek().is_kw("ROLLBACK") {
        p.bump();
        p.eat_kw("TRANSACTION");
        Stmt::Rollback
    } else if p.peek().is_kw("PRAGMA") {
        p.pragma()?
    } else {
        let what = p.peek().ident_text().unwrap_or("").to_ascii_uppercase();
        return Err(Error::sql(format!(
            "unsupported statement{}",
            if what.is_empty() {
                String::new()
            } else {
                format!(": {what}")
            }
        )));
    };
    p.eat_punct(";");
    if !p.at_eof() {
        return Err(Error::sql("unexpected text after the statement"));
    }
    Ok(stmt)
}

/// Parse a standalone expression (used by tests and CHECK constraints).
pub fn parse_expr(sql: &str) -> Result<Expr> {
    let mut p = Parser::new(sql)?;
    let e = p.expr()?;
    if !p.at_eof() {
        return Err(Error::sql("unexpected text after the expression"));
    }
    Ok(e)
}

/// Keywords that cannot stand in for a table or column name. SQLite has a
/// longer list; these are the ones reachable from the grammar this engine
/// parses, and they keep `SELECT FROM t` from reading as a column called FROM.
const RESERVED: [&str; 48] = [
    "SELECT", "FROM", "WHERE", "GROUP", "HAVING", "ORDER", "LIMIT", "OFFSET", "JOIN", "INNER",
    "LEFT", "RIGHT", "FULL", "OUTER", "CROSS", "NATURAL", "ON", "USING", "UNION", "INTERSECT",
    "EXCEPT", "AS", "AND", "OR", "NOT", "IS", "IN", "BETWEEN", "LIKE", "GLOB", "ESCAPE", "CASE",
    "WHEN", "THEN", "ELSE", "END", "DISTINCT", "ALL", "VALUES", "INSERT", "UPDATE", "DELETE",
    "INTO", "SET", "CREATE", "DROP", "BY", "COLLATE",
];

fn is_reserved(text: &str) -> bool {
    RESERVED.iter().any(|k| text.eq_ignore_ascii_case(k))
}

/// Words that can never be a bare alias, because they start the next clause.
const NON_ALIAS: [&str; 26] = [
    "FROM", "WHERE", "GROUP", "HAVING", "ORDER", "LIMIT", "OFFSET", "JOIN", "INNER", "LEFT",
    "RIGHT", "FULL", "CROSS", "NATURAL", "ON", "USING", "UNION", "INTERSECT", "EXCEPT", "AS",
    "AND", "OR", "NOT", "IS", "IN", "COLLATE",
];

struct Parser {
    toks: Vec<Token>,
    i: usize,
    /// The original text, so DDL can be stored in `sqlite_master` verbatim.
    src: String,
    /// Largest parameter number assigned so far. A bare `?` takes one more
    /// than this, and `?NNN` raises it — SQLite's numbering, which is why the
    /// order parameters appear in the *text* is the order they bind in.
    max_param: u32,
}

impl Parser {
    fn new(sql: &str) -> Result<Parser> {
        Ok(Parser {
            toks: tokenize(sql)?,
            i: 0,
            src: sql.to_string(),
            max_param: 0,
        })
    }

    /// Source text from token `from` up to (not including) the current token.
    fn text_from(&self, from: usize) -> String {
        let start = self.toks[from.min(self.toks.len() - 1)].pos;
        let end = self.toks[self.i.min(self.toks.len() - 1)].pos;
        self.src[start..end.max(start)].trim().trim_end_matches(';').trim().to_string()
    }

    fn peek(&self) -> &Token {
        &self.toks[self.i.min(self.toks.len() - 1)]
    }
    fn peek_at(&self, n: usize) -> &Token {
        &self.toks[(self.i + n).min(self.toks.len() - 1)]
    }
    fn at_eof(&self) -> bool {
        matches!(self.peek().tok, Tok::Eof)
    }
    fn bump(&mut self) -> Token {
        let t = self.toks[self.i.min(self.toks.len() - 1)].clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.peek().is_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect_kw(&mut self, kw: &str) -> Result<()> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(Error::sql(format!("expected {kw}")))
        }
    }
    fn eat_punct(&mut self, p: &str) -> bool {
        if self.peek().is_punct(p) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect_punct(&mut self, p: &str) -> Result<()> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(Error::sql(format!("expected {p:?}")))
        }
    }
    fn ident(&mut self) -> Result<String> {
        match self.peek().ident_text() {
            Some(s) => {
                let s = s.to_string();
                self.bump();
                Ok(s)
            }
            None => Err(Error::sql("expected a name")),
        }
    }

    /// A table or column name: a reserved word here is a syntax error, not an
    /// identifier, unless it was quoted.
    fn plain_ident(&mut self) -> Result<String> {
        match &self.peek().tok {
            Tok::Ident { text, quoted } if *quoted || !is_reserved(text) => {
                let s = text.clone();
                self.bump();
                Ok(s)
            }
            Tok::Ident { text, .. } => Err(Error::sql(format!(
                "{} is a keyword and cannot be a name here",
                text.to_ascii_uppercase()
            ))),
            _ => Err(Error::sql("expected a name")),
        }
    }

    // -----------------------------------------------------------------------
    // SELECT
    // -----------------------------------------------------------------------

    fn select(&mut self) -> Result<SelectStmt> {
        let mut first = self.select_core()?;
        // Compound selects chain to the right; ORDER BY / LIMIT at the end
        // belong to the whole compound, so they are kept on the first select.
        let mut tail: Option<Box<Compound>> = None;
        {
            let mut sink = &mut tail;
            loop {
                let op = if self.eat_kw("UNION") {
                    if self.eat_kw("ALL") {
                        CompoundOp::UnionAll
                    } else {
                        CompoundOp::Union
                    }
                } else if self.eat_kw("INTERSECT") {
                    CompoundOp::Intersect
                } else if self.eat_kw("EXCEPT") {
                    CompoundOp::Except
                } else {
                    break;
                };
                let next = self.select_core()?;
                *sink = Some(Box::new(Compound { op, select: next }));
                sink = &mut sink.as_mut().expect("just set").select.compound;
            }
        }
        first.compound = tail;

        if self.eat_kw("ORDER") {
            self.expect_kw("BY")?;
            loop {
                let expr = self.expr()?;
                let mut desc = false;
                if self.eat_kw("ASC") {
                } else if self.eat_kw("DESC") {
                    desc = true;
                }
                let mut nulls_first = None;
                if self.eat_kw("NULLS") {
                    if self.eat_kw("FIRST") {
                        nulls_first = Some(true);
                    } else if self.eat_kw("LAST") {
                        nulls_first = Some(false);
                    } else {
                        return Err(Error::sql("expected FIRST or LAST after NULLS"));
                    }
                }
                first.order_by.push(OrderTerm {
                    expr,
                    desc,
                    nulls_first,
                });
                if !self.eat_punct(",") {
                    break;
                }
            }
        }
        if self.eat_kw("LIMIT") {
            let a = self.expr()?;
            if self.eat_kw("OFFSET") {
                first.limit = Some(a);
                first.offset = Some(self.expr()?);
            } else if self.eat_punct(",") {
                // The legacy "LIMIT offset, count" spelling.
                first.offset = Some(a);
                first.limit = Some(self.expr()?);
            } else {
                first.limit = Some(a);
            }
        }
        Ok(first)
    }

    fn select_core(&mut self) -> Result<SelectStmt> {
        // A parenthesised select, e.g. `(SELECT ...) UNION ...`.
        if self.eat_punct("(") {
            let inner = self.select()?;
            self.expect_punct(")")?;
            return Ok(inner);
        }
        self.expect_kw("SELECT")?;
        let mut stmt = SelectStmt::empty();
        if self.eat_kw("DISTINCT") {
            stmt.distinct = true;
        } else {
            self.eat_kw("ALL");
        }
        loop {
            stmt.columns.push(self.result_column()?);
            if !self.eat_punct(",") {
                break;
            }
        }
        if stmt.columns.is_empty() {
            return Err(Error::sql("SELECT needs at least one result column"));
        }
        if self.eat_kw("FROM") {
            stmt.from = Some(self.from_clause()?);
        }
        if self.eat_kw("WHERE") {
            stmt.where_clause = Some(self.expr()?);
        }
        if self.eat_kw("GROUP") {
            self.expect_kw("BY")?;
            loop {
                stmt.group_by.push(self.expr()?);
                if !self.eat_punct(",") {
                    break;
                }
            }
            if self.eat_kw("HAVING") {
                stmt.having = Some(self.expr()?);
            }
        }
        Ok(stmt)
    }

    fn result_column(&mut self) -> Result<ResultColumn> {
        if self.peek().is_punct("*") {
            self.bump();
            return Ok(ResultColumn::Star);
        }
        // `t.*`
        if self.peek().ident_text().is_some()
            && self.peek_at(1).is_punct(".")
            && self.peek_at(2).is_punct("*")
        {
            let name = self.ident()?;
            self.bump(); // .
            self.bump(); // *
            return Ok(ResultColumn::TableStar(name));
        }
        let expr = self.expr()?;
        let alias = self.opt_alias()?;
        Ok(ResultColumn::Expr { expr, alias })
    }

    fn opt_alias(&mut self) -> Result<Option<String>> {
        if self.eat_kw("AS") {
            return Ok(Some(self.ident()?));
        }
        match &self.peek().tok {
            Tok::Ident { text, quoted } => {
                let is_keyword = !*quoted
                    && NON_ALIAS
                        .iter()
                        .any(|k| text.eq_ignore_ascii_case(k));
                if is_keyword {
                    Ok(None)
                } else {
                    Ok(Some(self.ident()?))
                }
            }
            Tok::Str(s) => {
                // SQLite tolerates a string literal as an alias.
                let s = s.clone();
                self.bump();
                Ok(Some(s))
            }
            _ => Ok(None),
        }
    }

    fn from_clause(&mut self) -> Result<FromClause> {
        let base = self.table_ref()?;
        let mut joins = Vec::new();
        loop {
            if self.eat_punct(",") {
                let table = self.table_ref()?;
                joins.push(Join {
                    kind: JoinKind::Cross,
                    table,
                    constraint: JoinConstraint::None,
                });
                continue;
            }
            let save = self.i;
            let mut kind = JoinKind::Inner;
            let mut saw_modifier = false;
            if self.eat_kw("NATURAL") {
                return Err(Error::unsupported("NATURAL JOIN"));
            }
            if self.eat_kw("LEFT") {
                self.eat_kw("OUTER");
                kind = JoinKind::Left;
                saw_modifier = true;
            } else if self.eat_kw("RIGHT") || self.eat_kw("FULL") {
                return Err(Error::unsupported("RIGHT/FULL OUTER JOIN"));
            } else if self.eat_kw("CROSS") {
                kind = JoinKind::Cross;
                saw_modifier = true;
            } else if self.eat_kw("INNER") {
                saw_modifier = true;
            }
            if !self.eat_kw("JOIN") {
                if saw_modifier {
                    return Err(Error::sql("expected JOIN"));
                }
                self.i = save;
                break;
            }
            let table = self.table_ref()?;
            let constraint = if self.eat_kw("ON") {
                JoinConstraint::On(self.expr()?)
            } else if self.eat_kw("USING") {
                self.expect_punct("(")?;
                let mut cols = Vec::new();
                loop {
                    cols.push(self.ident()?);
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(")")?;
                JoinConstraint::Using(cols)
            } else {
                JoinConstraint::None
            };
            joins.push(Join {
                kind,
                table,
                constraint,
            });
        }
        Ok(FromClause { base, joins })
    }

    fn table_ref(&mut self) -> Result<TableRef> {
        if self.peek().is_punct("(") {
            // A subquery, or a parenthesised join we do not support.
            if self.peek_at(1).is_kw("SELECT") || self.peek_at(1).is_punct("(") {
                self.bump();
                let select = self.select()?;
                self.expect_punct(")")?;
                let alias = self.opt_alias()?;
                return Ok(TableRef::Subquery {
                    select: Box::new(select),
                    alias,
                });
            }
            return Err(Error::unsupported("parenthesised join in FROM"));
        }
        let mut name = self.plain_ident()?;
        if self.eat_punct(".") {
            // schema.table: the schema qualifier is ignored (one database).
            name = self.plain_ident()?;
        }
        let alias = self.opt_alias()?;
        Ok(TableRef::Named { name, alias })
    }

    // -----------------------------------------------------------------------
    // Data modification
    // -----------------------------------------------------------------------

    /// `[INSERT [OR ...] | REPLACE] INTO t [(cols)] VALUES … | SELECT … |
    /// DEFAULT VALUES [ON CONFLICT …]`
    fn insert(&mut self) -> Result<Stmt> {
        let replace_form = self.peek().is_kw("REPLACE");
        self.bump();
        let mut on_conflict = if replace_form {
            OnConflict::Replace
        } else {
            OnConflict::Abort
        };
        if !replace_form && self.eat_kw("OR") {
            on_conflict = self.conflict_action()?;
        }
        if !self.eat_kw("INTO") {
            return Err(Error::sql("expected INTO"));
        }
        let table = self.qualified_name()?;
        let mut columns = Vec::new();
        if self.peek().is_punct("(") && !self.peek_at(1).is_kw("SELECT") {
            self.bump();
            loop {
                columns.push(self.plain_ident()?);
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct(")")?;
        }
        let source = if self.eat_kw("VALUES") {
            let mut rows = Vec::new();
            loop {
                self.expect_punct("(")?;
                let mut row = Vec::new();
                if !self.peek().is_punct(")") {
                    loop {
                        row.push(self.expr()?);
                        if !self.eat_punct(",") {
                            break;
                        }
                    }
                }
                self.expect_punct(")")?;
                rows.push(row);
                if !self.eat_punct(",") {
                    break;
                }
            }
            InsertSource::Values(rows)
        } else if self.peek().is_kw("SELECT") || self.peek().is_punct("(") {
            InsertSource::Select(Box::new(self.select()?))
        } else if self.eat_kw("DEFAULT") {
            self.expect_kw("VALUES")?;
            InsertSource::DefaultValues
        } else {
            return Err(Error::sql("expected VALUES, SELECT or DEFAULT VALUES"));
        };
        if self.eat_kw("ON") {
            self.expect_kw("CONFLICT")?;
            let mut target = Vec::new();
            if self.eat_punct("(") {
                loop {
                    target.push(self.plain_ident()?);
                    // Ignore COLLATE / ASC / DESC inside a conflict target.
                    while self.eat_kw("COLLATE") {
                        let _ = self.ident()?;
                    }
                    self.eat_kw("ASC");
                    self.eat_kw("DESC");
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(")")?;
                if self.eat_kw("WHERE") {
                    let _ = self.expr()?; // partial-index target predicate
                }
            }
            self.expect_kw("DO")?;
            if self.eat_kw("NOTHING") {
                on_conflict = OnConflict::DoNothing { target };
            } else if self.eat_kw("UPDATE") {
                self.expect_kw("SET")?;
                let sets = self.set_list()?;
                let where_clause = if self.eat_kw("WHERE") {
                    Some(self.expr()?)
                } else {
                    None
                };
                on_conflict = OnConflict::DoUpdate {
                    target,
                    sets,
                    where_clause,
                };
            } else {
                return Err(Error::sql("expected DO NOTHING or DO UPDATE"));
            }
        }
        Ok(Stmt::Insert(Box::new(InsertStmt {
            table,
            columns,
            source,
            on_conflict,
        })))
    }

    fn conflict_action(&mut self) -> Result<OnConflict> {
        if self.eat_kw("IGNORE") {
            Ok(OnConflict::Ignore)
        } else if self.eat_kw("REPLACE") {
            Ok(OnConflict::Replace)
        } else if self.eat_kw("ABORT") || self.eat_kw("FAIL") || self.eat_kw("ROLLBACK") {
            Ok(OnConflict::Abort)
        } else {
            Err(Error::sql("expected a conflict action after OR"))
        }
    }

    fn set_list(&mut self) -> Result<Vec<(String, Expr)>> {
        let mut sets = Vec::new();
        loop {
            let name = self.plain_ident()?;
            if !self.eat_punct("=") && !self.eat_punct("==") {
                return Err(Error::sql("expected = in a SET clause"));
            }
            sets.push((name, self.expr()?));
            if !self.eat_punct(",") {
                break;
            }
        }
        Ok(sets)
    }

    fn update(&mut self) -> Result<Stmt> {
        self.expect_kw("UPDATE")?;
        let or_conflict = if self.eat_kw("OR") {
            self.conflict_action()?
        } else {
            OnConflict::Abort
        };
        let table = self.qualified_name()?;
        self.expect_kw("SET")?;
        let sets = self.set_list()?;
        let where_clause = if self.eat_kw("WHERE") {
            Some(self.expr()?)
        } else {
            None
        };
        Ok(Stmt::Update(Box::new(UpdateStmt {
            table,
            sets,
            where_clause,
            or_conflict,
        })))
    }

    fn delete(&mut self) -> Result<Stmt> {
        self.expect_kw("DELETE")?;
        if !self.eat_kw("FROM") {
            return Err(Error::sql("expected FROM"));
        }
        let table = self.qualified_name()?;
        let where_clause = if self.eat_kw("WHERE") {
            Some(self.expr()?)
        } else {
            None
        };
        Ok(Stmt::Delete(Box::new(DeleteStmt {
            table,
            where_clause,
        })))
    }

    // -----------------------------------------------------------------------
    // Schema changes
    // -----------------------------------------------------------------------

    fn qualified_name(&mut self) -> Result<String> {
        let mut name = self.plain_ident()?;
        if self.eat_punct(".") {
            name = self.plain_ident()?;
        }
        Ok(name)
    }

    fn create(&mut self) -> Result<Stmt> {
        let start = self.i;
        self.expect_kw("CREATE")?;
        if self.eat_kw("TEMP") || self.eat_kw("TEMPORARY") {
            return Err(Error::unsupported("temporary tables"));
        }
        if self.peek().is_kw("VIRTUAL") {
            return Err(Error::unsupported("virtual tables"));
        }
        if self.peek().is_kw("VIEW") || self.peek().is_kw("TRIGGER") {
            return Err(Error::unsupported("views and triggers"));
        }
        let unique = self.eat_kw("UNIQUE");
        if self.eat_kw("INDEX") {
            let if_not_exists = self.if_not_exists()?;
            let name = self.qualified_name()?;
            if !self.eat_kw("ON") {
                return Err(Error::sql("expected ON"));
            }
            let table = self.plain_ident()?;
            self.skip_parens()?;
            if self.eat_kw("WHERE") {
                let _ = self.expr()?;
            }
            let sql = self.text_from(start);
            return Ok(Stmt::CreateIndex {
                name,
                table,
                unique,
                if_not_exists,
                sql,
            });
        }
        if !self.eat_kw("TABLE") {
            return Err(Error::sql("expected TABLE or INDEX after CREATE"));
        }
        let if_not_exists = self.if_not_exists()?;
        let name = self.qualified_name()?;
        if self.peek().is_kw("AS") {
            return Err(Error::unsupported("CREATE TABLE ... AS SELECT"));
        }
        self.skip_parens()?;
        while self.eat_kw("WITHOUT") {
            self.eat_kw("ROWID");
        }
        self.eat_kw("STRICT");
        let sql = self.text_from(start);
        Ok(Stmt::CreateTable {
            name,
            if_not_exists,
            sql,
        })
    }

    fn if_not_exists(&mut self) -> Result<bool> {
        if self.eat_kw("IF") {
            if !self.eat_kw("NOT") {
                return Err(Error::sql("expected IF NOT EXISTS"));
            }
            if !self.eat_kw("EXISTS") {
                return Err(Error::sql("expected IF NOT EXISTS"));
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Consume a balanced parenthesised group.
    fn skip_parens(&mut self) -> Result<()> {
        self.expect_punct("(")?;
        let mut depth = 1;
        while depth > 0 {
            if self.at_eof() {
                return Err(Error::sql("unbalanced parentheses"));
            }
            let t = self.bump();
            if t.is_punct("(") {
                depth += 1;
            } else if t.is_punct(")") {
                depth -= 1;
            }
        }
        Ok(())
    }

    fn drop_stmt(&mut self) -> Result<Stmt> {
        self.expect_kw("DROP")?;
        let is_index = if self.eat_kw("TABLE") {
            false
        } else if self.eat_kw("INDEX") {
            true
        } else {
            return Err(Error::unsupported("DROP of anything but a table or index"));
        };
        let if_exists = if self.eat_kw("IF") {
            if !self.eat_kw("EXISTS") {
                return Err(Error::sql("expected IF EXISTS"));
            }
            true
        } else {
            false
        };
        let name = self.qualified_name()?;
        Ok(if is_index {
            Stmt::DropIndex { name, if_exists }
        } else {
            Stmt::DropTable { name, if_exists }
        })
    }

    fn alter(&mut self) -> Result<Stmt> {
        self.expect_kw("ALTER")?;
        if !self.eat_kw("TABLE") {
            return Err(Error::sql("expected TABLE after ALTER"));
        }
        let table = self.qualified_name()?;
        if self.eat_kw("RENAME") {
            if self.eat_kw("TO") {
                let new_name = self.qualified_name()?;
                return Ok(Stmt::AlterRenameTable { table, new_name });
            }
            return Err(Error::unsupported("ALTER TABLE RENAME COLUMN"));
        }
        if self.eat_kw("ADD") {
            self.eat_kw("COLUMN");
            let start = self.i;
            // The column definition runs to the end of the statement.
            while !self.at_eof() && !self.peek().is_punct(";") {
                self.bump();
            }
            let column_sql = self.text_from(start);
            if column_sql.is_empty() {
                return Err(Error::sql("ADD COLUMN needs a column definition"));
            }
            return Ok(Stmt::AlterAddColumn { table, column_sql });
        }
        Err(Error::unsupported("this ALTER TABLE form"))
    }

    fn pragma(&mut self) -> Result<Stmt> {
        self.expect_kw("PRAGMA")?;
        let name = self.qualified_name()?;
        // A pragma value is often a bare word (FULL, WAL, ON, NORMAL); those
        // are keywords to the parser but plain text to a pragma.
        let value = if self.eat_punct("=") || self.eat_punct("==") {
            Some(self.pragma_value()?)
        } else if self.eat_punct("(") {
            let e = self.pragma_value()?;
            self.expect_punct(")")?;
            Some(e)
        } else {
            None
        };
        Ok(Stmt::Pragma { name, value })
    }

    /// Give a parameter its final number, in the order it appears in the text.
    fn number_param(&mut self, p: ParamRef) -> ParamRef {
        match p {
            ParamRef::Next => {
                self.max_param += 1;
                ParamRef::Index(self.max_param)
            }
            ParamRef::Index(n) => {
                self.max_param = self.max_param.max(n);
                ParamRef::Index(n)
            }
            named => named,
        }
    }

    fn pragma_value(&mut self) -> Result<Expr> {
        if let Tok::Ident { text, quoted } = self.peek().tok.clone() {
            let upper = text.to_ascii_uppercase();
            if !quoted && !matches!(upper.as_str(), "NULL" | "TRUE" | "FALSE") {
                self.bump();
                return Ok(Expr::Literal(Value::Text(text)));
            }
        }
        self.expr()
    }

    // -----------------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------------

    fn expr(&mut self) -> Result<Expr> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.and_expr()?;
        while self.eat_kw("OR") {
            let rhs = self.and_expr()?;
            lhs = Expr::Binary {
                op: BinOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn and_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.not_expr()?;
        while self.eat_kw("AND") {
            let rhs = self.not_expr()?;
            lhs = Expr::Binary {
                op: BinOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn not_expr(&mut self) -> Result<Expr> {
        if self.eat_kw("NOT") {
            let e = self.not_expr()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(e),
            });
        }
        self.eq_expr()
    }

    /// The equality family, including the postfix and infix keyword operators.
    fn eq_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.cmp_expr()?;
        loop {
            if self.peek().is_punct("=") || self.peek().is_punct("==") {
                self.bump();
                let rhs = self.cmp_expr()?;
                lhs = bin(BinOp::Eq, lhs, rhs);
                continue;
            }
            if self.peek().is_punct("!=") || self.peek().is_punct("<>") {
                self.bump();
                let rhs = self.cmp_expr()?;
                lhs = bin(BinOp::Ne, lhs, rhs);
                continue;
            }
            if self.peek().is_kw("ISNULL") {
                self.bump();
                lhs = Expr::IsNull {
                    expr: Box::new(lhs),
                    negated: false,
                };
                continue;
            }
            if self.peek().is_kw("NOTNULL") {
                self.bump();
                lhs = Expr::IsNull {
                    expr: Box::new(lhs),
                    negated: true,
                };
                continue;
            }
            if self.peek().is_kw("IS") {
                self.bump();
                let negated = self.eat_kw("NOT");
                if self.eat_kw("NULL") {
                    lhs = Expr::IsNull {
                        expr: Box::new(lhs),
                        negated,
                    };
                } else {
                    let rhs = self.cmp_expr()?;
                    lhs = bin(if negated { BinOp::IsNot } else { BinOp::Is }, lhs, rhs);
                }
                continue;
            }
            // The NOT-prefixed forms: NOT IN / NOT LIKE / NOT GLOB / NOT BETWEEN
            let negated = if self.peek().is_kw("NOT")
                && (self.peek_at(1).is_kw("IN")
                    || self.peek_at(1).is_kw("LIKE")
                    || self.peek_at(1).is_kw("GLOB")
                    || self.peek_at(1).is_kw("BETWEEN"))
            {
                self.bump();
                true
            } else {
                false
            };
            if self.peek().is_kw("IN") {
                self.bump();
                lhs = self.in_tail(lhs, negated)?;
                continue;
            }
            if self.peek().is_kw("LIKE") || self.peek().is_kw("GLOB") {
                let glob = self.peek().is_kw("GLOB");
                self.bump();
                let rhs = self.cmp_expr()?;
                let escape = if self.eat_kw("ESCAPE") {
                    Some(Box::new(self.cmp_expr()?))
                } else {
                    None
                };
                lhs = Expr::Like {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    escape,
                    negated,
                    glob,
                };
                continue;
            }
            if self.peek().is_kw("BETWEEN") {
                self.bump();
                let low = self.cmp_expr()?;
                self.expect_kw("AND")?;
                let high = self.cmp_expr()?;
                lhs = Expr::Between {
                    expr: Box::new(lhs),
                    low: Box::new(low),
                    high: Box::new(high),
                    negated,
                };
                continue;
            }
            if negated {
                return Err(Error::sql("expected IN, LIKE, GLOB or BETWEEN after NOT"));
            }
            return Ok(lhs);
        }
    }

    fn in_tail(&mut self, lhs: Expr, negated: bool) -> Result<Expr> {
        self.expect_punct("(")?;
        if self.peek().is_kw("SELECT") {
            let select = self.select()?;
            self.expect_punct(")")?;
            return Ok(Expr::InSelect {
                expr: Box::new(lhs),
                select: Box::new(select),
                negated,
            });
        }
        let mut list = Vec::new();
        if !self.peek().is_punct(")") {
            loop {
                list.push(self.expr()?);
                if !self.eat_punct(",") {
                    break;
                }
            }
        }
        self.expect_punct(")")?;
        Ok(Expr::InList {
            expr: Box::new(lhs),
            list,
            negated,
        })
    }

    fn cmp_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.bit_expr()?;
        loop {
            let op = if self.peek().is_punct("<") {
                BinOp::Lt
            } else if self.peek().is_punct("<=") {
                BinOp::Le
            } else if self.peek().is_punct(">") {
                BinOp::Gt
            } else if self.peek().is_punct(">=") {
                BinOp::Ge
            } else {
                return Ok(lhs);
            };
            self.bump();
            let rhs = self.bit_expr()?;
            lhs = bin(op, lhs, rhs);
        }
    }

    fn bit_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.add_expr()?;
        loop {
            let op = if self.peek().is_punct("&") {
                BinOp::BitAnd
            } else if self.peek().is_punct("|") {
                BinOp::BitOr
            } else if self.peek().is_punct("<<") {
                BinOp::Shl
            } else if self.peek().is_punct(">>") {
                BinOp::Shr
            } else {
                return Ok(lhs);
            };
            self.bump();
            let rhs = self.add_expr()?;
            lhs = bin(op, lhs, rhs);
        }
    }

    fn add_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.mul_expr()?;
        loop {
            let op = if self.peek().is_punct("+") {
                BinOp::Add
            } else if self.peek().is_punct("-") {
                BinOp::Sub
            } else {
                return Ok(lhs);
            };
            self.bump();
            let rhs = self.mul_expr()?;
            lhs = bin(op, lhs, rhs);
        }
    }

    fn mul_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.concat_expr()?;
        loop {
            let op = if self.peek().is_punct("*") {
                BinOp::Mul
            } else if self.peek().is_punct("/") {
                BinOp::Div
            } else if self.peek().is_punct("%") {
                BinOp::Mod
            } else {
                return Ok(lhs);
            };
            self.bump();
            let rhs = self.concat_expr()?;
            lhs = bin(op, lhs, rhs);
        }
    }

    fn concat_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.unary_expr()?;
        while self.peek().is_punct("||") {
            self.bump();
            let rhs = self.unary_expr()?;
            lhs = bin(BinOp::Concat, lhs, rhs);
        }
        Ok(lhs)
    }

    fn unary_expr(&mut self) -> Result<Expr> {
        if self.peek().is_punct("-") {
            self.bump();
            let e = self.unary_expr()?;
            // Fold a sign into the literal so negative bounds stay literals.
            return Ok(match e {
                Expr::Literal(Value::Integer(i)) => Expr::Literal(Value::Integer(-i)),
                Expr::Literal(Value::Real(f)) => Expr::Literal(Value::Real(-f)),
                other => Expr::Unary {
                    op: UnaryOp::Negate,
                    expr: Box::new(other),
                },
            });
        }
        if self.peek().is_punct("+") {
            self.bump();
            return self.unary_expr();
        }
        if self.peek().is_punct("~") {
            self.bump();
            let e = self.unary_expr()?;
            return Ok(Expr::Unary {
                op: UnaryOp::BitNot,
                expr: Box::new(e),
            });
        }
        if self.peek().is_kw("NOT") {
            self.bump();
            let e = self.unary_expr()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(e),
            });
        }
        self.collate_expr()
    }

    fn collate_expr(&mut self) -> Result<Expr> {
        let mut e = self.primary()?;
        while self.eat_kw("COLLATE") {
            let name = self.ident()?;
            e = Expr::Collate {
                expr: Box::new(e),
                collation: name,
            };
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr> {
        // Parenthesised expression or subquery.
        if self.peek().is_punct("(") {
            if self.peek_at(1).is_kw("SELECT") {
                self.bump();
                let select = self.select()?;
                self.expect_punct(")")?;
                return Ok(Expr::Subquery(Box::new(select)));
            }
            self.bump();
            let e = self.expr()?;
            self.expect_punct(")")?;
            return Ok(e);
        }
        if self.peek().is_kw("EXISTS") {
            self.bump();
            self.expect_punct("(")?;
            let select = self.select()?;
            self.expect_punct(")")?;
            return Ok(Expr::Exists {
                select: Box::new(select),
                negated: false,
            });
        }
        if self.peek().is_kw("CASE") {
            return self.case_expr();
        }
        if self.peek().is_kw("CAST") && self.peek_at(1).is_punct("(") {
            self.bump();
            self.bump();
            let e = self.expr()?;
            self.expect_kw("AS")?;
            let type_name = self.type_name()?;
            self.expect_punct(")")?;
            return Ok(Expr::Cast {
                expr: Box::new(e),
                type_name,
            });
        }
        match self.peek().tok.clone() {
            Tok::Int(v) => {
                self.bump();
                Ok(Expr::Literal(Value::Integer(v)))
            }
            Tok::Real(v) => {
                self.bump();
                Ok(Expr::Literal(Value::Real(v)))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Expr::Literal(Value::Text(s)))
            }
            Tok::Blob(b) => {
                self.bump();
                Ok(Expr::Literal(Value::Blob(b)))
            }
            Tok::Param(p) => {
                self.bump();
                Ok(Expr::Param(self.number_param(p)))
            }
            Tok::Ident { text, quoted } => {
                // Function call?
                if !quoted && self.peek_at(1).is_punct("(") {
                    return self.function_call();
                }
                if !quoted {
                    match text.to_ascii_uppercase().as_str() {
                        "NULL" => {
                            self.bump();
                            return Ok(Expr::Literal(Value::Null));
                        }
                        "TRUE" => {
                            self.bump();
                            return Ok(Expr::Literal(Value::Integer(1)));
                        }
                        "FALSE" => {
                            self.bump();
                            return Ok(Expr::Literal(Value::Integer(0)));
                        }
                        "CURRENT_DATE" | "CURRENT_TIME" | "CURRENT_TIMESTAMP" => {
                            return Err(Error::unsupported(
                                "CURRENT_DATE / CURRENT_TIME / CURRENT_TIMESTAMP",
                            ))
                        }
                        _ => {}
                    }
                }
                // Column reference, optionally qualified.
                let first = self.plain_ident()?;
                if self.eat_punct(".") {
                    let second = self.plain_ident()?;
                    if self.eat_punct(".") {
                        // schema.table.column: drop the schema.
                        let third = self.plain_ident()?;
                        return Ok(Expr::Column {
                            table: Some(second),
                            name: third,
                        });
                    }
                    return Ok(Expr::Column {
                        table: Some(first),
                        name: second,
                    });
                }
                Ok(Expr::Column {
                    table: None,
                    name: first,
                })
            }
            Tok::Punct(p) => Err(Error::sql(format!("unexpected {p:?} in an expression"))),
            Tok::Eof => Err(Error::sql("statement ended in the middle of an expression")),
        }
    }

    fn function_call(&mut self) -> Result<Expr> {
        let name = self.ident()?;
        self.expect_punct("(")?;
        let mut args = Vec::new();
        let mut distinct = false;
        let mut star = false;
        if self.peek().is_punct("*") {
            self.bump();
            star = true;
        } else if !self.peek().is_punct(")") {
            if self.eat_kw("DISTINCT") {
                distinct = true;
            } else {
                self.eat_kw("ALL");
            }
            loop {
                args.push(self.expr()?);
                if !self.eat_punct(",") {
                    break;
                }
            }
        }
        self.expect_punct(")")?;
        Ok(Expr::Function {
            name,
            args,
            distinct,
            star,
        })
    }

    fn case_expr(&mut self) -> Result<Expr> {
        self.expect_kw("CASE")?;
        let operand = if self.peek().is_kw("WHEN") {
            None
        } else {
            Some(Box::new(self.expr()?))
        };
        let mut whens = Vec::new();
        while self.eat_kw("WHEN") {
            let cond = self.expr()?;
            self.expect_kw("THEN")?;
            let result = self.expr()?;
            whens.push((cond, result));
        }
        if whens.is_empty() {
            return Err(Error::sql("CASE needs at least one WHEN"));
        }
        let else_result = if self.eat_kw("ELSE") {
            Some(Box::new(self.expr()?))
        } else {
            None
        };
        self.expect_kw("END")?;
        Ok(Expr::Case {
            operand,
            whens,
            else_result,
        })
    }

    /// A type name: one or more words, with optional `(n)` or `(n,m)`.
    fn type_name(&mut self) -> Result<String> {
        let mut parts = Vec::new();
        while self.peek().ident_text().is_some() {
            parts.push(self.ident()?);
        }
        if parts.is_empty() {
            return Err(Error::sql("expected a type name"));
        }
        if self.eat_punct("(") {
            let mut depth = 1;
            while depth > 0 {
                if self.at_eof() {
                    return Err(Error::sql("unterminated type argument list"));
                }
                let t = self.bump();
                if t.is_punct("(") {
                    depth += 1;
                } else if t.is_punct(")") {
                    depth -= 1;
                }
            }
        }
        Ok(parts.join(" "))
    }
}

fn bin(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    Expr::Binary {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(sql: &str) -> SelectStmt {
        match parse(sql).unwrap_or_else(|e| panic!("{sql}: {e}")) {
            Stmt::Select(s) => *s,
            other => panic!("{sql} parsed as {other:?}"),
        }
    }

    #[test]
    fn result_columns_and_aliases() {
        let s = sel("SELECT *, t.*, a, b AS bee, c cee FROM t");
        assert_eq!(s.columns.len(), 5);
        assert!(matches!(s.columns[0], ResultColumn::Star));
        assert!(matches!(&s.columns[1], ResultColumn::TableStar(t) if t == "t"));
        match &s.columns[3] {
            ResultColumn::Expr { alias, .. } => assert_eq!(alias.as_deref(), Some("bee")),
            _ => panic!(),
        }
        match &s.columns[4] {
            ResultColumn::Expr { alias, .. } => assert_eq!(alias.as_deref(), Some("cee")),
            _ => panic!(),
        }
    }

    #[test]
    fn and_binds_tighter_than_or() {
        let s = sel("SELECT 1 FROM t WHERE a = 1 AND b = 2 OR c = 3");
        let Some(Expr::Binary { op, lhs, .. }) = s.where_clause else {
            panic!("expected a binary root")
        };
        assert_eq!(op, BinOp::Or);
        assert!(matches!(*lhs, Expr::Binary { op: BinOp::And, .. }));
    }

    #[test]
    fn comparison_binds_tighter_than_and() {
        let s = sel("SELECT 1 FROM t WHERE a < 1 AND b > 2");
        let Some(Expr::Binary { op: BinOp::And, lhs, rhs }) = s.where_clause else {
            panic!()
        };
        assert!(matches!(*lhs, Expr::Binary { op: BinOp::Lt, .. }));
        assert!(matches!(*rhs, Expr::Binary { op: BinOp::Gt, .. }));
    }

    #[test]
    fn arithmetic_precedence() {
        let e = parse_expr("1 + 2 * 3").unwrap();
        let Expr::Binary { op: BinOp::Add, rhs, .. } = e else {
            panic!()
        };
        assert!(matches!(*rhs, Expr::Binary { op: BinOp::Mul, .. }));
        let e = parse_expr("-x * 2").unwrap();
        assert!(matches!(e, Expr::Binary { op: BinOp::Mul, .. }));
        assert_eq!(
            parse_expr("-3").unwrap(),
            Expr::Literal(Value::Integer(-3))
        );
    }

    #[test]
    fn null_tests() {
        assert!(matches!(
            parse_expr("a IS NULL").unwrap(),
            Expr::IsNull { negated: false, .. }
        ));
        assert!(matches!(
            parse_expr("a IS NOT NULL").unwrap(),
            Expr::IsNull { negated: true, .. }
        ));
        assert!(matches!(
            parse_expr("a NOTNULL").unwrap(),
            Expr::IsNull { negated: true, .. }
        ));
        // NOT applies to the whole IS NULL test
        assert!(matches!(
            parse_expr("NOT a IS NULL").unwrap(),
            Expr::Unary { op: UnaryOp::Not, .. }
        ));
        assert!(matches!(
            parse_expr("a IS 1").unwrap(),
            Expr::Binary { op: BinOp::Is, .. }
        ));
        assert!(matches!(
            parse_expr("a IS NOT 1").unwrap(),
            Expr::Binary { op: BinOp::IsNot, .. }
        ));
    }

    #[test]
    fn in_between_like() {
        assert!(matches!(
            parse_expr("a IN (1, 2, 3)").unwrap(),
            Expr::InList { negated: false, .. }
        ));
        assert!(matches!(
            parse_expr("a NOT IN (1)").unwrap(),
            Expr::InList { negated: true, .. }
        ));
        assert!(matches!(
            parse_expr("a IN (SELECT b FROM t)").unwrap(),
            Expr::InSelect { .. }
        ));
        assert!(matches!(
            parse_expr("a BETWEEN 1 AND 2").unwrap(),
            Expr::Between { negated: false, .. }
        ));
        assert!(matches!(
            parse_expr("a NOT BETWEEN 1 AND 2").unwrap(),
            Expr::Between { negated: true, .. }
        ));
        // BETWEEN must not swallow the following AND
        let e = parse_expr("a BETWEEN 1 AND 2 AND b = 3").unwrap();
        assert!(matches!(e, Expr::Binary { op: BinOp::And, .. }));
        assert!(matches!(
            parse_expr("a LIKE 'x%' ESCAPE '\\'").unwrap(),
            Expr::Like { escape: Some(_), glob: false, .. }
        ));
        assert!(matches!(
            parse_expr("a NOT GLOB 'x*'").unwrap(),
            Expr::Like { negated: true, glob: true, .. }
        ));
    }

    #[test]
    fn functions_and_case() {
        assert!(matches!(
            parse_expr("count(*)").unwrap(),
            Expr::Function { star: true, .. }
        ));
        assert!(matches!(
            parse_expr("count(DISTINCT x)").unwrap(),
            Expr::Function { distinct: true, .. }
        ));
        let Expr::Function { name, args, .. } = parse_expr("coalesce(a, b, 'c')").unwrap() else {
            panic!()
        };
        assert_eq!(name, "coalesce");
        assert_eq!(args.len(), 3);
        assert!(matches!(
            parse_expr("CASE WHEN a THEN 1 ELSE 2 END").unwrap(),
            Expr::Case { operand: None, .. }
        ));
        assert!(matches!(
            parse_expr("CASE a WHEN 1 THEN 'x' END").unwrap(),
            Expr::Case { operand: Some(_), .. }
        ));
        assert!(matches!(
            parse_expr("CAST(a AS INTEGER)").unwrap(),
            Expr::Cast { .. }
        ));
    }

    #[test]
    fn joins() {
        let s = sel("SELECT 1 FROM a JOIN b ON b.id = a.id");
        assert_eq!(s.from.as_ref().unwrap().joins.len(), 1);
        assert_eq!(s.from.as_ref().unwrap().joins[0].kind, JoinKind::Inner);
        let s = sel("SELECT 1 FROM a LEFT OUTER JOIN b USING (id)");
        assert_eq!(s.from.as_ref().unwrap().joins[0].kind, JoinKind::Left);
        assert!(matches!(
            s.from.as_ref().unwrap().joins[0].constraint,
            JoinConstraint::Using(_)
        ));
        let s = sel("SELECT 1 FROM a, b WHERE a.id = b.id");
        assert_eq!(s.from.as_ref().unwrap().joins[0].kind, JoinKind::Cross);
        let s = sel("SELECT 1 FROM a x JOIN b AS y ON x.i = y.i");
        assert_eq!(
            s.from.as_ref().unwrap().base.binding(),
            Some("x")
        );
    }

    #[test]
    fn subqueries() {
        let s = sel("SELECT n FROM (SELECT count(*) AS n FROM t) q");
        assert!(matches!(
            s.from.as_ref().unwrap().base,
            TableRef::Subquery { .. }
        ));
        assert!(matches!(
            parse_expr("EXISTS (SELECT 1 FROM t)").unwrap(),
            Expr::Exists { negated: false, .. }
        ));
        assert!(matches!(
            parse_expr("NOT EXISTS (SELECT 1 FROM t)").unwrap(),
            Expr::Unary { op: UnaryOp::Not, .. }
        ));
        assert!(matches!(
            parse_expr("(SELECT max(a) FROM t)").unwrap(),
            Expr::Subquery(_)
        ));
    }

    #[test]
    fn limits_and_order() {
        let s = sel("SELECT a FROM t ORDER BY a DESC, b NULLS LAST LIMIT 5 OFFSET 2");
        assert_eq!(s.order_by.len(), 2);
        assert!(s.order_by[0].desc);
        assert_eq!(s.order_by[1].nulls_first, Some(false));
        assert!(s.limit.is_some() && s.offset.is_some());
        let s = sel("SELECT a FROM t LIMIT 2, 5");
        assert_eq!(s.offset, Some(Expr::Literal(Value::Integer(2))));
        assert_eq!(s.limit, Some(Expr::Literal(Value::Integer(5))));
    }

    #[test]
    fn compounds() {
        let s = sel("SELECT a FROM t UNION ALL SELECT b FROM u ORDER BY 1");
        assert!(s.compound.is_some());
        assert_eq!(s.compound.as_ref().unwrap().op, CompoundOp::UnionAll);
        assert_eq!(s.order_by.len(), 1, "ORDER BY belongs to the compound");
        let s = sel("SELECT a FROM t INTERSECT SELECT b FROM u EXCEPT SELECT c FROM v");
        assert_eq!(s.compound.as_ref().unwrap().op, CompoundOp::Intersect);
        assert_eq!(
            s.compound
                .as_ref()
                .unwrap()
                .select
                .compound
                .as_ref()
                .unwrap()
                .op,
            CompoundOp::Except
        );
    }

    #[test]
    fn literals_and_params() {
        let s = sel("SELECT ?1, ?, :name, x'00ff', 'it''s', 1.5e3, NULL, TRUE");
        assert_eq!(s.columns.len(), 8);
        let ResultColumn::Expr { expr, .. } = &s.columns[0] else {
            panic!()
        };
        assert_eq!(*expr, Expr::Param(ParamRef::Index(1)));
        let ResultColumn::Expr { expr, .. } = &s.columns[3] else {
            panic!()
        };
        assert_eq!(*expr, Expr::Literal(Value::Blob(vec![0, 255])));
        let s = sel("SELECT \"quoted col\" FROM \"quoted table\"");
        assert!(matches!(
            &s.columns[0],
            ResultColumn::Expr {
                expr: Expr::Column { name, .. },
                ..
            } if name == "quoted col"
        ));
    }

    #[test]
    fn store_queries_parse() {
        // Verbatim shapes from the asset store's SQL inventory.
        for sql in [
            "SELECT t.principal_id, t.expires_ms, t.revoked, p.disabled FROM tokens t JOIN principals p ON p.principal_id = t.principal_id WHERE t.token_hash = ?1",
            "SELECT 1 FROM grants WHERE principal_id=?1 AND capability=?2 AND scope IN (?3, '*')",
            "SELECT seq, kind, detail, created_ms FROM operation_events WHERE operation_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
            "SELECT asset_id, ns, created_ms FROM asset_index WHERE ns=?1 AND asset_id>?2 ORDER BY asset_id LIMIT ?3",
            "SELECT EXISTS(SELECT 1 FROM asset_aliases WHERE asset_id = ?1), COALESCE((SELECT MIN(alias) FROM asset_aliases WHERE asset_id = ?1), '')",
            "SELECT DISTINCT aa.asset_id FROM asset_aliases aa JOIN search_annotations sa ON sa.asset_id = aa.asset_id ORDER BY aa.asset_id",
            "SELECT COALESCE(MAX(seq), 0) FROM operation_events WHERE operation_id = ?1",
            "SELECT job_id, ns, kind, enqueued_by, created_ms FROM job_meta WHERE ns=?1 ORDER BY created_ms DESC, job_id DESC LIMIT ?2",
            "SELECT COUNT(*) FROM (SELECT a.asset_id FROM search_annotations a WHERE 1=1)",
            "SELECT a.asset_id, a.namespace, a.title, a.description, a.live, 0 AS score, a.kind, a.canon_alias FROM search_annotations a WHERE 1=1",
        ] {
            parse(sql).unwrap_or_else(|e| panic!("{sql}\n  {e}"));
        }
    }

    #[test]
    fn dml_statements() {
        let Stmt::Insert(i) = parse("INSERT INTO t(a, b) VALUES (1, 'x'), (2, ?1)").unwrap() else {
            panic!()
        };
        assert_eq!(i.table, "t");
        assert_eq!(i.columns, vec!["a", "b"]);
        assert_eq!(i.on_conflict, OnConflict::Abort);
        match &i.source {
            InsertSource::Values(rows) => assert_eq!(rows.len(), 2),
            _ => panic!(),
        }
        let Stmt::Insert(i) = parse("INSERT OR IGNORE INTO t VALUES (1)").unwrap() else {
            panic!()
        };
        assert_eq!(i.on_conflict, OnConflict::Ignore);
        let Stmt::Insert(i) = parse("REPLACE INTO t VALUES (1)").unwrap() else {
            panic!()
        };
        assert_eq!(i.on_conflict, OnConflict::Replace);
        let Stmt::Insert(i) = parse(
            "INSERT INTO asset_aliases(alias, asset_id, head_revision, updated_ms) VALUES(?1, ?2, ?3, ?4) ON CONFLICT(alias) DO UPDATE SET asset_id=?2, head_revision=?3, updated_ms=?4",
        )
        .unwrap() else {
            panic!()
        };
        match &i.on_conflict {
            OnConflict::DoUpdate { target, sets, .. } => {
                assert_eq!(target, &vec!["alias".to_string()]);
                assert_eq!(sets.len(), 3);
            }
            other => panic!("{other:?}"),
        }
        let Stmt::Insert(i) = parse("INSERT INTO t(a) SELECT b FROM u").unwrap() else {
            panic!()
        };
        assert!(matches!(i.source, InsertSource::Select(_)));

        let Stmt::Update(u) = parse("UPDATE t SET a = 1, b = b + 1 WHERE id = ?1").unwrap() else {
            panic!()
        };
        assert_eq!(u.sets.len(), 2);
        assert!(u.where_clause.is_some());

        let Stmt::Delete(d) = parse("DELETE FROM t WHERE a IS NULL").unwrap() else {
            panic!()
        };
        assert_eq!(d.table, "t");
        assert!(parse("DELETE FROM t").is_ok());
    }

    #[test]
    fn ddl_and_control_statements() {
        let stmt = parse("CREATE TABLE IF NOT EXISTS t(a INTEGER PRIMARY KEY, b TEXT NOT NULL)").unwrap();
        match &stmt {
            Stmt::CreateTable {
                name,
                if_not_exists,
                sql,
            } => {
                assert_eq!(name, "t");
                assert!(if_not_exists);
                assert!(sql.starts_with("CREATE TABLE IF NOT EXISTS t("), "{sql}");
                assert!(sql.ends_with(')'), "{sql}");
            }
            other => panic!("{other:?}"),
        }
        let stmt = parse("CREATE UNIQUE INDEX i ON t(a, b)").unwrap();
        match &stmt {
            Stmt::CreateIndex {
                name,
                table,
                unique,
                sql,
                ..
            } => {
                assert_eq!((name.as_str(), table.as_str(), *unique), ("i", "t", true));
                assert_eq!(sql, "CREATE UNIQUE INDEX i ON t(a, b)");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse("DROP TABLE IF EXISTS t").unwrap(),
            Stmt::DropTable { if_exists: true, .. }
        ));
        assert!(matches!(
            parse("DROP INDEX i").unwrap(),
            Stmt::DropIndex { if_exists: false, .. }
        ));
        match parse("ALTER TABLE t ADD COLUMN c TEXT NOT NULL DEFAULT ''").unwrap() {
            Stmt::AlterAddColumn { table, column_sql } => {
                assert_eq!(table, "t");
                assert_eq!(column_sql, "c TEXT NOT NULL DEFAULT ''");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse("ALTER TABLE t RENAME TO u").unwrap(),
            Stmt::AlterRenameTable { .. }
        ));
        assert!(matches!(
            parse("BEGIN IMMEDIATE").unwrap(),
            Stmt::Begin(TxKind::Immediate)
        ));
        assert!(matches!(parse("BEGIN DEFERRED").unwrap(), Stmt::Begin(TxKind::Deferred)));
        assert!(matches!(parse("COMMIT").unwrap(), Stmt::Commit));
        assert!(matches!(parse("END TRANSACTION").unwrap(), Stmt::Commit));
        assert!(matches!(parse("ROLLBACK").unwrap(), Stmt::Rollback));
        match parse("PRAGMA user_version = 8").unwrap() {
            Stmt::Pragma { name, value } => {
                assert_eq!(name, "user_version");
                assert_eq!(value, Some(Expr::Literal(Value::Integer(8))));
            }
            other => panic!("{other:?}"),
        }
        match parse("PRAGMA table_info(assets)").unwrap() {
            Stmt::Pragma { name, value } => {
                assert_eq!(name, "table_info");
                assert!(value.is_some());
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse("PRAGMA journal_mode=WAL").unwrap(),
            Stmt::Pragma { .. }
        ));
    }

    #[test]
    fn anonymous_parameters_are_numbered_in_text_order() {
        // SQLite numbers a bare `?` one higher than the largest so far, in the
        // order it appears in the statement — not in the order a planner walks
        // the tree. The store's search statement depends on this.
        let s = sel("SELECT ?, a FROM t WHERE b IN (?, ?) AND c = ? GROUP BY a HAVING COUNT(*) = ?");
        let ResultColumn::Expr { expr, .. } = &s.columns[0] else {
            panic!()
        };
        assert_eq!(*expr, Expr::Param(ParamRef::Index(1)));
        let Some(Expr::Binary { lhs, rhs, .. }) = &s.where_clause else {
            panic!()
        };
        let Expr::InList { list, .. } = lhs.as_ref() else {
            panic!()
        };
        assert_eq!(list[0], Expr::Param(ParamRef::Index(2)));
        assert_eq!(list[1], Expr::Param(ParamRef::Index(3)));
        let Expr::Binary { rhs: c, .. } = rhs.as_ref() else {
            panic!()
        };
        assert_eq!(**c, Expr::Param(ParamRef::Index(4)));
        let Some(Expr::Binary { rhs, .. }) = &s.having else {
            panic!()
        };
        assert_eq!(**rhs, Expr::Param(ParamRef::Index(5)));
        // Explicit numbers raise the counter, as SQLite documents.
        let s = sel("SELECT ?3, ? FROM t");
        let ResultColumn::Expr { expr, .. } = &s.columns[1] else {
            panic!()
        };
        assert_eq!(*expr, Expr::Param(ParamRef::Index(4)));
    }

    #[test]
    fn errors_are_clean() {
        for bad in [
            "",
            "SELECT",
            "SELECT FROM t",
            "SELECT a FROM",
            "SELECT a FROM t WHERE",
            "SELECT (1",
            "SELECT a FROM t junk junk",
            "SELECT a FROM t GROUP",
            "SELECT a FROM t ORDER a",
            "SELECT count(",
            "SELECT CASE END",
            "SELECT a NOT b",
            "INSERT INTO t",
            "INSERT INTO t VALUES",
            "UPDATE t SET",
            "UPDATE t WHERE a = 1",
            "DELETE t",
            "CREATE TABLE",
            "CREATE INDEX i ON",
            "ALTER TABLE t",
            "DROP VIEW v",
            "PRAGMA",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} should not parse");
        }
    }
}
