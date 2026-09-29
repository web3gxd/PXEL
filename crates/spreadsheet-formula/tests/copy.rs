use spreadsheet_formula::{ast::Node, copy_formula_text, parse};

#[test]
fn copy_shifts_only_relative_axes_and_preserves_sheet_names()
-> Result<(), Box<dyn std::error::Error>> {
    let text = copy_formula_text("=A1+$A$1+A$1+$A1+'Other sheet'!B2", (2, 2), (4, 3))?;
    let parsed = parse(&text)?;
    let refs = parsed
        .ast
        .nodes
        .iter()
        .filter_map(|node| match node {
            Node::Cell(r) => Some(r),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(refs.len(), 5);
    assert_eq!((refs[0].row, refs[0].column), (2, 1));
    assert_eq!((refs[1].row, refs[1].column), (0, 0));
    assert_eq!((refs[2].row, refs[2].column), (0, 1));
    assert_eq!((refs[3].row, refs[3].column), (2, 0));
    assert_eq!((refs[4].row, refs[4].column), (3, 2));
    assert_eq!(refs[4].sheet.as_deref(), Some("Other sheet"));
    Ok(())
}

#[test]
fn copy_ranges_and_out_of_bounds_become_ref() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(copy_formula_text("=A1+$B$1", (0, 2), (1, 3))?, "=(B2+$B$1)");
    let text = copy_formula_text("=SUM(A1:B2)", (0, 0), (1, 1))?;
    let parsed = parse(&text)?;
    assert!(parsed.ast.nodes.iter().any(|node| matches!(node, Node::Range(range) if range.start.row == 1 && range.start.column == 1 && range.end.row == 2 && range.end.column == 2)));
    assert_eq!(copy_formula_text("=A1", (1, 1), (0, 0))?, "=#REF!");
    assert_eq!(copy_formula_text("=$A$1", (1, 1), (0, 0))?, "=$A$1");
    assert_eq!(copy_formula_text("=XFD1048576", (0, 0), (0, 1))?, "=#REF!");
    Ok(())
}
