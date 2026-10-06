//! Same bounded protocol and parser, compiled as a standalone WASI command.
fn main() -> std::process::ExitCode {
    let result = spreadsheet_xlsx::worker_protocol::run(
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
    );
    if result.is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
