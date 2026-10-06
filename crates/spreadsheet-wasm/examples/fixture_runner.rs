//! Native half of the shared native/browser differential harness.
use serde::Deserialize;
use serde_json::value::RawValue;
use std::io::{self, Read};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    source: String,
    input: serde_json::Value,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin()
        .take(16 * 1024 * 1024 + 1)
        .read_to_string(&mut input)?;
    if input.len() > 16 * 1024 * 1024 {
        return Err("fixture runner input too large".into());
    }
    let requests: Vec<Request> = serde_json::from_str(&input)?;
    let results = requests
        .into_iter()
        .map(|request| {
            let value = spreadsheet_wasm::evaluate_fixture(
                &request.source,
                &serde_json::to_string(&request.input)?,
            )?;
            // Preserve the evaluator's exact JSON number spelling through the
            // native harness; parsing and reserializing f64s here can shift a
            // value by one ULP before the browser comparison sees it.
            Ok(serde_json::from_str::<Box<RawValue>>(&value)?)
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    println!("{}", serde_json::to_string(&results)?);
    Ok(())
}
