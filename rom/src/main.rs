//! The `rom` command-line entry point.

use std::process::ExitCode;

#[expect(
  clippy::print_stderr,
  clippy::use_debug,
  reason = "misstep renders its report through Debug"
)]
fn main() -> ExitCode {
  match rom::run() {
    Ok(code) => code,
    Err(report) => {
      eprintln!("{report:?}");
      ExitCode::FAILURE
    },
  }
}
