//! Background Pomodoro session service and loopback API.

#![forbid(unsafe_code)]

mod api;
mod focus;
mod history;
mod model;
mod persistence;
mod service;

pub use api::{ApiRequest, ApiResponse, PomodoroApi};
pub use focus::{
    category_is_allowed, ActivityObservation, PomodoroActionToken, PomodoroEvent,
    PomodoroNotificationAction, PomodoroNotificationOptions,
};
pub use history::{HistoryError, HistoryStore};
pub use model::{
    CategoryPath, DistractionView, HistoryPage, InterruptionReason, PauseReasonView, PhaseKind,
    PhaseView, PomodoroSettings, PomodoroState, SessionHistory, SessionSettings, SessionStatus,
    StartRequest, StateResponse,
};
pub use service::{PomodoroService, ServiceError};
