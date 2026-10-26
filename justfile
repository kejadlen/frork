default: all

fmt:
    cargo fmt --all

# Not --workspace: frork-cli and frork-lua use incompatible mlua features.
check:
    cargo check -p frork-cli -p frork-lib

clippy:
    # frork-lib's tests link a vendored Lua, so they lint alongside frork-cli.
    cargo clippy -p frork-cli -p frork-lib --all-targets -- -D warnings
    # frork (frork-lua) builds frork-lib against its host's Lua instead.
    cargo clippy -p frork -- -D warnings

coverage:
    ./bin/coverage

mutants:
    #!/usr/bin/env bash
    set -uo pipefail
    cargo mutants -p frork-cli -p frork-lib --timeout-multiplier 3 -j4
    rc=$?
    # 0 = all caught, 3 = timeouts (infinite loops from mutants, still caught).
    if [ "$rc" -eq 0 ] || [ "$rc" -eq 3 ]; then
        exit 0
    fi
    exit "$rc"

all: fmt clippy coverage

install:
    cargo install --locked --path frork-cli
