//! ADR-065: pure persisted-context evaluation. No clock or mutable RNG.
use crate::{
    DateSystem, FormulaError, NodeId, ReferenceResolver, Value,
    calculation::{COMPATIBILITY_PROFILE, ENGINE_VERSION},
    evaluator::{EvaluationBudget, EvaluationError},
};
use sha2::{Digest, Sha256};
use spreadsheet_domain::workbook::{CalculationContext, CellAddress, DocumentId};

#[derive(Clone, Copy)]
pub struct EvaluationContext<'a> {
    pub calculation: &'a CalculationContext,
    pub document_id: DocumentId,
    pub formula_cell: CellAddress,
}
pub(crate) fn call(
    name: &str,
    args: &[NodeId],
    node: NodeId,
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
) -> Result<Value, EvaluationError> {
    if !args.is_empty() {
        return Ok(Value::Error(FormulaError::Value));
    }
    let context = resolver
        .evaluation_context()
        .ok_or(EvaluationError::UnsupportedCompatibilityFeature)?;
    let calculation = context.calculation;
    if calculation.engine_version() != ENGINE_VERSION
        || calculation.compatibility_profile() != COMPATIBILITY_PROFILE
    {
        return Err(EvaluationError::UnsupportedCompatibilityFeature);
    }
    if name == "RAND" {
        for _ in 0..64 {
            budget.charge()?;
        }
        let node = u64::try_from(node.0).map_err(|_| EvaluationError::InvalidAst)?;
        let mut hash = Sha256::new();
        hash.update(b"PXEL-RAND\0\x01");
        hash.update(context.document_id.as_bytes());
        hash.update(calculation.epoch().to_be_bytes());
        hash.update(context.formula_cell.sheet.as_bytes());
        hash.update(context.formula_cell.row.as_bytes());
        hash.update(context.formula_cell.column.as_bytes());
        hash.update(node.to_be_bytes());
        hash.update(calculation.random_seed());
        let bytes = hash.finalize();
        let mut head = [0u8; 8];
        head.copy_from_slice(&bytes[..8]);
        let mantissa = u64::from_be_bytes(head) >> 11;
        return Ok(Value::Number(mantissa as f64 * (1.0 / 9007199254740992.0)));
    }
    budget.charge()?;
    let epoch_serial = match resolver.date_system() {
        DateSystem::Excel1900 => 25569,
        DateSystem::Excel1904 => 24107,
    };
    let day = i64::from(calculation.civil_day_since_unix_epoch()) + epoch_serial;
    if !(0..=crate::calendar::maximum(resolver.date_system())).contains(&day) {
        return Ok(Value::Error(FormulaError::Num));
    }
    if name == "TODAY" {
        return Ok(Value::Number(day as f64));
    }
    if name != "NOW" {
        return Err(EvaluationError::InvalidAst);
    }
    let Some(local_ms) = calculation
        .effective_timestamp_utc_ms()
        .checked_add(i64::from(calculation.utc_offset_minutes()) * 60000)
    else {
        return Ok(Value::Error(FormulaError::Num));
    };
    let fraction = local_ms.rem_euclid(86400000) as f64 / 86400000.0;
    Ok(Value::Number(day as f64 + fraction))
}
