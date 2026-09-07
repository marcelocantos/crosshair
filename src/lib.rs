// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0

pub mod cli;
pub mod loader;
pub mod runner;
pub mod schema;
pub mod status;
pub mod store;
pub mod tick;

#[cfg(test)]
mod vcheck_gate_plant {
    #[test]
    fn planted_failure_must_redden_the_gate() {
        let observed = std::env::var("VCHECK_PLANT").unwrap_or_else(|_| "red".to_string());
        assert_eq!(observed, "green", "planted by gate audit");
    }
}
