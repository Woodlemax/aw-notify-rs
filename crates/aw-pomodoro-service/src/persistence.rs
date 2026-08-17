use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{CategoryPath, PomodoroSettings, SessionHistory, SessionSettings};

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PersistedState {
    pub schema_version: u32,
    pub settings: PomodoroSettings,
    pub active_session: Option<SessionCheckpoint>,
    pub pending_history: Vec<SessionHistory>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            settings: PomodoroSettings::default(),
            active_session: None,
            pending_history: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionCheckpoint {
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub settings: SessionSettings,
    pub selected_categories: Vec<CategoryPath>,
    pub planned_focus_intervals: u32,
    pub completed_focus_intervals: u32,
    pub metrics: PersistedMetrics,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PersistedMetrics {
    pub actual_focus_milliseconds: u64,
    pub actual_break_milliseconds: u64,
    pub manual_pause_milliseconds: u64,
    pub afk_pause_milliseconds: u64,
    pub monitoring_pause_milliseconds: u64,
    pub distraction_count: u32,
    pub distraction_milliseconds: u64,
    pub allowed_focus_milliseconds: u64,
}

pub(crate) struct FileStateStore {
    path: PathBuf,
}

impl FileStateStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<(Self, PersistedState), String> {
        let store = Self { path: path.into() };
        if let Some(parent) = store.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create Pomodoro state directory {}: {error}",
                    parent.display()
                )
            })?;
        }

        if !store.path.exists() {
            let state = PersistedState::default();
            store.save(&state)?;
            return Ok((store, state));
        }

        let bytes = fs::read(&store.path).map_err(|error| {
            format!(
                "failed to read Pomodoro state {}: {error}",
                store.path.display()
            )
        })?;
        let state: PersistedState = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "failed to parse Pomodoro state {}: {error}",
                store.path.display()
            )
        })?;
        if state.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported Pomodoro state schema version {}",
                state.schema_version
            ));
        }
        state.settings.validate().map_err(|error| {
            format!(
                "invalid Pomodoro settings in {}: {error}",
                store.path.display()
            )
        })?;
        Ok((store, state))
    }

    pub fn save(&self, state: &PersistedState) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|error| format!("failed to serialize Pomodoro state: {error}"))?;
        fs::write(&self.path, bytes).map_err(|error| {
            format!(
                "failed to save Pomodoro state {}: {error}",
                self.path.display()
            )
        })
    }
}
