use std::process::Command;

#[test]
fn version_reports_the_bootstrap_compiler() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("--version")
        .output()
        .expect("run chimi --version");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "chimi 0.0.0-bootstrap\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn check_reaches_the_explicit_m0_frontend_boundary() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["check", "program.chimi"])
        .output()
        .expect("run chimi check");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "chimi: frontend unavailable for program.chimi\n"
    );
}

#[test]
fn invalid_cli_input_is_rejected_at_the_boundary() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["check", "one.chimi", "two.chimi"])
        .output()
        .expect("run invalid chimi check");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "chimi: check accepts exactly one source path\nusage: chimi --version | chimi check <path>\n"
    );
}
