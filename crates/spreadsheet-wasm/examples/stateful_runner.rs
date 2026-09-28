//! Native differential harness; stdout is fixture output, never application logging.
use serde::Deserialize;
use std::io::{self, Read};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    init: serde_json::Value,
    patches: Vec<serde_json::Value>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const CAP: usize = 64 * 1024 * 1024;
    let mut input = String::new();
    io::stdin()
        .take(CAP as u64 + 1)
        .read_to_string(&mut input)?;
    if input.len() > CAP {
        return Err("harness input limit".into());
    }
    let requests: Vec<Request> = serde_json::from_str(&input)?;
    let mut results = Vec::with_capacity(requests.len());
    for request in requests {
        let mut workbook =
            spreadsheet_wasm::PreviewWorkbook::from_json(&serde_json::to_string(&request.init)?)?;
        let mut steps = vec![serde_json::from_str::<serde_json::Value>(
            &workbook.calculate_json()?,
        )?];
        for patch in request.patches {
            workbook.apply_json(&serde_json::to_string(&patch)?)?;
            steps.push(serde_json::from_str::<serde_json::Value>(
                &workbook.calculate_json()?,
            )?);
        }
        results.push(steps);
    }
    let output = serde_json::to_string(&results)?;
    if output.len() > CAP {
        return Err("harness output limit".into());
    }
    println!("{output}");
    Ok(())
}
