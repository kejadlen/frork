default: all

fmt:
    cargo fmt --all

# Not --workspace: frork-cli and frork-lua use incompatible mlua features.
check:
    cargo check -p frork-cli

clippy:
    cargo clippy -p frork-cli --all-targets -- -D warnings

# Report-only until assertions.rs has tests; raise the threshold as it climbs.
coverage:
    COVERAGE_THRESHOLD=0 ./bin/coverage

mutants:
    #!/usr/bin/env bash
    set -uo pipefail
    cargo mutants -p frork-cli --timeout-multiplier 3 -j4
    rc=$?
    # 0 = all caught, 3 = timeouts (infinite loops from mutants, still caught).
    if [ "$rc" -eq 0 ] || [ "$rc" -eq 3 ]; then
        exit 0
    fi
    exit "$rc"

all: fmt clippy coverage

install:
    cargo install --locked --path frork-cli
