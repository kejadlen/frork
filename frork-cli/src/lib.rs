// Panic discipline applies to code that handles Fennel scripts and shell
// output; test code asserts on known-good fixtures.
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

pub mod assertions;
pub mod error;
pub mod registry;
pub mod runtime;
pub mod utils;
