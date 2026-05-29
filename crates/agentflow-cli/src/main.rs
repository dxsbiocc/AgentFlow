use std::process::ExitCode;

fn main() -> ExitCode {
    match agentflow_cli::run(std::env::args_os()) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", error.message());
            ExitCode::FAILURE
        }
    }
}
