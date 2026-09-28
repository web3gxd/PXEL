//! Stable explicit encoding; never Rust layout, Debug, platform widths or protobuf.
use super::*;
use sha2::{Digest, Sha256};

fn text(hash: &mut Sha256, value: &str) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value.as_bytes());
}
fn mode(hash: &mut Sha256, value: AxisReferenceMode) {
    match value {
        AxisReferenceMode::Absolute => hash.update([0]),
        AxisReferenceMode::Relative { original_delta } => {
            hash.update([1]);
            hash.update(original_delta.to_be_bytes());
        }
    }
}
fn reference(hash: &mut Sha256, value: &BoundCellReference) {
    hash.update(value.target_sheet.as_bytes());
    hash.update(value.target_row.as_bytes());
    hash.update(value.target_column.as_bytes());
    mode(hash, value.row_mode);
    mode(hash, value.column_mode);
}
pub(super) fn semantic_hash(formula: &Formula) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"PXEL-FORMULA-1\0");
    text(&mut hash, formula.source());
    text(&mut hash, formula.canonical());
    hash.update(formula.ast().schema_version().to_be_bytes());
    text(&mut hash, formula.profile().identifier());
    let origin = formula.origin();
    hash.update(origin.sheet.as_bytes());
    hash.update(origin.row.as_bytes());
    hash.update(origin.column.as_bytes());
    hash.update(formula.bound_at_version().get().to_be_bytes());
    hash.update(formula.ast().root().0.to_be_bytes());
    hash.update((formula.ast().nodes().len() as u64).to_be_bytes());
    for node in formula.ast().nodes() {
        match node {
            Node::Number(number) => {
                hash.update([0]);
                hash.update(number.to_bits().to_be_bytes());
            }
            Node::Text(value) => {
                hash.update([1]);
                text(&mut hash, value);
            }
            Node::Boolean(value) => hash.update([2, u8::from(*value)]),
            Node::Error(value) => {
                hash.update([3]);
                text(&mut hash, value.code());
            }
            Node::Missing => hash.update([4]),
            Node::Cell(value) => {
                hash.update([5]);
                reference(&mut hash, value);
            }
            Node::Range(value) => {
                hash.update([6]);
                reference(&mut hash, &value.start);
                reference(&mut hash, &value.end);
            }
            Node::InvalidReference(value) => hash.update([
                7,
                match value {
                    ReferenceError::DeletedTarget => 0,
                    ReferenceError::OutOfBounds => 1,
                    ReferenceError::MissingSheet => 2,
                },
            ]),
            Node::Unary { op, operand } => {
                hash.update([
                    8,
                    match op {
                        UnaryOp::Plus => 0,
                        UnaryOp::Minus => 1,
                        UnaryOp::Percent => 2,
                    },
                ]);
                hash.update(operand.0.to_be_bytes());
            }
            Node::Binary { op, left, right } => {
                hash.update([
                    9,
                    match op {
                        BinaryOp::Add => 0,
                        BinaryOp::Subtract => 1,
                        BinaryOp::Multiply => 2,
                        BinaryOp::Divide => 3,
                        BinaryOp::Power => 4,
                        BinaryOp::Concat => 5,
                        BinaryOp::Equal => 6,
                        BinaryOp::NotEqual => 7,
                        BinaryOp::Less => 8,
                        BinaryOp::LessEqual => 9,
                        BinaryOp::Greater => 10,
                        BinaryOp::GreaterEqual => 11,
                    },
                ]);
                hash.update(left.0.to_be_bytes());
                hash.update(right.0.to_be_bytes());
            }
            Node::Call { name, arguments } => {
                hash.update([10]);
                text(&mut hash, name.as_str());
                hash.update((arguments.len() as u64).to_be_bytes());
                for argument in arguments {
                    hash.update(argument.0.to_be_bytes());
                }
            }
        }
    }
    hash.finalize().into()
}
