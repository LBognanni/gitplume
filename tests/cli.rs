use std::process::Command;

#[test]
fn version_flag_prints_the_package_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_gitplume"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("gitplume {}\n", env!("CARGO_PKG_VERSION"))
    );
}
