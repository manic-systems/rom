//! The `rom` command-line entry point.

use std::process::ExitCode;

fn main() -> misstep::Result<ExitCode> {
  rom::run()
}
