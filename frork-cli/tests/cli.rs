#![cfg_attr(test, allow(clippy::unwrap_used))]

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
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
fn shows_warnings_on_stderr_without_rust_log() {
    let dir = tempfile::TempDir::new().unwrap();
    let link = dir.path().join("link");
    let source = dir.path().join("missing");
    let code = format!(
        r#"(ok :symlink "{}" "{}")"#,
        link.display(),
        source.display()
    );

    cmd()
        .env_remove("RUST_LOG")
        .args(["do", &code])
        .assert()
        .success()
        .stderr(contains("source does not exist"))
        .stdout(contains("source does not exist").not());
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
