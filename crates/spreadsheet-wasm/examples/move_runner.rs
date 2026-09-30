//! Native differential harness. Authored fixture output is never application logging.
use serde::Deserialize;
use serde_json::value::RawValue;
use std::io::{self, Read};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    init: serde_json::Value,
    moves: Vec<serde_json::Value>,
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
    let mut output_bytes = 0usize;
    for request in requests {
        let mut book =
            spreadsheet_wasm::PreviewWorkbook::from_json(&serde_json::to_string(&request.init)?)?;
        let initial = book.calculate_json()?;
        output_bytes = output_bytes.saturating_add(initial.len());
        if output_bytes > CAP {
            return Err("harness output limit".into());
        }
        let mut steps = vec![RawValue::from_string(initial)?];
        for command in request.moves {
            let output = book.move_json(&serde_json::to_string(&command)?)?;
            output_bytes = output_bytes.saturating_add(output.len());
            if output_bytes > CAP {
                return Err("harness output limit".into());
            }
            steps.push(RawValue::from_string(output)?);
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
