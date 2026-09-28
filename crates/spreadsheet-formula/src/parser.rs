use crate::{
    ast::*,
    lexer::{Token, TokenKind, lex},
};

pub fn parse(source: &str) -> Result<DiagnosticAst, ParseError> {
    let mut parser = Parser {
        tokens: lex(source)?,
        position: 0,
        nodes: Vec::new(),
        spans: Vec::new(),
        depths: Vec::new(),
    };
    let root = parser.expression(0, 0)?;
    if parser.peek()?.kind != TokenKind::End {
        return Err(parser.error(ParseErrorCode::UnexpectedToken)?);
    }
    Ok(DiagnosticAst {
        ast: Ast {
            schema_version: AST_SCHEMA_VERSION,
            root,
            nodes: parser.nodes,
        },
        spans: parser.spans,
    })
}
struct Parser {
    tokens: Vec<Token>,
    position: usize,
    nodes: Vec<Node>,
    spans: Vec<Span>,
    depths: Vec<usize>,
}
impl Parser {
    fn peek(&self) -> Result<&Token, ParseError> {
        self.tokens
            .get(self.position)
            .ok_or_else(|| ParseError::at(ParseErrorCode::UnexpectedToken, 0, 0))
    }
    fn take(&mut self) -> Result<Token, ParseError> {
        let token = self.peek()?.clone();
        self.position += 1;
        Ok(token)
    }
    fn error(&self, code: ParseErrorCode) -> Result<ParseError, ParseError> {
        let span = self.peek()?.span;
        Ok(ParseError { code, span })
    }
    fn add(&mut self, node: Node, span: Span) -> Result<NodeId, ParseError> {
        let depth = match &node {
            Node::Unary { operand, .. } => {
                self.depths.get(operand.0).copied().unwrap_or(MAX_DEPTH) + 1
            }
            Node::Binary { left, right, .. } => {
                self.depths
                    .get(left.0)
                    .copied()
                    .unwrap_or(MAX_DEPTH)
                    .max(self.depths.get(right.0).copied().unwrap_or(MAX_DEPTH))
                    + 1
            }
            Node::Call { arguments, .. } => {
                arguments
                    .iter()
                    .filter_map(|id| self.depths.get(id.0))
                    .max()
                    .copied()
                    .unwrap_or(0)
                    + 1
            }
            _ => 1,
        };
        if self.nodes.len() >= MAX_NODES || depth > MAX_DEPTH {
            return Err(ParseError {
                code: ParseErrorCode::LimitExceeded,
                span,
            });
        }
        let id = NodeId(self.nodes.len());
        self.nodes.push(node);
        self.spans.push(span);
        self.depths.push(depth);
        Ok(id)
    }
    fn expression(&mut self, minimum: u8, depth: usize) -> Result<NodeId, ParseError> {
        // Canonical parentheses can add one grammar frame per semantic node.
        // The AST itself retains its independent strict MAX_DEPTH bound in add().
        if depth >= MAX_DEPTH * 2 {
            return Err(self.error(ParseErrorCode::LimitExceeded)?);
        }
        let token = self.take()?;
        let start = token.span.start;
        let quoted = matches!(&token.kind, TokenKind::Quoted(_));
        let mut left = match token.kind {
            TokenKind::Number(value) => self.add(Node::Number(value), token.span)?,
            TokenKind::Text(value) => self.add(Node::Text(value), token.span)?,
            TokenKind::Error(value) => self.add(Node::Error(value), token.span)?,
            TokenKind::Plus | TokenKind::Minus => {
                let op = if token.kind == TokenKind::Plus {
                    UnaryOp::Plus
                } else {
                    UnaryOp::Minus
                };
                let operand = self.expression(70, depth + 1)?;
                let end = self.spans[operand.0].end;
                self.add(Node::Unary { op, operand }, Span { start, end })?
            }
            TokenKind::LeftParen => {
                let inside = self.expression(0, depth + 1)?;
                if self.take()?.kind != TokenKind::RightParen {
                    return Err(self.error(ParseErrorCode::UnexpectedToken)?);
                }
                inside
            }
            TokenKind::Word(word) | TokenKind::Quoted(word) => {
                if self.peek()?.kind == TokenKind::Bang {
                    self.take()?;
                    let reference = self.take()?;
                    let TokenKind::Word(address) = reference.kind else {
                        return Err(ParseError {
                            code: ParseErrorCode::InvalidReference,
                            span: reference.span,
                        });
                    };
                    let reference = parse_cell(&address, Some(word), reference.span)?;
                    self.reference(reference, start)?
                } else if quoted {
                    return Err(ParseError {
                        code: ParseErrorCode::InvalidReference,
                        span: token.span,
                    });
                } else if self.peek()?.kind == TokenKind::LeftParen {
                    if !word
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
                    {
                        return Err(ParseError {
                            code: ParseErrorCode::UnsupportedSyntax,
                            span: token.span,
                        });
                    }
                    self.take()?;
                    let mut arguments = Vec::new();
                    if self.peek()?.kind != TokenKind::RightParen {
                        loop {
                            if arguments.len() >= 255 {
                                return Err(self.error(ParseErrorCode::LimitExceeded)?);
                            }
                            let argument = if matches!(
                                self.peek()?.kind,
                                TokenKind::Comma | TokenKind::RightParen
                            ) {
                                self.add(Node::Missing, self.peek()?.span)?
                            } else {
                                self.expression(0, depth + 1)?
                            };
                            arguments.push(argument);
                            if self.peek()?.kind != TokenKind::Comma {
                                break;
                            }
                            self.take()?;
                        }
                    }
                    let close = self.take()?;
                    if close.kind != TokenKind::RightParen {
                        return Err(ParseError {
                            code: ParseErrorCode::UnexpectedToken,
                            span: close.span,
                        });
                    }
                    self.add(
                        Node::Call {
                            name: word.to_ascii_uppercase(),
                            arguments,
                        },
                        Span {
                            start,
                            end: close.span.end,
                        },
                    )?
                } else if word.eq_ignore_ascii_case("TRUE") || word.eq_ignore_ascii_case("FALSE") {
                    self.add(Node::Boolean(word.eq_ignore_ascii_case("TRUE")), token.span)?
                } else {
                    let reference = parse_cell(&word, None, token.span)?;
                    self.reference(reference, start)?
                }
            }
            _ => {
                return Err(ParseError {
                    code: ParseErrorCode::UnexpectedToken,
                    span: token.span,
                });
            }
        };
        loop {
            if self.peek()?.kind == TokenKind::Percent && minimum <= 60 {
                let end = self.take()?.span.end;
                left = self.add(
                    Node::Unary {
                        op: UnaryOp::Percent,
                        operand: left,
                    },
                    Span { start, end },
                )?;
                continue;
            }
            let Some((op, precedence)) = binary(&self.peek()?.kind) else {
                break;
            };
            if precedence < minimum {
                break;
            }
            self.take()?;
            let right = self.expression(precedence + 1, depth + 1)?;
            let end = self.spans[right.0].end;
            left = self.add(Node::Binary { op, left, right }, Span { start, end })?;
        }
        Ok(left)
    }
    fn reference(&mut self, start_ref: CellReference, start: usize) -> Result<NodeId, ParseError> {
        if self.peek()?.kind != TokenKind::Colon {
            let end = self
                .tokens
                .get(self.position.saturating_sub(1))
                .map_or(start, |t| t.span.end);
            return self.add(Node::Cell(start_ref), Span { start, end });
        }
        self.take()?;
        let endpoint = self.take()?;
        let (name, sheet) = match endpoint.kind {
            TokenKind::Word(word) | TokenKind::Quoted(word)
                if self.peek()?.kind == TokenKind::Bang =>
            {
                self.take()?;
                let next = self.take()?;
                let TokenKind::Word(address) = next.kind else {
                    return Err(ParseError {
                        code: ParseErrorCode::InvalidReference,
                        span: next.span,
                    });
                };
                (address, Some(word))
            }
            TokenKind::Word(word) => (word, start_ref.sheet.clone()),
            _ => {
                return Err(ParseError {
                    code: ParseErrorCode::InvalidReference,
                    span: endpoint.span,
                });
            }
        };
        let mut end_ref = parse_cell(&name, sheet, endpoint.span)?;
        if start_ref.sheet.as_deref().map(str::to_lowercase)
            != end_ref.sheet.as_deref().map(str::to_lowercase)
        {
            return Err(ParseError {
                code: ParseErrorCode::UnsupportedSyntax,
                span: endpoint.span,
            });
        }
        end_ref.sheet = start_ref.sheet.clone();
        let range = RangeReference {
            start: start_ref,
            end: end_ref,
        };
        if range.cell_count() > MAX_RANGE_CELLS {
            return Err(ParseError {
                code: ParseErrorCode::LimitExceeded,
                span: endpoint.span,
            });
        }
        let end = self
            .tokens
            .get(self.position.saturating_sub(1))
            .map_or(start, |t| t.span.end);
        self.add(Node::Range(range), Span { start, end })
    }
}
fn parse_cell(text: &str, sheet: Option<String>, span: Span) -> Result<CellReference, ParseError> {
    let invalid = || ParseError {
        code: ParseErrorCode::InvalidReference,
        span,
    };
    let bytes = text.as_bytes();
    let mut cursor = 0;
    let column_absolute = bytes.get(cursor) == Some(&b'$');
    if column_absolute {
        cursor += 1;
    }
    let column_start = cursor;
    let mut column = 0u32;
    while let Some(&ch) = bytes.get(cursor).filter(|ch| ch.is_ascii_alphabetic()) {
        column = column
            .checked_mul(26)
            .and_then(|v| v.checked_add(u32::from(ch.to_ascii_uppercase() - b'A') + 1))
            .ok_or_else(invalid)?;
        cursor += 1;
    }
    if cursor == column_start || column > 16_384 {
        return Err(invalid());
    }
    let row_absolute = bytes.get(cursor) == Some(&b'$');
    if row_absolute {
        cursor += 1;
    }
    let row_start = cursor;
    let mut row = 0u32;
    while let Some(&ch) = bytes.get(cursor).filter(|ch| ch.is_ascii_digit()) {
        row = row
            .checked_mul(10)
            .and_then(|v| v.checked_add(u32::from(ch - b'0')))
            .ok_or_else(invalid)?;
        cursor += 1;
    }
    if row_start == cursor
        || cursor != bytes.len()
        || row == 0
        || row > 1_048_576
        || bytes.get(row_start) == Some(&b'0')
    {
        return Err(invalid());
    }
    Ok(CellReference {
        sheet,
        row: row - 1,
        column: column - 1,
        row_absolute,
        column_absolute,
    })
}
fn binary(token: &TokenKind) -> Option<(BinaryOp, u8)> {
    Some(match token {
        TokenKind::Plus => (BinaryOp::Add, 30),
        TokenKind::Minus => (BinaryOp::Subtract, 30),
        TokenKind::Star => (BinaryOp::Multiply, 40),
        TokenKind::Slash => (BinaryOp::Divide, 40),
        TokenKind::Caret => (BinaryOp::Power, 50),
        TokenKind::Ampersand => (BinaryOp::Concat, 20),
        TokenKind::Equal => (BinaryOp::Equal, 10),
        TokenKind::NotEqual => (BinaryOp::NotEqual, 10),
        TokenKind::Less => (BinaryOp::Less, 10),
        TokenKind::LessEqual => (BinaryOp::LessEqual, 10),
        TokenKind::Greater => (BinaryOp::Greater, 10),
        TokenKind::GreaterEqual => (BinaryOp::GreaterEqual, 10),
        _ => return None,
    })
}
