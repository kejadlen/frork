use crate::assertions::AssertionType;
use crate::assertions::Status;

/// Renders an assertion's status as the lines the CLI prints. Kept apart from
/// the printing itself so the wording is testable.
pub fn render(status: &Status, assertion: &dyn AssertionType) -> Vec<String> {
    match status {
        Status::Ok => vec![format!("ok: {assertion}")],
        Status::Missing => vec![format!("missing: {assertion}")],
        // TODO: show a nicer diff?
        Status::ConflictUpgrade(conflict) => vec![
            format!("conflict (upgradable): {assertion}"),
            format!("  expected: {}", conflict.expected),
            format!("    actual: {}", conflict.actual),
        ],
    }
}

/// Whether a reply to the upgrade prompt means yes. Anything else declines,
/// so an unrecognised answer never upgrades.
pub fn wants_upgrade(input: &str) -> bool {
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assertions::Conflict;
    use crate::utils::ExpandedPath;

    fn directory() -> crate::assertions::Directory {
        crate::assertions::Directory {
            path: ExpandedPath::try_from("/tmp").unwrap(),
        }
    }

    #[test]
    fn test_render_ok() {
        assert_eq!(render(&Status::Ok, &directory()), ["ok: directory /tmp"]);
    }

    #[test]
    fn test_render_missing() {
        assert_eq!(
            render(&Status::Missing, &directory()),
            ["missing: directory /tmp"]
        );
    }

    #[test]
    fn test_render_conflict_includes_both_sides() {
        let status = Status::ConflictUpgrade(Conflict {
            expected: "a".to_string(),
            actual: "b".to_string(),
        });

        assert_eq!(
            render(&status, &directory()),
            [
                "conflict (upgradable): directory /tmp",
                "  expected: a",
                "    actual: b",
            ]
        );
    }

    #[test]
    fn test_wants_upgrade_accepts_y_and_yes() {
        for input in ["y", "yes", "Y", "YES", "  yes  ", "Yes\n"] {
            assert!(wants_upgrade(input), "expected {input:?} to upgrade");
        }
    }

    #[test]
    fn test_wants_upgrade_declines_everything_else() {
        for input in ["", "n", "no", "ye", "yess", "yep", "1", "true"] {
            assert!(!wants_upgrade(input), "expected {input:?} to decline");
        }
    }
}
