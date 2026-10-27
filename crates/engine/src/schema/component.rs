//! Contracts and result types for the commands in `cmd/component.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Finish editing a linked component's design in place. In the app window: save the part, go back
/// to the assembly around it, which reads the link again, and close the part's tab when the edit
/// opened it. Outside the window nothing is edited in place, so nothing changes.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FinishEditParams {}

/// What `component.finish_edit` returns.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct FinishEditResult {
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
#[path = "component_tests.rs"]
mod tests;
