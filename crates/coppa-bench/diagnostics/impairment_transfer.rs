//! Seeded multi-frame synthetic impairment measurements. stdout is CSV only.
use clap::Parser;
use coppa_bench::impairment::{csv, run, Options};
use std::io::{self, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse();
    let rows = run(&options).map_err(|e| format!("impairment transfer: {e}"))?;
    io::stdout()
        .lock()
        .write_all(csv(&options, &rows).as_bytes())?;
    Ok(())
}
