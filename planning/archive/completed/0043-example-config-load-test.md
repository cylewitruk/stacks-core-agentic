# Completed: Example Config Load Test

- **id:** `0043-example-config-load-test`
- **status:** `shipped`
- **completed:** `2026-06-11`
- **iteration:** [v5: Archive Metadata](v5-archive-metadata.md)

## Problem

`assets/example.config.toml` can drift from the typed `Settings` shape during
configuration refactors, leaving fresh operator copies broken.

## Shipped

Added an integration test that reads `assets/example.config.toml` as-is and
deserializes it into `Settings` with `toml::from_str`. The test intentionally
skips `Settings::load` runtime helpers so it checks template shape, not local
operator environment.

## Validation

- `tests/example_config.rs::example_config_template_parses_into_settings`
  passes.
- A manual `[bogus_section]` injection failed with the expected unknown-field
  error, then was restored.
