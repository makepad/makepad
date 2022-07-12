use {crate::Ast, std::result};

pub fn parse(pattern: &str) -> Result<Ast> {
    ParseContext { pattern, pos: 0 }.parse()
}

pub type Result<T> = result::Result<T, Error>;

pub struct Error {
    pub pos: usize,
    pub message: String,
}

struct ParseContext<'a> {
    pattern: &'a str,
    pos: usize,
}

impl<'a> ParseContext<'a> {
    fn parse(&mut self) -> Result<Ast> {
        self.parse_alt()
    }

    fn parse_alt(&mut self) -> Result<Ast> {
        let ast = self.parse_cat()?;
        if self.peek_char() != Some('|') {
            return Ok(ast);
        }
        let mut asts = vec![ast];
        loop {
            self.skip_char();
            asts.push(self.parse_cat()?);
            if self.peek_char() != Some('|') {
                break;
            }
        }
        Ok(Ast::Alt(asts))
    }

    fn parse_cat(&mut self) -> Result<Ast> {
        let ast = self.parse_rep()?;
        if self.peek_char().map_or(true, |c| c == '|' || c == ')') {
            return Ok(ast);
        }
        let mut asts = vec![ast];
        loop {
            self.skip_char();
            asts.push(self.parse_rep()?);
            if self.peek_char().map_or(true, |c| c == '|' || c == ')') {
                break;
            }
        }
        Ok(Ast::Alt(asts))
    }

    fn parse_rep(&mut self) -> Result<Ast> {
        let ast = self.parse_atom()?;
        Ok(match self.peek_char() {
            Some('?') => {
                self.skip_char();
                Ast::Quest(Box::new(ast))
            }
            Some('*') => {
                self.skip_char();
                Ast::Star(Box::new(ast))
            }
            Some('+') => {
                self.skip_char();
                Ast::Plus(Box::new(ast))
            }
            _ => ast,
        })
    }

    fn parse_atom(&mut self) -> Result<Ast> {
        Ok(match self.peek_char() {
            Some('(') => {
                self.skip_char();
                let ast = self.parse()?;
                if self.peek_char() != Some(')') {
                    return Err(self.error(""));
                }
                self.skip_char();
                ast
            }
            Some(c) => {
                self.skip_char();
                Ast::Char(c)
            }
            None => return Err(self.error("")),
        })
    }

    fn error(&self, message: &str) -> Error {
        return Error {
            pos: self.pos,
            message: message.to_string(),
        };
    }

    fn peek_char(&self) -> Option<char> {
        self.pattern[self.pos..].chars().next()
    }

    fn skip_char(&mut self) {
        self.pos += self.peek_char().unwrap().len_utf8();
    }
}
