use proptest::prelude::*;
use spreadsheet_domain::workbook::{CellAddress, ColumnId, RowId, SheetId};
use spreadsheet_formula::range_index::{RangeDependency, RangeIndex};
use std::collections::BTreeSet;

fn identity() -> Result<CellAddress, Box<dyn std::error::Error>> {
    Ok(CellAddress {
        sheet: SheetId::from_bytes([1; 16])?,
        row: RowId::from_bytes([2; 16])?,
        column: ColumnId::from_bytes([3; 16])?,
    })
}

proptest! {
    #[test]
    fn lookup_matches_rectangle_scan(
        rectangles in prop::collection::vec((0_u32..100, 0_u32..100, 0_u32..40, 0_u32..40), 0..200),
        row in 0_u32..120, column in 0_u32..50
    ) {
        let address = identity().map_err(|_| TestCaseError::fail("test identity"))?;
        let ranges: Vec<_> = rectangles.into_iter().enumerate().map(|(index,(r1,r2,c1,c2))| Ok(RangeDependency {
            formula: CellAddress { row: RowId::from_bytes((index as u128 + 1).to_be_bytes())
                .map_err(|_| TestCaseError::fail("formula identity"))?, ..address }, sheet: address.sheet,
            first_row: r1.min(r2), last_row: r1.max(r2),
            first_column: c1.min(c2), last_column: c1.max(c2),
        })).collect::<Result<_, TestCaseError>>()?;
        let expected: BTreeSet<_> = ranges.iter().filter(|r| r.first_row <= row && row <= r.last_row
            && r.first_column <= column && column <= r.last_column).map(|r| r.formula).collect();
        let index = RangeIndex::build(ranges).map_err(|_| TestCaseError::fail("build"))?;
        let actual = index.affected(address.sheet, row, column, &mut 1000)
            .map_err(|_| TestCaseError::fail("query budget"))?;
        prop_assert_eq!(actual, expected);
    }
}

#[test]
fn unrelated_intervals_are_pruned_and_invalid_geometry_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let address = identity()?;
    let ranges = (0..10_000).map(|row| RangeDependency {
        formula: address,
        sheet: address.sheet,
        first_row: row * 2,
        last_row: row * 2,
        first_column: 0,
        last_column: 1,
    });
    let index = RangeIndex::build(ranges)?;
    let result = index.affected(address.sheet, 1000, 0, &mut 40)?;
    assert_eq!(result, BTreeSet::from([address]));
    assert!(index.affected(address.sheet, 1000, 0, &mut 0).is_err());
    assert!(
        RangeIndex::build([RangeDependency {
            formula: address,
            sheet: address.sheet,
            first_row: 4,
            last_row: 3,
            first_column: 0,
            last_column: 1
        }])
        .is_err()
    );
    Ok(())
}
