//! Clock and entropy sampling live only at the server application boundary.
use spreadsheet_collab::SessionError;
use spreadsheet_domain::workbook::{
    CalculationContext, CalculationContextId, CalculationReason, OperationId,
};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub fn sample(
    reason: CalculationReason,
) -> Result<(OperationId, CalculationContext), SessionError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .ok_or(SessionError::Unavailable)?;
    let day =
        i32::try_from(timestamp.div_euclid(86400000)).map_err(|_| SessionError::Unavailable)?;
    let mut seed = [0; 32];
    seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    let context = CalculationContext::new(
        CalculationContextId::from_bytes(*Uuid::now_v7().as_bytes())?,
        1,
        timestamp,
        0,
        day,
        seed,
        spreadsheet_collab::calculation::ENGINE_VERSION.into(),
        spreadsheet_collab::calculation::COMPATIBILITY_PROFILE.into(),
        reason,
    )?;
    Ok((
        OperationId::from_bytes(*Uuid::now_v7().as_bytes())?,
        context,
    ))
}
