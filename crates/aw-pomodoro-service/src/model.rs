use std::fmt;

use aw_pomodoro_core::PomodoroSettings as CoreSettings;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const MAX_DURATION_SECONDS: u64 = 24 * 60 * 60;
pub const MAX_WORK_INTERVALS: u32 = 100;
pub const MAX_CATEGORY_PATHS: usize = 128;
pub const MAX_CATEGORY_DEPTH: usize = 16;
pub const MAX_CATEGORY_SEGMENT_BYTES: usize = 256;

pub type CategoryPath = Vec<String>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PomodoroSettings {
    pub focus_duration_seconds: u64,
    pub short_break_duration_seconds: u64,
    pub long_break_duration_seconds: u64,
    pub work_intervals: u32,
    pub distraction_timeout_seconds: u64,
    pub system_notifications: bool,
    pub chrome_notifications: bool,
    pub sound_enabled: bool,
    pub last_selected_categories: Vec<CategoryPath>,
}

impl PomodoroSettings {
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_range(
            "focus_duration_seconds",
            self.focus_duration_seconds,
            1,
            MAX_DURATION_SECONDS,
        )?;
        validate_range(
            "short_break_duration_seconds",
            self.short_break_duration_seconds,
            1,
            MAX_DURATION_SECONDS,
        )?;
        validate_range(
            "long_break_duration_seconds",
            self.long_break_duration_seconds,
            1,
            MAX_DURATION_SECONDS,
        )?;
        validate_range(
            "distraction_timeout_seconds",
            self.distraction_timeout_seconds,
            1,
            MAX_DURATION_SECONDS,
        )?;
        if !(1..=MAX_WORK_INTERVALS).contains(&self.work_intervals) {
            return Err(ValidationError::OutOfRange {
                field: "work_intervals",
                min: 1,
                max: u64::from(MAX_WORK_INTERVALS),
            });
        }
        validate_categories(&self.last_selected_categories, true)
    }

    pub fn core_settings(&self) -> CoreSettings {
        CoreSettings {
            focus_duration_secs: self.focus_duration_seconds,
            short_break_duration_secs: self.short_break_duration_seconds,
            long_break_duration_secs: self.long_break_duration_seconds,
            work_intervals: self.work_intervals,
            distraction_timeout_secs: self.distraction_timeout_seconds,
        }
    }

    pub fn session_snapshot(&self) -> SessionSettings {
        SessionSettings {
            focus_duration_seconds: self.focus_duration_seconds,
            short_break_duration_seconds: self.short_break_duration_seconds,
            long_break_duration_seconds: self.long_break_duration_seconds,
            work_intervals: self.work_intervals,
            distraction_timeout_seconds: self.distraction_timeout_seconds,
            system_notifications: self.system_notifications,
            chrome_notifications: self.chrome_notifications,
            sound_enabled: self.sound_enabled,
        }
    }

    pub fn apply_session_settings(&mut self, settings: &SessionSettings) {
        self.focus_duration_seconds = settings.focus_duration_seconds;
        self.short_break_duration_seconds = settings.short_break_duration_seconds;
        self.long_break_duration_seconds = settings.long_break_duration_seconds;
        self.work_intervals = settings.work_intervals;
        self.distraction_timeout_seconds = settings.distraction_timeout_seconds;
        self.system_notifications = settings.system_notifications;
        self.chrome_notifications = settings.chrome_notifications;
        self.sound_enabled = settings.sound_enabled;
    }
}

