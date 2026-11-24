use crate::assertions::Provider;
use crate::assertions::Status;

/// Renders an assertion's status as the lines the CLI prints. Kept apart from
/// the printing itself so the wording is testable.
pub fn render(status: &Status, assertion: &dyn Provider) -> Vec<String> {
    match status {
        Status::Present => vec![format!("ok {assertion}")],
        Status::Absent => vec![format!("ok {assertion}: absent")],
        // TODO: show a nicer diff?
        Status::Drifted(drift) => vec![
            format!("ok {assertion}: drifted"),
            format!("  expected: {}", drift.expected),
            format!("    actual: {}", drift.actual),
        ],
    }
}

/// The question asked before updating a drifted assertion.
pub fn update_prompt(assertion: &dyn Provider) -> String {
    format!("Update {assertion}? [y/N] ")
}

/// The line printed when the update prompt is declined.
pub fn kept(assertion: &dyn Provider) -> String {
    format!("kept: {assertion}")
}

/// Whether a reply to the update prompt means yes. Anything else declines,
/// so an unrecognised answer never updates.
pub fn wants_update(input: &str) -> bool {
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    // The property tests rarely land on near misses, so they're pinned here.
    #[test]
    fn test_wants_update_declines_everything_else() {
        for input in ["", "n", "no", "ye", "yess", "yep", "1", "true"] {
            assert!(!wants_update(input), "expected {input:?} to decline");
        }
    }
}
