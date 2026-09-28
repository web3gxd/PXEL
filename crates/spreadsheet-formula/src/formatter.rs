use crate::ast::*;

pub fn canonical(ast: &Ast) -> Result<String, ParseError> {
    if ast.schema_version != AST_SCHEMA_VERSION || ast.nodes.len() > MAX_NODES {
        return Err(error());
    }
    let mut output = "=".to_owned();
    let mut budget = MAX_NODES * 4;
    print(ast, ast.root, 0, &mut budget, &mut output)?;
    Ok(output)
}
fn error() -> ParseError {
    ParseError::at(ParseErrorCode::InvalidAst, 0, 0)
}
fn append(output: &mut String, text: &str) -> Result<(), ParseError> {
    if output.len().saturating_add(text.len()) > MAX_SOURCE_BYTES {
        return Err(ParseError::at(ParseErrorCode::LimitExceeded, 0, 0));
    }
    output.push_str(text);
    Ok(())
}
fn reference(reference: &CellReference, show_sheet: bool) -> Result<String, ParseError> {
    if reference.row >= 1_048_576 || reference.column >= 16_384 {
        return Err(error());
    }
    let mut value = String::new();
    if show_sheet && let Some(sheet) = &reference.sheet {
        if sheet.len() > MAX_SOURCE_BYTES {
            return Err(error());
        }
        value.push('\'');
        value.push_str(&sheet.replace('\'', "''"));
        value.push_str("'!");
    }
    if reference.column_absolute {
        value.push('$');
    }
    let mut column = reference.column + 1;
    let mut label = Vec::new();
    while column > 0 {
        label.push(char::from(b'A' + ((column - 1) % 26) as u8));
        column = (column - 1) / 26;
    }
    value.extend(label.iter().rev());
    if reference.row_absolute {
        value.push('$');
    }
    value.push_str(&(reference.row + 1).to_string());
    Ok(value)
}
fn print(
    ast: &Ast,
    id: NodeId,
    depth: usize,
    budget: &mut usize,
    out: &mut String,
) -> Result<(), ParseError> {
    if depth > MAX_DEPTH || *budget == 0 {
        return Err(error());
    }
    *budget -= 1;
    let node = ast.nodes.get(id.0).ok_or_else(error)?;
    match node {
        Node::Number(n) => {
            if !n.is_finite() || *n < 0.0 {
                return Err(error());
            }
            append(out, &if *n == 0.0 { "0".into() } else { n.to_string() })?;
        }
        Node::Text(text) => {
            if text.len() > MAX_SOURCE_BYTES {
                return Err(error());
            }
            append(out, "\"")?;
            append(out, &text.replace('"', "\"\""))?;
            append(out, "\"")?;
        }
        Node::Boolean(b) => append(out, if *b { "TRUE" } else { "FALSE" })?,
        Node::Error(e) => append(out, e.code())?,
        Node::InvalidReference => append(out, FormulaError::Ref.code())?,
        Node::Missing => (),
        Node::Cell(cell) => append(out, &reference(cell, true)?)?,
        Node::Range(range) => {
            if range.start.sheet != range.end.sheet || range.cell_count() > MAX_RANGE_CELLS {
                return Err(error());
            }
            append(out, &reference(&range.start, true)?)?;
            append(out, ":")?;
            append(out, &reference(&range.end, false)?)?;
        }
        Node::Unary { op, operand } => {
            append(out, "(")?;
            if *op != UnaryOp::Percent {
                append(out, if *op == UnaryOp::Plus { "+" } else { "-" })?;
            }
            print(ast, *operand, depth + 1, budget, out)?;
            if *op == UnaryOp::Percent {
                append(out, "%")?;
            }
            append(out, ")")?;
        }
        Node::Binary { op, left, right } => {
            append(out, "(")?;
            print(ast, *left, depth + 1, budget, out)?;
            append(
                out,
                match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Subtract => "-",
                    BinaryOp::Multiply => "*",
                    BinaryOp::Divide => "/",
                    BinaryOp::Power => "^",
                    BinaryOp::Concat => "&",
                    BinaryOp::Equal => "=",
                    BinaryOp::NotEqual => "<>",
                    BinaryOp::Less => "<",
                    BinaryOp::LessEqual => "<=",
                    BinaryOp::Greater => ">",
                    BinaryOp::GreaterEqual => ">=",
                },
            )?;
            print(ast, *right, depth + 1, budget, out)?;
            append(out, ")")?;
        }
        Node::Call { name, arguments } => {
            if name.is_empty()
                || name.len() > MAX_SOURCE_BYTES
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
                || arguments.len() > 255
            {
                return Err(error());
            }
            append(out, &name.to_ascii_uppercase())?;
            append(out, "(")?;
            for (index, arg) in arguments.iter().enumerate() {
                if index > 0 {
                    append(out, ",")?;
                }
                print(ast, *arg, depth + 1, budget, out)?;
            }
            append(out, ")")?;
        }
    }
    Ok(())
}
