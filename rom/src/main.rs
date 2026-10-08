//! The `rom` command-line entry point.

use std::process::ExitCode;

fn main() -> eyre::Result<ExitCode> {
  rom::run()
}
