// Test code asserts on known-good fixtures.
#![cfg_attr(test, allow(clippy::unwrap_used))]

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
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
fn compiles_a_script_into_a_standalone_binary() {
    let dir = tempfile::TempDir::new().unwrap();
    let script = dir.path().join("main.fnl");
    let target = dir.path().join("made");
    fs_err::write(
        &script,
        format!(
            "((. (require :frork) :ok) :directory {:?})",
            target.display()
        ),
    )
    .unwrap();
    let exe = dir.path().join("standalone");

    cmd()
        .arg("compile")
        .arg(&script)
        .arg("-o")
        .arg(&exe)
        .assert()
        .success();

    Command::new(&exe)
        .assert()
        .success()
        .stdout(contains("missing: directory"));
    Command::new(&exe).arg("--bogus").assert().failure();
    Command::new(&exe)
        .args(["--satisfy", "--", "--bogus"])
        .assert()
        .success();
    assert!(target.is_dir());
}
