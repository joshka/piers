//! Line-oriented entry point for the Piers harness PoC.

use std::io::{self, Write};

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

mod guest_component;
mod harness;
mod runtime;
mod staging;

use harness::Harness;

fn main() -> Result<()> {
    init_tracing();

    let root = std::env::current_dir().context("get current directory")?;
    let mut harness = Harness::load(root)?;

    println!("piers harness");
    println!("commands: :evolve <spec>, :reload, :status, :quit");

    let stdin = io::stdin();
    loop {
        print!("piers> ");
        io::stdout().flush().context("flush prompt")?;

        let mut line = String::new();
        if stdin.read_line(&mut line).context("read input")? == 0 {
            break;
        }

        let line = line.trim_end();
        if line == ":quit" {
            break;
        } else if line == ":status" {
            println!("{}", harness.status());
        } else if line == ":reload" {
            harness.reload()?;
            println!("reloaded generation {}", harness.generation());
        } else if let Some(spec) = line.strip_prefix(":evolve") {
            harness.evolve(spec.trim())?;
            println!("evolved and reloaded generation {}", harness.generation());
        } else if line.is_empty() {
            continue;
        } else {
            println!("{}", harness.handle(line)?);
        }
    }

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
