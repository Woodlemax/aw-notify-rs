//! Background Pomodoro session service and loopback API.

#![forbid(unsafe_code)]

mod api;
mod history;
mod model;
mod persistence;
mod service;

pub use api::{ApiRequest, ApiResponse, PomodoroApi};
pub use history::{HistoryError, HistoryStore};
pub use model::{
    CategoryPath, DistractionView, HistoryPage, InterruptionReason, PhaseKind, PhaseView,
    PomodoroSettings, PomodoroState, SessionHistory, SessionSettings, SessionStatus, StartRequest,
    StateResponse,
};
pub use service::{PomodoroService, ServiceError};
