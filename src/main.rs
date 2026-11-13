use eidoryn_psx::app::BINARY_NAME;

mod cli;
#[cfg(feature = "desktop")]
mod desktop;

fn main() -> std::process::ExitCode {
    match cli::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{BINARY_NAME}: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
