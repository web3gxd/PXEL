use spreadsheet_domain::workbook::*;
use spreadsheet_formula::{
    binder::bind,
    bound::evaluate_bound,
    calculation::{COMPATIBILITY_PROFILE, ENGINE_VERSION},
    *,
};
use std::collections::BTreeMap;
fn book(
    epoch: u64,
    timestamp: i64,
    offset: i16,
) -> Result<(Workbook, CellAddress), Box<dyn std::error::Error>> {
    let document = DocumentId::from_bytes([3; 16])?;
    let cell = CellAddress {
        sheet: SheetId::from_bytes([1; 16])?,
        row: RowId::from_bytes([2; 16])?,
        column: ColumnId::from_bytes([5; 16])?,
    };
    let seed = SheetSeed {
        id: cell.sheet,
        name: "Sheet1".into(),
        rows: vec![cell.row],
        columns: vec![ColumnId::from_bytes([4; 16])?, cell.column],
    };
    let context = CalculationContext::new(
        CalculationContextId::from_bytes([6; 16])?,
        epoch,
        timestamp,
        offset,
        i32::try_from((timestamp + i64::from(offset) * 60000).div_euclid(86400000))?,
        [7; 32],
        ENGINE_VERSION.into(),
        COMPATIBILITY_PROFILE.into(),
        CalculationReason::Explicit,
    )?;
    Ok((
        Workbook::hydrate_sheets_with_context(
            document,
            DocumentVersion::new(3)?,
            [seed],
            [],
            context,
        )?,
        cell,
    ))
}
fn eval(
    source: &str,
    book: &Workbook,
    cell: CellAddress,
) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(evaluate_bound(
        &bind(source, book, cell)?,
        book,
        &BTreeMap::new(),
        &mut EvaluationBudget::default(),
    )?)
}
#[test]
fn persisted_instant_and_cell_randomness() -> Result<(), Box<dyn std::error::Error>> {
    let (book, cell) = book(1, 86400000, 180)?;
    assert_eq!(eval("=TODAY()", &book, cell)?, Value::Number(25570.0));
    assert_eq!(eval("=NOW()", &book, cell)?, Value::Number(25570.125));
    let first = eval("=RAND()", &book, cell)?;
    assert!(matches!(first,Value::Number(n) if (0.0..1.0).contains(&n)));
    assert_eq!(eval("=RAND()", &book, cell)?, first);
    // Independently derived with Python hashlib/struct, ADR-065 byte layout.
    assert!(matches!(first, Value::Number(n) if n.to_bits() == 0x3feafe71ac28de77));
    assert_eq!(eval("=RAND()=RAND()", &book, cell)?, Value::Boolean(false));
    Ok(())
}

struct ContextResolver<'a> {
    context: EvaluationContext<'a>,
    system: DateSystem,
}
impl ReferenceResolver for ContextResolver<'_> {
    fn evaluation_context(&self) -> Option<EvaluationContext<'_>> {
        Some(self.context)
    }
    fn date_system(&self) -> DateSystem {
        self.system
    }
    fn resolve_cell(&self, reference: &CellReference) -> Value {
        NoReferences.resolve_cell(reference)
    }
    fn visit_range(
        &self,
        reference: &RangeReference,
        visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
    ) -> Result<(), EvaluationError> {
        NoReferences.visit_range(reference, visitor)
    }
}

#[test]
fn calendar_profiles_offsets_and_final_millisecond() -> Result<(), Box<dyn std::error::Error>> {
    for (timestamp, offset, day, fraction) in [
        (0, -1, -1, 86340000.0 / 86400000.0),
        (0, 840, 0, 14.0 / 24.0),
        (86400000, -840, 0, 10.0 / 24.0),
        (86400000 - 1, 0, 0, 86399999.0 / 86400000.0),
        (86400000, 0, 1, 0.0),
    ] {
        let (book, cell) = book(1, timestamp, offset)?;
        for (system, origin) in [
            (DateSystem::Excel1900, 25569.0),
            (DateSystem::Excel1904, 24107.0),
        ] {
            let resolver = ContextResolver {
                context: EvaluationContext {
                    calculation: book
                        .calculation_context()
                        .ok_or("missing fixture context")?,
                    document_id: book.document_id(),
                    formula_cell: cell,
                },
                system,
            };
            for (source, expected) in [
                ("=TODAY()", origin + f64::from(day)),
                ("=NOW()", origin + f64::from(day) + fraction),
            ] {
                assert_eq!(
                    evaluate(
                        &parse(source)?.ast,
                        &resolver,
                        &mut EvaluationBudget::default()
                    )?,
                    Value::Number(expected),
                    "{source} {timestamp} {offset} {system:?}"
                );
            }
        }
    }
    let last_day = 2958465_i64 - 25569;
    let (last, cell) = book(1, (last_day + 1) * 86400000 - 1, 0)?;
    assert_eq!(eval("=TODAY()", &last, cell)?, Value::Number(2958465.0));
    assert!(
        matches!(eval("=NOW()", &last, cell)?, Value::Number(n) if (2958465.0..2958466.0).contains(&n))
    );
    let (outside, cell) = book(1, (last_day + 1) * 86400000, 0)?;
    for source in ["=NOW()", "=TODAY()"] {
        assert_eq!(
            eval(source, &outside, cell)?,
            Value::Error(FormulaError::Num)
        );
    }
    Ok(())
}

