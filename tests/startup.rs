use std::{fs, process::Command};

fn rex_kiosk() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rex-kiosk"));
    for name in ["REX_BIN_DIR", "REX_SESSION", "REX_BLOCK"] {
        command.env_remove(name);
    }
    command
}

#[test]
fn invalid_config_exits_nonzero_with_a_clear_error() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("rex-kiosk")).unwrap();
    fs::write(
        temp.path().join("rex-kiosk/config.toml"),
        "search_dirs = 42",
    )
    .unwrap();

    let output = rex_kiosk()
        .env("XDG_CONFIG_HOME", temp.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("rex-kiosk: "), "stderr was: {stderr}");
    assert!(
        stderr.contains("invalid config TOML"),
        "stderr was: {stderr}"
    );
    assert!(!stderr.contains("panicked"), "stderr was: {stderr}");
}

#[test]
fn launcher_options_outside_a_rex_terminal_exit_with_a_clear_error() {
    let temp = tempfile::tempdir().unwrap();

    let output = rex_kiosk()
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["--block", "block:1"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("rex-kiosk: --client and --block need a Rex terminal"),
        "stderr was: {stderr}"
    );
}

#[test]
fn help_and_version_print_without_starting_the_picker() {
    let help = rex_kiosk().arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).starts_with("usage: rex-kiosk"));

    let version = rex_kiosk().arg("-V").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        format!("rex-kiosk {}\n", env!("CARGO_PKG_VERSION"))
    );

    let bad = rex_kiosk().arg("--nope").output().unwrap();
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unexpected argument \"--nope\""));
}
