//! Personal Slack workflows for the slk command-line application.

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("slk supports macOS, Linux, and Windows");

mod app;
mod auth;
mod cli;
mod error;
mod ids;
mod messages;
mod model;
mod output;
mod platform;
mod scheduled;
mod slack;
#[cfg(test)]
mod testing;
mod update;

const APP_ID: &str = env!("CARGO_PKG_NAME");

fn initialize_tls() {
    // Rustls shares one provider across the SDK and file transfers; preserve an installed provider.
    match rustls::crypto::ring::default_provider().install_default() {
        Ok(()) | Err(_) => (),
    }
}

pub use cli::run;
