//! Contracts and result types for the commands in `cmd/component.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::{IdOrName, present};

/// Edit a linked component's design in place. In the app window: open the design it links in a
/// tab of its own, with the rest of the assembly drawn faded around it as the placement `path`
/// puts it; `component.finish_edit` goes back. It fails when the component is not linked, the
/// assembly has no file yet, or something is open in it. Outside the window it only checks the
/// component and the placement.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EditInPlaceParams {
    /// The linked component, by id or name.
    pub component: IdOrName,
    /// The placement to edit it at, as occurrence ids from the top of the assembly down
    /// (`component.list`). Default: the component's first placement.
    #[serde(default, deserialize_with = "present")]
    pub path: Option<Vec<u64>>,
}

/// What `component.edit_in_place` returns.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct EditInPlaceResult {
    /// The component, by id.
    pub component: u64,
    /// The placement it is edited at.
    pub path: Vec<u64>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Finish editing a linked component's design in place. In the app window: go back to the
/// assembly around it, which reads the link again, and close the part's tab when the edit opened
/// it. It fails while the part has unsaved changes (save it first) or something is open in it;
/// asked by the part's own agent, it finishes when that agent's turn ends. Outside the window
/// nothing is edited in place, so nothing changes.
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
