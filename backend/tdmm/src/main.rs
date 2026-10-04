//! Thin binary entry point for the `tdmm` management CLI; all logic lives in
//! the library so integration tests can exercise the same surface.

/// Parses argv, dispatches, and maps the outcome onto the process exit code.
#[tokio::main]
async fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(tdmm::run().await)
}
