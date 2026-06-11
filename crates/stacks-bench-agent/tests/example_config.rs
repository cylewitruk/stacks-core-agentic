//! Guard against `assets/example.config.toml` drifting away from the
//! `Settings` struct shape.
//!
//! Reads the bundled template AS-IS and deserializes it directly into
//! `Settings` via `toml::from_str`. **Intentionally bypasses
//! `Settings::load`**, which layers `config` crate resolution +
//! validation on top — those are operator-environment concerns
//! (path canonicalization, `publish.token_file` existence probes,
//! etc.), not template-shape concerns. The shape concerns are
//! exactly what `deny_unknown_fields` + the typed field set enforce,
//! and those run during plain `toml::from_str`.

use stacks_bench_agent::settings::Settings;

/// Path to the bundled template, resolved relative to the crate root
/// at compile time (`CARGO_MANIFEST_DIR`). Lives next to the operator
/// `setup.md`; the template is the canonical example for a fresh
/// operator config.
const EXAMPLE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/example.config.toml");

#[test]
fn example_config_template_parses_into_settings() {
    let body = std::fs::read_to_string(EXAMPLE_PATH).unwrap_or_else(|e| {
        panic!("reading bundled template at {EXAMPLE_PATH}: {e}");
    });
    match toml::from_str::<Settings>(&body) {
        Ok(_) => {}
        Err(e) => panic!(
            "assets/example.config.toml failed to deserialize into the current `Settings` shape — \
             the bundled template has drifted from the typed model. Update the template (or the \
             model) so a fresh operator copy is valid.\n\nUnderlying error:\n{e}",
        ),
    }
}
