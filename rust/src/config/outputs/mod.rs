//! Outputs module — tag-selector DSL and resolution logic.
//!
//! Replaces the legacy `envs:` DSL block with a tag-based `outputs:` block.
//! See `.omo/plans/remove-envs-replace-with-tags.md` for the full migration plan.

pub mod dsl;
pub mod outputs_mut;
pub mod selector;

pub use dsl::{AliasMap, OutputDef, OutputsMap, SelectorEntry};
pub mod resolver;