#[test]
fn context_arity_lazy_branches_and_resource_errors() -> Result<(), Box<dyn std::error::Error>> {
    for name in ["NOW", "TODAY", "RAND"] {
        assert_eq!(
            evaluate(
                &parse(&format!("={name}(1)"))?.ast,
                &NoReferences,
                &mut EvaluationBudget::default()
            )?,
            Value::Error(FormulaError::Value)
        );
        assert_eq!(
            evaluate(
                &parse(&format!("={name}()"))?.ast,
                &NoReferences,
                &mut EvaluationBudget::default()
            ),
            Err(EvaluationError::UnsupportedCompatibilityFeature)
        );
    }
    assert_eq!(
        evaluate(
            &parse("=IF(FALSE,RAND(),7)")?.ast,
            &NoReferences,
            &mut EvaluationBudget::default()
        )?,
        Value::Number(7.0)
    );
    assert_eq!(
        evaluate(
            &parse("=IFERROR(RAND(),7)")?.ast,
            &NoReferences,
            &mut EvaluationBudget::default()
        ),
        Err(EvaluationError::UnsupportedCompatibilityFeature)
    );
    let (book, cell) = book(1, 0, 0)?;
    let formula = bind("=IFERROR(RAND(),7)", &book, cell)?;
    assert_eq!(
        evaluate_bound(
            &formula,
            &book,
            &BTreeMap::new(),
            &mut EvaluationBudget::new(20)
        ),
        Err(EvaluationError::CalculationLimitExceeded)
    );
    let mut cancelled = EvaluationBudget::default();
    cancelled.cancel();
    assert_eq!(
        evaluate_bound(&formula, &book, &BTreeMap::new(), &mut cancelled),
        Err(EvaluationError::Cancelled)
    );
    Ok(())
}

#[test]
fn randomness_is_scoped_to_epoch_and_stable_cell() -> Result<(), Box<dyn std::error::Error>> {
    let (first, cell) = book(1, 0, 0)?;
    let (second, _) = book(2, 0, 0)?;
    assert_ne!(
        eval("=RAND()", &first, cell)?,
        eval("=RAND()", &second, cell)?
    );
    let other = CellAddress {
        column: ColumnId::from_bytes([4; 16])?,
        ..cell
    };
    assert_ne!(
        eval("=RAND()", &first, cell)?,
        eval("=RAND()", &first, other)?
    );
    let (different_time, _) = book(1, 86400000, 180)?;
    assert_eq!(
        eval("=RAND()", &first, cell)?,
        eval("=RAND()", &different_time, cell)?
    );
    Ok(())
}

#[test]
fn incompatible_contexts_fail_and_seed_document_separate_randomness()
-> Result<(), Box<dyn std::error::Error>> {
    let (book, cell) = book(1, 0, 0)?;
    let base = book
        .calculation_context()
        .ok_or("missing fixture context")?;
    let random = parse("=RAND()")?.ast;
    let baseline = eval("=RAND()", &book, cell)?;
    for (engine, profile, seed) in [
        ("old-engine", COMPATIBILITY_PROFILE, [7; 32]),
        (ENGINE_VERSION, "unknown-profile", [7; 32]),
        (ENGINE_VERSION, COMPATIBILITY_PROFILE, [8; 32]),
    ] {
        let calculation = CalculationContext::new(
            base.context_id(),
            1,
            0,
            0,
            0,
            seed,
            engine.into(),
            profile.into(),
            CalculationReason::Explicit,
        )?;
        let resolver = ContextResolver {
            context: EvaluationContext {
                calculation: &calculation,
                document_id: book.document_id(),
                formula_cell: cell,
            },
            system: DateSystem::Excel1900,
        };
        let result = evaluate(&random, &resolver, &mut EvaluationBudget::default());
        if seed == [8; 32] {
            assert_ne!(result?, baseline);
        } else {
            assert_eq!(
                result,
                Err(EvaluationError::UnsupportedCompatibilityFeature)
            );
        }
    }
    let resolver = ContextResolver {
        context: EvaluationContext {
            calculation: base,
            document_id: DocumentId::from_bytes([9; 16])?,
            formula_cell: cell,
        },
        system: DateSystem::Excel1900,
    };
    assert_ne!(
        evaluate(&random, &resolver, &mut EvaluationBudget::default())?,
        baseline
    );
    Ok(())
}
