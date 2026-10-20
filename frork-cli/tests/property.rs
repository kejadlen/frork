// Property tests exercise invariants that hold across all inputs; unit tests
// in the modules pin down known-good fixtures.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]

use frork_cli::report;
use frork_cli::utils::Utils;
use hegel::TestCase;
use hegel::generators::text;

#[hegel::test]
fn chomp_strips_all_trailing_newlines(tc: TestCase) {
    let s = tc.draw(text());
    let chomped = Utils::chomp(&s);
    assert!(
        !chomped.ends_with('\n') && !chomped.ends_with('\r'),
        "chomp({s:?}) = {chomped:?}, which still ends in a newline"
    );
}

#[hegel::test]
fn chomp_is_idempotent(tc: TestCase) {
    let s = tc.draw(text());
    assert_eq!(
        Utils::chomp(&Utils::chomp(&s)),
        Utils::chomp(&s),
        "chomping {s:?} twice differs from chomping once"
    );
}

#[hegel::test]
fn chomp_only_removes_from_the_end(tc: TestCase) {
    let s = tc.draw(text());
    assert!(
        s.starts_with(&Utils::chomp(&s)),
        "chomp({s:?}) is not a prefix of its input"
    );
}

#[hegel::test]
fn chomp_never_grows_its_input(tc: TestCase) {
    let s = tc.draw(text());
    assert!(
        Utils::chomp(&s).len() <= s.len(),
        "chomp({s:?}) grew its input"
    );
}

#[hegel::test]
fn wants_upgrade_accepts_only_y_or_yes(tc: TestCase) {
    let s = tc.draw(text());
    if report::wants_upgrade(&s) {
        let normalized = s.trim().to_lowercase();
        assert!(
            normalized == "y" || normalized == "yes",
            "wants_upgrade({s:?}) accepted something other than y/yes"
        );
    }
}
