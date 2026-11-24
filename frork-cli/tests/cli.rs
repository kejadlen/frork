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
        .args(["apply", "-e", &code])
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

fn directory_fixture() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("made");
    let code = format!(r#"(ok :directory "{}")"#, path.display());
    (dir, path, code)
}

fn script_with(dir: &tempfile::TempDir, code: &str) -> String {
    let script = dir.path().join("frork.fnl");
    fs_err::write(
        &script,
        format!("(local {{: ok}} (require :frork))\n{code}\n"),
    )
    .unwrap();
    script.display().to_string()
}

#[test]
fn check_reports_inline_code_without_changing_anything() {
    let (_dir, path, code) = directory_fixture();

    cmd()
        .args(["check", "-e", &code])
        .assert()
        .success()
        .stdout(format!("ok directory {}: absent\n", path.display()));
    assert!(!path.exists());
}

#[test]
fn check_reports_a_script() {
    let (dir, path, code) = directory_fixture();
    let script = script_with(&dir, &code);

    cmd()
        .args(["check", &script])
        .assert()
        .success()
        .stdout(format!("ok directory {}: absent\n", path.display()));
    assert!(!path.exists());
}

#[test]
fn apply_creates_from_inline_code() {
    let (_dir, path, code) = directory_fixture();
    let display = path.display();

    cmd()
        .args(["apply", "-e", &code])
        .assert()
        .success()
        .stdout(format!("ok directory {display}: absent\n  created\n"));
    assert!(path.is_dir());
}

#[test]
fn apply_creates_from_a_script() {
    let (dir, path, code) = directory_fixture();
    let script = script_with(&dir, &code);

    cmd().args(["apply", &script]).assert().success();
    assert!(path.is_dir());
}

#[test]
fn check_and_apply_require_a_script_or_eval() {
    for subcommand in ["check", "apply"] {
        cmd()
            .arg(subcommand)
            .assert()
            .failure()
            .stderr(contains("required"));
    }
}

#[test]
fn check_and_apply_reject_both_a_script_and_eval() {
    for subcommand in ["check", "apply"] {
        cmd()
            .args([subcommand, "script.fnl", "-e", "(+ 1 1)"])
            .assert()
            .failure()
            .stderr(contains("cannot be used with"));
    }
}

/// A drifted provider whose update leaves a marker file, so a test can tell
/// whether it ran.
fn drifted_fixture() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let dir = tempfile::TempDir::new().unwrap();
    let marker = dir.path().join("updated");
    let code = format!(
        r#"(local frork (require :frork))
           (frork.provide :thing
             {{:status (fn [] (values :drifted {{:expected "new" :actual "old"}}))
               :create (fn [])
               :update (fn [] (: (io.open "{}" :w) :close))}})
           (ok :thing)"#,
        marker.display()
    );
    (dir, marker, code)
}

const DRIFTED: &str = "ok thing: drifted\n  expected: new\n    actual: old\n  update? [y/N] ";

#[test]
fn apply_keeps_a_drift_when_declined() {
    let (_dir, marker, code) = drifted_fixture();

    cmd()
        .args(["apply", "-e", &code])
        .write_stdin("n\n")
        .assert()
        .success()
        .stdout(format!("{DRIFTED}  kept\n"));
    assert!(!marker.exists());
}

#[test]
fn apply_updates_a_drift_when_accepted() {
    let (_dir, marker, code) = drifted_fixture();

    cmd()
        .args(["apply", "-e", &code])
        .write_stdin("y\n")
        .assert()
        .success()
        .stdout(format!("{DRIFTED}  updated\n"));
    assert!(marker.exists());
}
