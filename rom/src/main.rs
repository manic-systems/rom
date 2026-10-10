//! The `rom` command-line entry point.

use std::process::ExitCode;

fn main() -> ExitCode {
  match rom::run() {
    Ok(code) => code,
    Err(report) => {
      eprintln!("{report:?}");
      ExitCode::FAILURE
    },
  }
}
