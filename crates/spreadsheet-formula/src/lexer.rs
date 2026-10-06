use crate::ast::*;
#[derive(Clone, PartialEq)]
pub(crate) enum TokenKind {
    Number(f64),
    Text(String),
    Word(String),
    Quoted(String),
    Error(FormulaError),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Percent,
    Ampersand,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    LeftParen,
    RightParen,
    Comma,
    Colon,
    Bang,
    End,
}
#[derive(Clone)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub(crate) fn lex(source: &str) -> Result<Vec<Token>, ParseError> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(ParseError::at(
            ParseErrorCode::LimitExceeded,
            0,
            source.len(),
        ));
    }
    if !source.starts_with('=') {
        return Err(ParseError::at(ParseErrorCode::MissingEquals, 0, 0));
    }
    let mut position = 1;
    let mut tokens = Vec::new();
    while position < source.len() {
        let Some(ch) = source[position..].chars().next() else {
            break;
        };
        if ch.is_whitespace() {
            position += ch.len_utf8();
            continue;
        }
        let start = position;
        let kind = match ch {
            '"' | '\'' => {
                let delimiter = ch;
                position += 1;
                let mut text = String::new();
                let mut closed = false;
                while position < source.len() {
                    let Some(next) = source[position..].chars().next() else {
                        break;
                    };
                    position += next.len_utf8();
                    if next == delimiter {
                        if source[position..].starts_with(delimiter) {
                            text.push(delimiter);
                            position += 1;
                        } else {
                            closed = true;
                            break;
                        }
                    } else {
                        text.push(next);
                    }
                }
                if !closed {
                    return Err(ParseError::at(
                        ParseErrorCode::UnterminatedString,
                        start,
                        position,
                    ));
                }
                if delimiter == '"' {
                    TokenKind::Text(text)
                } else {
                    TokenKind::Quoted(text)
                }
            }
            '#' => {
                let errors = [
                    FormulaError::Div0,
                    FormulaError::Value,
                    FormulaError::Ref,
                    FormulaError::Name,
                    FormulaError::Num,
                    FormulaError::Null,
                    FormulaError::Na,
                    FormulaError::Spill,
                    FormulaError::Cycle,
                ];
                let Some(error) = errors.into_iter().find(|e| {
                    source[start..]
                        .get(..e.code().len())
                        .is_some_and(|s| s.eq_ignore_ascii_case(e.code()))
                }) else {
                    return Err(ParseError::at(
                        ParseErrorCode::UnexpectedToken,
                        start,
                        start + 1,
                    ));
                };
                position += error.code().len();
                TokenKind::Error(error)
            }
            '0'..='9' | '.'
                if ch != '.'
                    || source
                        .as_bytes()
                        .get(start + 1)
                        .is_some_and(u8::is_ascii_digit) =>
            {
                while source
                    .as_bytes()
                    .get(position)
                    .is_some_and(u8::is_ascii_digit)
                {
                    position += 1;
                }
                if source.as_bytes().get(position) == Some(&b'.') {
                    position += 1;
                    while source
                        .as_bytes()
                        .get(position)
                        .is_some_and(u8::is_ascii_digit)
                    {
                        position += 1;
                    }
                }
                if source
                    .as_bytes()
                    .get(position)
                    .is_some_and(|b| *b == b'e' || *b == b'E')
                {
                    position += 1;
                    if source
                        .as_bytes()
                        .get(position)
                        .is_some_and(|b| *b == b'+' || *b == b'-')
                    {
                        position += 1;
                    }
                    let exponent_start = position;
                    while source
                        .as_bytes()
                        .get(position)
                        .is_some_and(u8::is_ascii_digit)
                    {
                        position += 1;
                    }
                    if position == exponent_start {
                        return Err(ParseError::at(
                            ParseErrorCode::InvalidNumber,
                            start,
                            position,
                        ));
                    }
                }
                let text = &source[start..position];
                if source.as_bytes().get(position) == Some(&b'!') {
                    TokenKind::Word(text.into())
                } else {
                    let normalized = excel_significant_digit_limit(text);
                    let text = normalized.as_deref().unwrap_or(text);
                    let number: f64 = text.parse().map_err(|_| {
                        ParseError::at(ParseErrorCode::InvalidNumber, start, position)
                    })?;
                    if !number.is_finite() {
                        return Err(ParseError::at(
                            ParseErrorCode::InvalidNumber,
                            start,
                            position,
                        ));
                    }
                    TokenKind::Number(if number == 0.0 { 0.0 } else { number })
                }
            }
            c if c.is_alphabetic() || matches!(c, '_' | '$' | '.') => {
                while let Some(next) = source[position..].chars().next() {
                    if !(next.is_alphanumeric() || matches!(next, '_' | '$' | '.')) {
                        break;
                    }
                    position += next.len_utf8();
                }
                TokenKind::Word(source[start..position].into())
            }
            _ => {
                position += ch.len_utf8();
                match ch {
                    '+' => TokenKind::Plus,
                    '-' => TokenKind::Minus,
                    '*' => TokenKind::Star,
                    '/' => TokenKind::Slash,
                    '^' => TokenKind::Caret,
                    '%' => TokenKind::Percent,
                    '&' => TokenKind::Ampersand,
                    '=' => TokenKind::Equal,
                    '<' => {
                        if source[position..].starts_with('=') {
                            position += 1;
                            TokenKind::LessEqual
                        } else if source[position..].starts_with('>') {
                            position += 1;
                            TokenKind::NotEqual
                        } else {
                            TokenKind::Less
                        }
                    }
                    '>' => {
                        if source[position..].starts_with('=') {
                            position += 1;
                            TokenKind::GreaterEqual
                        } else {
                            TokenKind::Greater
                        }
                    }
                    '(' => TokenKind::LeftParen,
                    ')' => TokenKind::RightParen,
                    ',' => TokenKind::Comma,
                    ':' => TokenKind::Colon,
                    '!' => TokenKind::Bang,
                    _ => {
                        return Err(ParseError::at(
                            ParseErrorCode::UnsupportedSyntax,
                            start,
                            position,
                        ));
                    }
                }
            }
        };
        tokens.push(Token {
            kind,
            span: Span {
                start,
                end: position,
            },
        });
        if tokens.len() > MAX_NODES * 4 {
            return Err(ParseError::at(
                ParseErrorCode::LimitExceeded,
                start,
                position,
            ));
        }
    }
    tokens.push(Token {
        kind: TokenKind::End,
        span: Span {
            start: source.len(),
            end: source.len(),
        },
    });
    Ok(tokens)
}

/// Excel keeps at most 15 significant decimal digits in entered numeric constants.
/// Preserve the exponent and decimal point, zeroing only excess mantissa digits.
fn excel_significant_digit_limit(text: &str) -> Option<String> {
    let mantissa = text
        .split_once(['e', 'E'])
        .map_or(text, |(mantissa, _)| mantissa);
    let first_significant = mantissa
        .bytes()
        .position(|byte| byte.is_ascii_digit() && byte != b'0')?;
    let mut significant_digits = 0_usize;
    let mut normalized = None::<Vec<u8>>;
    for (index, byte) in mantissa.bytes().enumerate().skip(first_significant) {
        if !byte.is_ascii_digit() {
            continue;
        }
        significant_digits += 1;
        if significant_digits > 15 {
            let bytes = normalized.get_or_insert_with(|| text.as_bytes().to_vec());
            bytes[index] = b'0';
        }
    }
    normalized.and_then(|bytes| String::from_utf8(bytes).ok())
}
