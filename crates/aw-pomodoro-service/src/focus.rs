use serde::{Deserialize, Serialize};

use crate::model::CategoryPath;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivityObservation {
    pub afk: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<CategoryPath>,
}

impl ActivityObservation {
    pub fn active(category: Option<CategoryPath>) -> Self {
        Self {
            afk: false,
            category,
        }
    }

    pub fn afk() -> Self {
        Self {
            afk: true,
            category: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PomodoroEvent {
    DistractionWarning {
        sequence: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        current_category: Option<CategoryPath>,
        remaining_milliseconds: u64,
    },
    AfkPaused,
    MonitoringUnavailablePaused,
}

pub fn category_is_allowed(category: &[String], selected: &[CategoryPath]) -> bool {
    selected
        .iter()
        .any(|allowed| category.starts_with(allowed.as_slice()))
}

#[cfg(test)]
mod tests {
    use super::category_is_allowed;

    #[test]
    fn parent_selection_includes_descendants_but_not_similar_names() {
        let selected = vec![vec!["Work".to_string()]];
        assert!(category_is_allowed(&["Work".into()], &selected));
        assert!(category_is_allowed(
            &["Work".into(), "Programming".into(), "Rust".into()],
            &selected
        ));
        assert!(!category_is_allowed(&["Work personal".into()], &selected));
    }

    #[test]
    fn any_selected_branch_is_allowed() {
        let selected = vec![vec!["Work".into()], vec!["Study".into(), "Rust".into()]];
        assert!(category_is_allowed(
            &["Study".into(), "Rust".into(), "Books".into()],
            &selected
        ));
        assert!(!category_is_allowed(
            &["Study".into(), "Math".into()],
            &selected
        ));
    }
}
