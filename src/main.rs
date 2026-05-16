//! Process entry point for the Piers CLI.
//!
use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

mod app;
mod guest_component;
mod harness;
mod provider;
mod runtime;
mod session;
mod staging;
mod tools;

fn main() -> Result<()> {
    init_tracing();

    let root = std::env::current_dir().context("get current directory")?;
    app::run(std::env::args().skip(1), root)
}

/// Installs process-wide tracing with `RUST_LOG` support.
///
/// The default filter is `warn` so ordinary REPL output stays readable unless a
/// caller opts into host diagnostics.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
