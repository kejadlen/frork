use crate::assertions::Assertion;
use crate::assertions::Status;
use crate::runtime::Declaration;

/// Renders an assertion's status as the lines the CLI prints. Kept apart from
/// the printing itself so the wording is testable. A declaration that holds
/// prints bare; otherwise the status it found follows.
pub fn render(declaration: Declaration, status: &Status, assertion: &dyn Assertion) -> Vec<String> {
    match (declaration, status) {
        (Declaration::Ok, Status::Present) | (Declaration::No, Status::Absent) => {
            vec![format!("{declaration} {assertion}")]
        }
        (_, Status::Present) => vec![format!("{declaration} {assertion}: present")],
        (_, Status::Absent) => vec![format!("{declaration} {assertion}: absent")],
        // TODO: show a nicer diff?
        (_, Status::Drifted(drift)) => vec![
            format!("{declaration} {assertion}: drifted"),
            format!("  expected: {}", drift.expected),
            format!("    actual: {}", drift.actual),
        ],
    }
}

/// What applying an assertion does, given its declaration and status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Nothing,
    Create,
    /// Ask first, since an update overwrites what is there.
    PromptUpdate,
    /// No prompt: `no` already says the thing should go.
    Remove,
}

pub fn action(declaration: Declaration, status: &Status) -> Action {
    match (declaration, status) {
        (Declaration::Ok, Status::Present) | (Declaration::No, Status::Absent) => Action::Nothing,
        (Declaration::Ok, Status::Absent) => Action::Create,
        (Declaration::Ok, Status::Drifted(_)) => Action::PromptUpdate,
        (Declaration::No, Status::Present | Status::Drifted(_)) => Action::Remove,
    }
}

/// The line printed once an absent assertion has been created.
pub const CREATED: &str = "  created";

/// The question asked under a drifted assertion's status lines, which already
/// name it.
pub const UPDATE_PROMPT: &str = "  update? [y/N] ";

/// The line printed when the update prompt is accepted and the update runs.
pub const UPDATED: &str = "  updated";

/// The line printed when the update prompt is declined.
pub const KEPT: &str = "  kept";

/// The line printed once a `no` assertion's target has been removed.
pub const REMOVED: &str = "  removed";

/// Whether a reply to the update prompt means yes. Anything else declines,
/// so an unrecognised answer never updates.
pub fn wants_update(input: &str) -> bool {
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assertions::Drift;

    fn drifted() -> Status {
        Status::Drifted(Drift {
            expected: "new".to_string(),
            actual: "old".to_string(),
        })
    }

    #[test]
    fn test_action_for_ok() {
        assert_eq!(action(Declaration::Ok, &Status::Present), Action::Nothing);
        assert_eq!(action(Declaration::Ok, &Status::Absent), Action::Create);
        assert_eq!(action(Declaration::Ok, &drifted()), Action::PromptUpdate);
    }

    #[test]
    fn test_action_for_no_removes_without_asking() {
        assert_eq!(action(Declaration::No, &Status::Absent), Action::Nothing);
        assert_eq!(action(Declaration::No, &Status::Present), Action::Remove);
        assert_eq!(action(Declaration::No, &drifted()), Action::Remove);
    }

    // The property tests rarely land on near misses, so they're pinned here.
    #[test]
    fn test_wants_update_declines_everything_else() {
        for input in ["", "n", "no", "ye", "yess", "yep", "1", "true"] {
            assert!(!wants_update(input), "expected {input:?} to decline");
        }
    }
}
