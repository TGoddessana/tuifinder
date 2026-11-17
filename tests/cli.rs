use std::process::Command;

#[test]
fn version_is_available_without_a_terminal() {
    for flag in ["--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_tuifinder"))
            .arg(flag)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            format!("tuifinder {}", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn unknown_or_extra_flags_fail_before_opening_a_terminal() {
    for args in [
        vec!["--unknown"],
        vec!["--update", "unexpected"],
        vec!["--version", "extra"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_tuifinder"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
}