impl Default for PomodoroSettings {
    fn default() -> Self {
        let core = CoreSettings::default();
        Self {
            focus_duration_seconds: core.focus_duration_secs,
            short_break_duration_seconds: core.short_break_duration_secs,
            long_break_duration_seconds: core.long_break_duration_secs,
            work_intervals: core.work_intervals,
            distraction_timeout_seconds: core.distraction_timeout_secs,
            system_notifications: true,
            chrome_notifications: true,
            sound_enabled: true,
            last_selected_categories: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SessionSettings {
    pub focus_duration_seconds: u64,
    pub short_break_duration_seconds: u64,
    pub long_break_duration_seconds: u64,
    pub work_intervals: u32,
    pub distraction_timeout_seconds: u64,
    pub system_notifications: bool,
    pub chrome_notifications: bool,
    pub sound_enabled: bool,
}

impl SessionSettings {
    pub fn validate(&self) -> Result<(), ValidationError> {
        let mut full = PomodoroSettings::default();
        full.apply_session_settings(self);
        full.validate()
    }

    pub fn core_settings(&self) -> CoreSettings {
        CoreSettings {
            focus_duration_secs: self.focus_duration_seconds,
            short_break_duration_secs: self.short_break_duration_seconds,
            long_break_duration_secs: self.long_break_duration_seconds,
            work_intervals: self.work_intervals,
            distraction_timeout_secs: self.distraction_timeout_seconds,
        }
    }
}

impl Default for SessionSettings {
    fn default() -> Self {
        PomodoroSettings::default().session_snapshot()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct StartRequest {
    pub selected_categories: Vec<CategoryPath>,
    #[serde(default)]
    pub settings: Option<SessionSettings>,
}

impl StartRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_categories(&self.selected_categories, false)?;
        if let Some(settings) = &self.settings {
            settings.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PomodoroState {
    Idle,
    RunningWork,
    RunningBreak,
    PausedManual,
    PausedAfk,
    WaitingConfirmation,
    Completed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PhaseKind {
    Focus,
    ShortBreak,
    LongBreak,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PhaseView {
    pub kind: PhaseKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_focus: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DistractionView {
    pub active: bool,
    pub elapsed_milliseconds: u64,
    pub warning_pending: bool,
    pub warning_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StateResponse {
    pub state: PomodoroState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<PhaseView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_phase: Option<PhaseView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_milliseconds: Option<u64>,
    pub completed_focus_intervals: u32,
    pub planned_focus_intervals: u32,
    pub selected_categories: Vec<CategoryPath>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_category: Option<CategoryPath>,
    pub distraction: DistractionView,
    pub afk: bool,
    pub monitoring_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<PauseReasonView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interruption_reason: Option<InterruptionReason>,
}

impl StateResponse {
    pub fn idle() -> Self {
        Self {
            state: PomodoroState::Idle,
            session_id: None,
            phase: None,
            next_phase: None,
            remaining_milliseconds: None,
            completed_focus_intervals: 0,
            planned_focus_intervals: 0,
            selected_categories: Vec::new(),
            current_category: None,
            distraction: DistractionView::default(),
            afk: false,
            monitoring_available: true,
            pause_reason: None,
            interruption_reason: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PauseReasonView {
    Manual,
    Afk,
    MonitoringUnavailable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Completed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InterruptionReason {
    User,
    ActivitywatchRestart,
    ServerError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionHistory {
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub status: SessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interruption_reason: Option<InterruptionReason>,
    pub settings: SessionSettings,
    pub selected_categories: Vec<CategoryPath>,
    pub planned_focus_intervals: u32,
    pub completed_focus_intervals: u32,
    pub actual_focus_milliseconds: u64,
    pub actual_break_milliseconds: u64,
    pub manual_pause_milliseconds: u64,
    pub afk_pause_milliseconds: u64,
    #[serde(default)]
    pub monitoring_pause_milliseconds: u64,
    pub distraction_count: u32,
    pub distraction_milliseconds: u64,
    pub allowed_focus_milliseconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_percentage: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryPage {
    pub items: Vec<SessionHistory>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    OutOfRange {
        field: &'static str,
        min: u64,
        max: u64,
    },
    MissingCategories,
    TooManyCategories,
    EmptyCategoryPath,
    CategoryTooDeep,
    InvalidCategorySegment,
    UncategorizedNotSelectable,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange { field, min, max } => {
                write!(f, "{field} must be between {min} and {max}")
            }
            Self::MissingCategories => write!(f, "at least one category must be selected"),
            Self::TooManyCategories => write!(f, "too many category paths"),
            Self::EmptyCategoryPath => write!(f, "category paths cannot be empty"),
            Self::CategoryTooDeep => write!(f, "category path is too deep"),
            Self::InvalidCategorySegment => {
                write!(
                    f,
                    "category segments must be non-empty and at most 256 bytes"
                )
            }
            Self::UncategorizedNotSelectable => {
                write!(f, "Uncategorized cannot be selected as a focus category")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

pub fn validate_categories(
    categories: &[CategoryPath],
    allow_empty: bool,
) -> Result<(), ValidationError> {
    if categories.is_empty() && !allow_empty {
        return Err(ValidationError::MissingCategories);
    }
    if categories.len() > MAX_CATEGORY_PATHS {
        return Err(ValidationError::TooManyCategories);
    }
    for path in categories {
        if path.is_empty() {
            return Err(ValidationError::EmptyCategoryPath);
        }
        if path.len() > MAX_CATEGORY_DEPTH {
            return Err(ValidationError::CategoryTooDeep);
        }
        if path
            .iter()
            .any(|part| part.trim().is_empty() || part.len() > MAX_CATEGORY_SEGMENT_BYTES)
        {
            return Err(ValidationError::InvalidCategorySegment);
        }
        if path.len() == 1 && path[0] == "Uncategorized" {
            return Err(ValidationError::UncategorizedNotSelectable);
        }
    }
    Ok(())
}

fn validate_range(
    field: &'static str,
    value: u64,
    min: u64,
    max: u64,
) -> Result<(), ValidationError> {
    if !(min..=max).contains(&value) {
        return Err(ValidationError::OutOfRange { field, min, max });
    }
    Ok(())
}
