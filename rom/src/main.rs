use std::process::ExitCode;

fn main() -> ExitCode {
  match rom::run() {
    Ok(()) => ExitCode::SUCCESS,
    Err(report) => {
      eprintln!("{report:?}");
      ExitCode::FAILURE
    },
  }
}
