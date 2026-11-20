use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_frob");

fn help(args: &[&str]) -> String {
    let output = Command::new(BIN)
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to invoke frob");
    assert!(output.status.success(), "{args:?}: {:?}", output.status);
    String::from_utf8(output.stdout).unwrap()
}

/// Agents new to frob read `--help` first and keep it in context for every later turn.
#[test]
fn top_level_help_is_a_short_primer_and_all_is_the_full_reference() {
    let brief = help(&["--help"]);
    assert!(brief.contains("Usage:") && brief.contains("snapshot -i"));
    assert!(brief.len() < 2500, "--help grew to {} bytes", brief.len());

    let full = help(&["--help", "--all"]);
    assert!(full.len() > brief.len() * 5);
    for section in ["Core Commands:", "Environment:", "Snapshot Options:"] {
        assert!(full.contains(section), "--help --all lacks {section}");
    }
}
