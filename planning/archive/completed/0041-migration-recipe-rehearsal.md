# 0041: Migration Recipe Rehearsal

- **id:** `0041-migration-recipe-rehearsal`
- **status:** `shipped`
- **priority:** `medium`
- **iteration:** [v4-v3-polish-and-bot-fork-seed](v4-v3-polish-and-bot-fork-seed.md)

## Shipped

Added a fixture-driven rehearsal for the documented pre-v3 operator migration
recipe. The rehearsal builds a synthetic pre-cutover operator repo, applies the
recipe, and asserts the post-cutover filesystem and git shape.

## Validation

- `tests/migration_recipe.rs::migration_recipe_converges_pre_cutover_operator_to_post_cutover`
  passes.
- Live validation against a real pre-v3 operator repo was waived on
  2026-06-12 because no such repo remains.
