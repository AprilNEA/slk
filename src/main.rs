//! Entry point for the slk command-line application.

#[tokio::main]
async fn main() -> std::process::ExitCode {
    slk::run().await
}
