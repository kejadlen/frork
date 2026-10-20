#![allow(clippy::unwrap_used)]

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt as _;
use predicates::str::contains;

fn cmd() -> Command {
    cargo_bin_cmd!("frork")
}

#[test]
fn shows_help() {
    cmd().arg("--help").assert().success();
}

#[test]
fn no_args_shows_help() {
    cmd().assert().success().stdout(contains("Usage"));
}

#[test]
fn generates_bash_completions() {
    cmd()
        .args(["--completions", "bash"])
        .assert()
        .success()
        .stdout(contains("frork"));
}

#[test]
fn generates_zsh_completions() {
    cmd()
        .args(["--completions", "zsh"])
        .assert()
        .success()
        .stdout(contains("frork"));
}

#[test]
fn builds_a_standalone_binary_that_satisfies_its_script() {
    let dir = tempfile::TempDir::new().unwrap();
    let exe = dir.path().join("demo");
    let root = dir.path().join("root");
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../examples/standalone/main.fnl"
    );

    cmd()
        .args(["build", script, "-o"])
        .arg(&exe)
        .assert()
        .success();

    let run = |args: &[&str]| {
        let mut command = Command::new(&exe);
        command.args(args).env("FRORK_DEMO_ROOT", &root);
        command.assert().success()
    };
    run(&[]).stdout(contains("missing: directory"));
    run(&["--satisfy"]);
    run(&[]).stdout(contains("missing").not());

    let link = root.join("settings.txt");
    assert!(link.symlink_metadata().unwrap().is_symlink());
    assert!(
        fs_err::read_to_string(link)
            .unwrap()
            .starts_with("built on ")
    );
}
