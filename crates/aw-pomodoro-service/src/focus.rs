use serde::{Deserialize, Serialize};

use crate::model::{CategoryPath, PauseReasonView, PhaseView};

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
        session_id: String,
        sequence: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        current_category: Option<CategoryPath>,
        remaining_milliseconds: u64,
        notifications: PomodoroNotificationOptions,
    },
    PhaseCompleted {
        session_id: String,
        completed: PhaseView,
        next: PhaseView,
        notifications: PomodoroNotificationOptions,
    },
    SessionCompleted {
        session_id: String,
        focus_intervals: u32,
        notifications: PomodoroNotificationOptions,
    },
    AfkPaused {
        session_id: String,
        notifications: PomodoroNotificationOptions,
    },
    MonitoringUnavailablePaused {
        session_id: String,
        notifications: PomodoroNotificationOptions,
    },
}

impl PomodoroEvent {
    pub fn chrome_notifications_enabled(&self) -> bool {
        match self {
            Self::DistractionWarning { notifications, .. }
            | Self::PhaseCompleted { notifications, .. }
            | Self::SessionCompleted { notifications, .. }
            | Self::AfkPaused { notifications, .. }
            | Self::MonitoringUnavailablePaused { notifications, .. } => {
                notifications.chrome_notifications
            }
        }
    }

    pub fn action_token(&self) -> Option<PomodoroActionToken> {
        match self {
            Self::DistractionWarning {
                session_id,
                sequence,
                ..
            } => Some(PomodoroActionToken::Distraction {
                session_id: session_id.clone(),
                sequence: *sequence,
            }),
            Self::PhaseCompleted {
                session_id,
                completed,
                next,
                ..
            } => Some(PomodoroActionToken::PhaseCompleted {
                session_id: session_id.clone(),
                completed: completed.clone(),
                next: next.clone(),
            }),
            Self::AfkPaused { session_id, .. } => Some(PomodoroActionToken::Paused {
                session_id: session_id.clone(),
                reason: PauseReasonView::Afk,
            }),
            Self::MonitoringUnavailablePaused { session_id, .. } => {
                Some(PomodoroActionToken::Paused {
                    session_id: session_id.clone(),
                    reason: PauseReasonView::MonitoringUnavailable,
                })
            }
            Self::SessionCompleted { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChromeNotification {
    pub id: u64,
    pub event: PomodoroEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChromeNotificationPage {
    pub items: Vec<ChromeNotification>,
    pub latest_id: u64,
    pub instance_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct PomodoroNotificationOptions {
    pub system_notifications: bool,
    pub chrome_notifications: bool,
    pub sound_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PomodoroActionToken {
    Distraction {
        session_id: String,
        sequence: u64,
    },
    PhaseCompleted {
        session_id: String,
        completed: PhaseView,
        next: PhaseView,
    },
    Paused {
        session_id: String,
        reason: PauseReasonView,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PomodoroNotificationAction {
    Continue,
    Pause,
    Stop,
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
