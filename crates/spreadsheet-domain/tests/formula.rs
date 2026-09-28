use spreadsheet_domain::{
    formula::*,
    workbook::{CellAddress, ColumnId, DocumentVersion, RowId, SheetId},
};
type ResultTest = Result<(), Box<dyn std::error::Error>>;
fn origin() -> Result<CellAddress, Box<dyn std::error::Error>> {
    Ok(CellAddress {
        sheet: SheetId::from_bytes([1; 16])?,
        row: RowId::from_bytes([2; 16])?,
        column: ColumnId::from_bytes([3; 16])?,
    })
}
fn reference() -> Result<BoundCellReference, Box<dyn std::error::Error>> {
    let origin = origin()?;
    Ok(BoundCellReference {
        target_sheet: origin.sheet,
        target_row: origin.row,
        target_column: origin.column,
        row_mode: AxisReferenceMode::Relative { original_delta: -2 },
        column_mode: AxisReferenceMode::Absolute,
    })
}
#[test]
fn normalized_tree_has_immutable_fields_and_zero_is_canonical() -> ResultTest {
    let ast = BoundAst::new(
        1,
        NodeId(2),
        vec![
            Node::Number(-0.0),
            Node::Number(2.0),
            Node::Binary {
                op: BinaryOp::Add,
                left: NodeId(0),
                right: NodeId(1),
            },
        ],
    )?;
    assert_eq!(ast.schema_version(), 1);
    assert_eq!(ast.root(), NodeId(2));
    assert!(matches!(ast.nodes().first(),Some(Node::Number(n)) if n.to_bits()==0));
    let formula = Formula::new(
        "=-0+2".into(),
        "=0+2".into(),
        ast,
        origin()?,
        DocumentVersion::new(1)?,
    )?;
    assert_eq!(formula.source(), "=-0+2");
    assert_eq!(formula.canonical(), "=0+2");
    assert_eq!(formula.profile().identifier(), "pxel-excel-subset-v1");
    assert_eq!(formula.text_bytes(), 9);
    Ok(())
}
#[test]
fn unknown_schema_invalid_root_forward_edges_orphans_and_shared_nodes_rejected() {
    assert!(matches!(
        BoundAst::new(2, NodeId(0), vec![Node::Missing]),
        Err(FormulaValidationError::UnsupportedSchema)
    ));
    for (root, nodes) in [
        (NodeId(0), vec![Node::Missing, Node::Missing]),
        (NodeId(1), vec![Node::Missing, Node::Missing]),
        (
            NodeId(1),
            vec![
                Node::Unary {
                    op: UnaryOp::Plus,
                    operand: NodeId(1),
                },
                Node::Missing,
            ],
        ),
        (
            NodeId(1),
            vec![
                Node::Missing,
                Node::Unary {
                    op: UnaryOp::Plus,
                    operand: NodeId(1),
                },
            ],
        ),
        (
            NodeId(1),
            vec![
                Node::Missing,
                Node::Binary {
                    op: BinaryOp::Add,
                    left: NodeId(0),
                    right: NodeId(0),
                },
            ],
        ),
    ] {
        assert!(matches!(
            BoundAst::new(1, root, nodes),
            Err(FormulaValidationError::InvalidStructure)
        ));
    }
}
#[test]
fn depth_and_memory_limits_are_checked_without_recursive_descent() -> ResultTest {
    let mut nodes = vec![Node::Number(1.0)];
    for index in 1..128 {
        nodes.push(Node::Unary {
            op: UnaryOp::Plus,
            operand: NodeId(index - 1),
        })
    }
    BoundAst::new(1, NodeId(127), nodes.clone())?;
    nodes.push(Node::Unary {
        op: UnaryOp::Plus,
        operand: NodeId(127),
    });
    assert!(matches!(
        BoundAst::new(1, NodeId(128), nodes),
        Err(FormulaValidationError::LimitExceeded)
    ));
    assert!(matches!(
        BoundAst::new(1, NodeId(4096), vec![Node::Missing; 4097]),
        Err(FormulaValidationError::LimitExceeded)
    ));
    assert!(matches!(
        BoundAst::new(
            1,
            NodeId(0),
            vec![Node::Text("x".repeat(MAX_LITERAL_BYTES + 1))]
        ),
        Err(FormulaValidationError::LimitExceeded)
    ));
    let text = Node::Text("x".repeat(MAX_LITERAL_BYTES));
    let nodes = vec![
        text.clone(),
        text.clone(),
        text,
        Node::Call {
            name: FunctionName::new("SUM".into())?,
            arguments: vec![NodeId(0), NodeId(1), NodeId(2)],
        },
    ];
    assert!(matches!(
        BoundAst::new(1, NodeId(3), nodes),
        Err(FormulaValidationError::LimitExceeded)
    ));
    Ok(())
}
#[test]
fn nonfinite_literals_and_invalid_names_are_rejected() -> ResultTest {
    for n in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(matches!(
            BoundAst::new(1, NodeId(0), vec![Node::Number(n)]),
            Err(FormulaValidationError::NonFiniteNumber)
        ));
    }
    for name in ["", "sum", "1SUM", "ПРОЧЕЕ", "SUM()"] {
        assert!(FunctionName::new(name.into()).is_err());
    }
    assert_eq!(
        FunctionName::new("_XLFN.UNKNOWN_2".into())?.as_str(),
        "_XLFN.UNKNOWN_2"
    );
    Ok(())
}
#[test]
fn bound_references_preserve_copy_modes_and_reject_extreme_delta_and_cross_sheet_ranges()
-> ResultTest {
    let r = reference()?;
    r.validate()?;
    assert!(matches!(
        r.row_mode,
        AxisReferenceMode::Relative { original_delta: -2 }
    ));
    assert_eq!(r.column_mode, AxisReferenceMode::Absolute);
    let mut bad = r;
    bad.row_mode = AxisReferenceMode::Relative {
        original_delta: i64::MIN,
    };
    assert!(matches!(
        bad.validate(),
        Err(FormulaValidationError::InvalidReference)
    ));
    let mut bad = r;
    bad.column_mode = AxisReferenceMode::Relative {
        original_delta: 16_384,
    };
    assert!(bad.validate().is_err());
    let mut other = r;
    other.target_sheet = SheetId::from_bytes([4; 16])?;
    assert!(
        BoundRangeReference {
            start: r,
            end: other
        }
        .validate()
        .is_err()
    );
    BoundAst::new(
        1,
        NodeId(0),
        vec![Node::Range(BoundRangeReference { start: r, end: r })],
    )?;
    Ok(())
}
#[test]
fn formula_source_limits_and_debug_redaction_are_enforced() -> ResultTest {
    let literal = "private-value-123";
    let ast = BoundAst::new(
        1,
        NodeId(1),
        vec![
            Node::Text(literal.into()),
            Node::Call {
                name: FunctionName::new("PRIVATE_FUNCTION".into())?,
                arguments: vec![NodeId(0)],
            },
        ],
    )?;
    let formula = Formula::new(
        "=PRIVATE_FUNCTION(\"private-value-123\")".into(),
        "=PRIVATE_FUNCTION(\"private-value-123\")".into(),
        ast.clone(),
        origin()?,
        DocumentVersion::new(1)?,
    )?;
    let debug = format!(
        "{formula:?} {ast:?} {:?} {:?}",
        ast.nodes(),
        FunctionName::new("PRIVATE_FUNCTION".into())?
    );
    assert!(!debug.contains(literal));
    assert!(!debug.contains("PRIVATE_FUNCTION"));
    for source in ["".to_string(), "1+2".to_string(), "=".to_string()] {
        assert!(matches!(
            Formula::new(
                source,
                "=1".into(),
                ast.clone(),
                origin()?,
                DocumentVersion::ZERO
            ),
            Err(FormulaValidationError::InvalidSource)
        ));
    }
    assert!(matches!(
        Formula::new(
            format!("={}", "1".repeat(MAX_SOURCE_BYTES)),
            "=1".into(),
            ast.clone(),
            origin()?,
            DocumentVersion::ZERO
        ),
        Err(FormulaValidationError::LimitExceeded)
    ));
    assert!(matches!(
        Formula::new(
            "=1".into(),
            format!("={}", "1".repeat(MAX_CANONICAL_BYTES)),
            ast,
            origin()?,
            DocumentVersion::ZERO
        ),
        Err(FormulaValidationError::LimitExceeded)
    ));
    Ok(())
}
