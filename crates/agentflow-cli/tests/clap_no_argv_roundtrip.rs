use std::fs;
use std::path::Path;

fn read_source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(path).expect("source file should be readable")
}

#[test]
fn clap_dispatch_does_not_roundtrip_to_argv() {
    let cli_args = read_source("src/cli_args.rs");

    assert!(
        !cli_args.contains("into_args"),
        "clap dispatch must pass typed Args directly instead of serializing back to argv"
    );
    assert!(
        !cli_args.contains(".append("),
        "clap Args structs must not append fields into an argv buffer"
    );
}

#[test]
fn handlers_do_not_call_hand_written_arg_parsers() {
    for relative in [
        "src/lib.rs",
        "src/agent_commands.rs",
        "src/agent_ops_commands.rs",
        "src/synth_commands.rs",
    ] {
        let source = read_source(relative);

        assert!(
            !source.contains("next_arg("),
            "{relative} must not consume argv with next_arg"
        );
        assert!(
            !source.contains("require_value("),
            "{relative} must not consume argv with require_value"
        );
    }
}
