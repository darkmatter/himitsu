//! Outputs module — tag-selector DSL and resolution logic.
//!
//! Replaces the legacy `envs:` DSL block with a tag-based `outputs:` block.
//! See `.omo/plans/remove-envs-replace-with-tags.md` for the full migration plan.

pub mod selector;
