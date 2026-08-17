use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use aw_pomodoro_core::{
    InterruptionReason as CoreInterruptionReason, PauseReason, Phase, PomodoroTimer, TimerError,
    TimerState,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::focus::{
    category_is_allowed, ActivityObservation, ChromeNotification, ChromeNotificationPage,
    PomodoroActionToken, PomodoroEvent, PomodoroNotificationAction, PomodoroNotificationOptions,
};
use crate::history::HistoryStore;
use crate::model::{
    validate_categories, DistractionView, HistoryPage, InterruptionReason, PauseReasonView,
    PhaseKind, PhaseView, PomodoroSettings, PomodoroState, SessionHistory, SessionStatus,
    StartRequest, StateResponse,
};
use crate::persistence::{FileStateStore, PersistedMetrics, PersistedState, SessionCheckpoint};

const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(1);
const MAX_CHROME_NOTIFICATIONS: usize = 100;

pub struct PomodoroService {
    file_store: FileStateStore,
    persisted: PersistedState,
    history_store: Box<dyn HistoryStore>,
    current: Option<RuntimeSession>,
    pending_events: Vec<PomodoroEvent>,
    chrome_notifications: VecDeque<ChromeNotification>,
    next_chrome_notification_id: u64,
    notification_instance_id: String,
}

struct RuntimeSession {
    session_id: String,
    started_at: DateTime<Utc>,
    settings: crate::model::SessionSettings,
    selected_categories: Vec<crate::model::CategoryPath>,
    timer: PomodoroTimer,
    metrics: PersistedMetrics,
    metric_anchor: Instant,
    checkpoint_anchor: Instant,
    finalized: bool,
    focus: FocusRuntime,
    afk: bool,
    monitoring_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusClassification {
    Unknown,
    Allowed,
    Distracted,
}

struct FocusRuntime {
    classification: FocusClassification,
    current_category: Option<crate::model::CategoryPath>,
    distracted_since: Option<Instant>,
    warning_pending: bool,
    warning_sequence: u64,
}

impl Default for FocusRuntime {
    fn default() -> Self {
        Self {
            classification: FocusClassification::Unknown,
            current_category: None,
            distracted_since: None,
            warning_pending: false,
            warning_sequence: 0,
        }
    }
}

impl FocusRuntime {
    fn reset(&mut self) {
        self.classification = FocusClassification::Unknown;
        self.current_category = None;
        self.distracted_since = None;
        self.warning_pending = false;
    }
}

impl PomodoroService {
    pub fn open(
        state_path: impl Into<PathBuf>,
        history_store: Box<dyn HistoryStore>,
    ) -> Result<Self, ServiceError> {
        let (file_store, mut persisted) =
            FileStateStore::open(state_path).map_err(ServiceError::Storage)?;

        if let Some(checkpoint) = persisted.active_session.take() {
            persisted
                .pending_history
                .push(history_from_restart(checkpoint));
            file_store.save(&persisted).map_err(ServiceError::Storage)?;
        }

        let mut service = Self {
            file_store,
            persisted,
            history_store,
            current: None,
            pending_events: Vec::new(),
            chrome_notifications: VecDeque::new(),
            next_chrome_notification_id: 1,
            notification_instance_id: Uuid::new_v4().to_string(),
        };
        service.flush_pending_history();
        Ok(service)
    }

    pub fn settings(&self) -> PomodoroSettings {
        self.persisted.settings.clone()
    }

    pub fn update_settings(
        &mut self,
        settings: PomodoroSettings,
    ) -> Result<PomodoroSettings, ServiceError> {
        if self.has_active_session() {
            return Err(ServiceError::Conflict(
                "settings cannot be changed while a session is active".to_string(),
            ));
        }
        settings
            .validate()
            .map_err(|error| ServiceError::Validation(error.to_string()))?;

        let mut next = self.persisted.clone();
        next.settings = settings.clone();
        self.file_store.save(&next).map_err(ServiceError::Storage)?;
        self.persisted = next;
        Ok(settings)
    }

    pub fn start(
        &mut self,
        request: StartRequest,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<StateResponse, ServiceError> {
        if self.has_active_session() {
            return Err(ServiceError::Conflict(
                "a Pomodoro session is already active".to_string(),
            ));
        }
        request
            .validate()
            .map_err(|error| ServiceError::Validation(error.to_string()))?;

        let categories = deduplicate_categories(request.selected_categories);
        validate_categories(&categories, false)
            .map_err(|error| ServiceError::Validation(error.to_string()))?;
        let settings = request
            .settings
            .unwrap_or_else(|| self.persisted.settings.session_snapshot());
        let mut timer = PomodoroTimer::new(settings.core_settings())
            .map_err(|error| ServiceError::Validation(error.to_string()))?;
        timer.start(now).map_err(ServiceError::Timer)?;

        let session = RuntimeSession {
            session_id: Uuid::new_v4().to_string(),
            started_at: wall_now,
            settings: settings.clone(),
            selected_categories: categories.clone(),
            timer,
            metrics: PersistedMetrics::default(),
            metric_anchor: now,
            checkpoint_anchor: now,
            finalized: false,
            focus: FocusRuntime::default(),
            afk: false,
            monitoring_available: true,
        };

        let mut next = self.persisted.clone();
        next.settings.apply_session_settings(&settings);
        next.settings.last_selected_categories = categories;
        next.active_session = Some(session.checkpoint(wall_now));
        self.file_store.save(&next).map_err(ServiceError::Storage)?;

        self.persisted = next;
        self.current = Some(session);
        self.state(now)
    }

    pub fn pause(
        &mut self,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<StateResponse, ServiceError> {
        self.accumulate(now);
        let session = self.active_session_mut()?;
        session
            .timer
            .pause(now, PauseReason::Manual)
            .map_err(ServiceError::Timer)?;
        session.focus.reset();
        self.save_checkpoint(wall_now)?;
        self.state(now)
    }

    pub fn resume(
        &mut self,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<StateResponse, ServiceError> {
        self.accumulate(now);
        let session = self.active_session_mut()?;
        if session.afk {
            return Err(ServiceError::Conflict(
                "the session cannot resume while the user is AFK".to_string(),
            ));
        }
        if !session.monitoring_available {
            return Err(ServiceError::Conflict(
                "the session cannot resume until ActivityWatch monitoring recovers".to_string(),
            ));
        }
        session.timer.resume(now).map_err(ServiceError::Timer)?;
        session.focus.reset();
        self.save_checkpoint(wall_now)?;
        self.state(now)
    }

    pub fn confirm_next(
        &mut self,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<StateResponse, ServiceError> {
        self.accumulate(now);
        let session = self.active_session_mut()?;
        if session.afk {
            return Err(ServiceError::Conflict(
                "the next phase cannot start while the user is AFK".to_string(),
            ));
        }
        if !session.monitoring_available {
            return Err(ServiceError::Conflict(
                "the next phase cannot start until ActivityWatch monitoring recovers".to_string(),
            ));
        }
        session
            .timer
            .confirm_next(now)
            .map_err(ServiceError::Timer)?;
        session.focus.reset();
        self.save_checkpoint(wall_now)?;
        self.state(now)
    }

    pub fn stop(
        &mut self,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<StateResponse, ServiceError> {
        self.accumulate(now);
        let session = self.active_session_mut()?;
        session
            .timer
            .stop(CoreInterruptionReason::UserStopped)
            .map_err(ServiceError::Timer)?;
        session.focus.reset();
        self.finalize(wall_now, Some(InterruptionReason::User))?;
        self.state(now)
    }

    pub fn observe_activity(
        &mut self,
        observation: ActivityObservation,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<(), ServiceError> {
        self.tick(now, wall_now)?;
        if !self.has_active_session() {
            return Ok(());
        }

        let session = self.active_session_mut()?;
        session.monitoring_available = true;
        session.afk = observation.afk;

        if observation.afk {
            session.focus.reset();
            if matches!(session.timer.state(), TimerState::Running { .. }) {
                session
                    .timer
                    .pause(now, PauseReason::Afk)
                    .map_err(ServiceError::Timer)?;
                let event = PomodoroEvent::AfkPaused {
                    session_id: session.session_id.clone(),
                    notifications: notification_options(&session.settings),
                };
                self.emit_event(event);
                self.save_checkpoint(wall_now)?;
            }
            return Ok(());
        }

        let is_focus = matches!(
            session.timer.state(),
            TimerState::Running {
                phase: Phase::Focus { .. }
            }
        );
        if !is_focus {
            session.focus.reset();
            return Ok(());
        }

        let category = observation.category.filter(|path| {
            !path.is_empty() && path.first().is_some_and(|part| part != "Uncategorized")
        });
        let allowed = category
            .as_ref()
            .is_some_and(|path| category_is_allowed(path, &session.selected_categories));
        let next_classification = if allowed {
            FocusClassification::Allowed
        } else {
            FocusClassification::Distracted
        };
        let entering_distraction = session.focus.classification != FocusClassification::Distracted
            && next_classification == FocusClassification::Distracted;
        session.focus.classification = next_classification;
        session.focus.current_category = category.clone();

        if allowed {
            session.focus.distracted_since = None;
            session.focus.warning_pending = false;
            return Ok(());
        }

        if entering_distraction {
            session.focus.distracted_since = Some(now);
            session.focus.warning_pending = false;
            session.metrics.distraction_count = session.metrics.distraction_count.saturating_add(1);
        }

        let distracted_since = session.focus.distracted_since.get_or_insert(now);
        let timeout = Duration::from_secs(session.settings.distraction_timeout_seconds);
        let warning = if now.saturating_duration_since(*distracted_since) >= timeout {
            session.focus.warning_sequence = session.focus.warning_sequence.saturating_add(1);
            session.focus.warning_pending = true;
            session.focus.distracted_since = Some(now);
            let remaining_milliseconds = session
                .timer
                .remaining(now)
                .map_err(ServiceError::Timer)?
                .map_or(0, duration_milliseconds);
            Some(PomodoroEvent::DistractionWarning {
                session_id: session.session_id.clone(),
                sequence: session.focus.warning_sequence,
                current_category: category,
                remaining_milliseconds,
                notifications: notification_options(&session.settings),
            })
        } else {
            None
        };
        if let Some(warning) = warning {
            self.emit_event(warning);
        }
        Ok(())
    }

    pub fn monitoring_failed(
        &mut self,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<(), ServiceError> {
        self.tick(now, wall_now)?;
        if !self.has_active_session() {
            return Ok(());
        }

        let session = self.active_session_mut()?;
        let was_available = session.monitoring_available;
        session.monitoring_available = false;
        session.focus.reset();
        if was_available && matches!(session.timer.state(), TimerState::Running { .. }) {
            session
                .timer
                .pause(now, PauseReason::MonitoringUnavailable)
                .map_err(ServiceError::Timer)?;
            let event = PomodoroEvent::MonitoringUnavailablePaused {
                session_id: session.session_id.clone(),
                notifications: notification_options(&session.settings),
            };
            self.emit_event(event);
            self.save_checkpoint(wall_now)?;
        }
        Ok(())
    }

    pub fn acknowledge_distraction(
        &mut self,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<StateResponse, ServiceError> {
        self.tick(now, wall_now)?;
        let session = self.active_session_mut()?;
        if !matches!(
            session.timer.state(),
            TimerState::Running {
                phase: Phase::Focus { .. }
            }
        ) {
            return Err(ServiceError::Conflict(
                "there is no running focus distraction to continue".to_string(),
            ));
        }
        if session.focus.warning_pending {
            session.focus.warning_pending = false;
            session.focus.distracted_since = Some(now);
        }
        self.state(now)
    }

    pub fn should_monitor(&self) -> bool {
        self.has_active_session()
    }

    pub fn take_events(&mut self) -> Vec<PomodoroEvent> {
        std::mem::take(&mut self.pending_events)
    }

    pub fn chrome_notifications_after(&self, after: u64) -> ChromeNotificationPage {
        ChromeNotificationPage {
            items: self
                .chrome_notifications
                .iter()
                .filter(|notification| notification.id > after)
                .cloned()
                .collect(),
            latest_id: self.next_chrome_notification_id.saturating_sub(1),
            instance_id: self.notification_instance_id.clone(),
        }
    }

    pub fn apply_chrome_notification_action(
        &mut self,
        notification_id: u64,
        action: PomodoroNotificationAction,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<bool, ServiceError> {
        let token = self
            .chrome_notifications
            .iter()
            .find(|notification| notification.id == notification_id)
            .and_then(|notification| notification.event.action_token());
        let Some(token) = token else {
            return Ok(false);
        };
        self.apply_notification_action(&token, action, now, wall_now)
    }

    /// Apply a button click from a desktop notification only if the notification
    /// still describes the current session state. The HTTP thread serializes these
    /// commands with normal API requests, so duplicate clicks become harmless stale
    /// actions after the first successful transition.
    pub fn apply_notification_action(
        &mut self,
        token: &PomodoroActionToken,
        action: PomodoroNotificationAction,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<bool, ServiceError> {
        self.tick(now, wall_now)?;
        if !self.notification_token_is_current(token, now)? {
            return Ok(false);
        }

        match (token, action) {
            (PomodoroActionToken::Distraction { .. }, PomodoroNotificationAction::Continue) => {
                self.acknowledge_distraction(now, wall_now)?;
            }
            (PomodoroActionToken::Distraction { .. }, PomodoroNotificationAction::Pause) => {
                self.pause(now, wall_now)?;
            }
            (PomodoroActionToken::PhaseCompleted { .. }, PomodoroNotificationAction::Continue) => {
                self.confirm_next(now, wall_now)?;
            }
            (PomodoroActionToken::Paused { .. }, PomodoroNotificationAction::Continue) => {
                self.resume(now, wall_now)?;
            }
            (_, PomodoroNotificationAction::Stop) => {
                self.stop(now, wall_now)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn tick(&mut self, now: Instant, wall_now: DateTime<Utc>) -> Result<(), ServiceError> {
        if !self.has_active_session() {
            return Ok(());
        }

        self.accumulate(now);
        let event = self
            .active_session_mut()?
            .timer
            .tick(now)
            .map_err(ServiceError::Timer)?;

        if event.is_some() {
            if let Some(session) = self.current.as_mut() {
                session.focus.reset();
            }
        }

        if let Some(event) = event {
            let notification_event = self.current.as_ref().and_then(|session| match event {
                aw_pomodoro_core::TimerEvent::PhaseCompleted { completed, next } => {
                    Some(PomodoroEvent::PhaseCompleted {
                        session_id: session.session_id.clone(),
                        completed: phase_view(completed),
                        next: phase_view(next),
                        notifications: notification_options(&session.settings),
                    })
                }
                aw_pomodoro_core::TimerEvent::SessionCompleted { focus_intervals } => {
                    Some(PomodoroEvent::SessionCompleted {
                        session_id: session.session_id.clone(),
                        focus_intervals,
                        notifications: notification_options(&session.settings),
                    })
                }
                _ => None,
            });
            if let Some(notification_event) = notification_event {
                self.emit_event(notification_event);
            }
        }

        if matches!(
            self.current.as_ref().map(|session| session.timer.state()),
            Some(TimerState::Completed)
        ) {
            self.finalize(wall_now, None)?;
        } else if event.is_some()
            || self.current.as_ref().is_some_and(|session| {
                now.saturating_duration_since(session.checkpoint_anchor) >= CHECKPOINT_INTERVAL
            })
        {
            self.save_checkpoint(wall_now)?;
        }
        Ok(())
    }

    fn emit_event(&mut self, event: PomodoroEvent) {
        if event.chrome_notifications_enabled() {
            self.chrome_notifications.push_back(ChromeNotification {
                id: self.next_chrome_notification_id,
                event: event.clone(),
            });
            self.next_chrome_notification_id = self.next_chrome_notification_id.saturating_add(1);
            while self.chrome_notifications.len() > MAX_CHROME_NOTIFICATIONS {
                self.chrome_notifications.pop_front();
            }
        }
        self.pending_events.push(event);
    }

    pub fn state(&self, now: Instant) -> Result<StateResponse, ServiceError> {
        let Some(session) = &self.current else {
            return Ok(StateResponse::idle());
        };
        let timer_state = session.timer.state();
        let (state, phase, next_phase, pause_reason, interruption_reason) = match timer_state {
            TimerState::Ready => (PomodoroState::Idle, None, None, None, None),
            TimerState::Running { phase } => {
                let state = if matches!(phase, Phase::Focus { .. }) {
                    PomodoroState::RunningWork
                } else {
                    PomodoroState::RunningBreak
                };
                (state, Some(phase_view(phase)), None, None, None)
            }
            TimerState::Paused { phase, reason } => {
                let (state, reason) = match reason {
                    PauseReason::Manual => (PomodoroState::PausedManual, PauseReasonView::Manual),
                    PauseReason::Afk => (PomodoroState::PausedAfk, PauseReasonView::Afk),
                    PauseReason::MonitoringUnavailable => (
                        PomodoroState::PausedManual,
                        PauseReasonView::MonitoringUnavailable,
                    ),
                };
                (state, Some(phase_view(phase)), None, Some(reason), None)
            }
            TimerState::WaitingForConfirmation { completed, next } => (
                PomodoroState::WaitingConfirmation,
                Some(phase_view(completed)),
                Some(phase_view(next)),
                None,
                None,
            ),
            TimerState::Completed => (PomodoroState::Completed, None, None, None, None),
            TimerState::Interrupted { reason } => {
                let reason = match reason {
                    CoreInterruptionReason::UserStopped => InterruptionReason::User,
                    CoreInterruptionReason::ActivityWatchRestart => {
                        InterruptionReason::ActivitywatchRestart
                    }
                };
                (PomodoroState::Interrupted, None, None, None, Some(reason))
            }
        };

        let distraction = DistractionView {
            active: session.focus.classification == FocusClassification::Distracted,
            elapsed_milliseconds: session
                .focus
                .distracted_since
                .map(|since| duration_milliseconds(now.saturating_duration_since(since)))
                .unwrap_or(0),
            warning_pending: session.focus.warning_pending,
            warning_sequence: session.focus.warning_sequence,
        };

        Ok(StateResponse {
            state,
            session_id: Some(session.session_id.clone()),
            phase,
            next_phase,
            remaining_milliseconds: session
                .timer
                .remaining(now)
                .map_err(ServiceError::Timer)?
                .map(duration_milliseconds),
            completed_focus_intervals: session.timer.completed_focus_intervals(),
            planned_focus_intervals: session.settings.work_intervals,
            selected_categories: session.selected_categories.clone(),
            current_category: session.focus.current_category.clone(),
            distraction,
            afk: session.afk,
            monitoring_available: session.monitoring_available,
            pause_reason,
            interruption_reason,
        })
    }

    pub fn history(&mut self, page: u64, page_size: u64) -> Result<HistoryPage, ServiceError> {
        if page == 0 || !(1..=100).contains(&page_size) {
            return Err(ServiceError::Validation(
                "page must be positive and page_size must be between 1 and 100".to_string(),
            ));
        }
        self.flush_pending_history();
        let mut items = self
            .history_store
            .load_all()
            .map_err(|error| ServiceError::History(error.to_string()))?;

        let mut known: HashSet<String> = items.iter().map(|item| item.session_id.clone()).collect();
        for pending in &self.persisted.pending_history {
            if known.insert(pending.session_id.clone()) {
                items.push(pending.clone());
            }
        }
        items.sort_by_key(|item| std::cmp::Reverse(item.ended_at));

        let total = u64::try_from(items.len()).unwrap_or(u64::MAX);
        let start = usize::try_from((page - 1).saturating_mul(page_size)).unwrap_or(usize::MAX);
        let page_len = usize::try_from(page_size).unwrap_or(usize::MAX);
        let items = items.into_iter().skip(start).take(page_len).collect();
        Ok(HistoryPage {
            items,
            page,
            page_size,
            total,
        })
    }

    fn notification_token_is_current(
        &self,
        token: &PomodoroActionToken,
        now: Instant,
    ) -> Result<bool, ServiceError> {
        let state = self.state(now)?;
        let Some(current_session_id) = state.session_id.as_deref() else {
            return Ok(false);
        };

        Ok(match token {
            PomodoroActionToken::Distraction {
                session_id,
                sequence,
            } => {
                current_session_id == session_id
                    && state.state == PomodoroState::RunningWork
                    && state.distraction.warning_pending
                    && state.distraction.warning_sequence == *sequence
            }
            PomodoroActionToken::PhaseCompleted {
                session_id,
                completed,
                next,
            } => {
                current_session_id == session_id
                    && state.state == PomodoroState::WaitingConfirmation
                    && state.phase.as_ref() == Some(completed)
                    && state.next_phase.as_ref() == Some(next)
            }
            PomodoroActionToken::Paused { session_id, reason } => {
                current_session_id == session_id
                    && matches!(
                        state.state,
                        PomodoroState::PausedManual | PomodoroState::PausedAfk
                    )
                    && state.pause_reason == Some(*reason)
            }
        })
    }

    fn has_active_session(&self) -> bool {
        self.current.as_ref().is_some_and(|session| {
            matches!(
                session.timer.state(),
                TimerState::Running { .. }
                    | TimerState::Paused { .. }
                    | TimerState::WaitingForConfirmation { .. }
            )
        })
    }

    fn active_session_mut(&mut self) -> Result<&mut RuntimeSession, ServiceError> {
        if !self.has_active_session() {
            return Err(ServiceError::Conflict(
                "there is no active Pomodoro session".to_string(),
            ));
        }
        self.current.as_mut().ok_or_else(|| {
            ServiceError::Internal("active session disappeared unexpectedly".to_string())
        })
    }

    fn accumulate(&mut self, now: Instant) {
        let Some(session) = self.current.as_mut() else {
            return;
        };
        if session.finalized {
            return;
        }
        let timer_state = session.timer.state();
        let previous_anchor = session.metric_anchor;
        let elapsed = now.saturating_duration_since(session.metric_anchor);
        session.metric_anchor = now;
        let counted = if matches!(timer_state, TimerState::Running { .. }) {
            session
                .timer
                .remaining(previous_anchor)
                .ok()
                .flatten()
                .map_or(elapsed, |remaining| elapsed.min(remaining))
        } else {
            elapsed
        };
        let milliseconds = duration_milliseconds(counted);
        match timer_state {
            TimerState::Running {
                phase: Phase::Focus { .. },
            } => {
                add_saturating(&mut session.metrics.actual_focus_milliseconds, milliseconds);
                match session.focus.classification {
                    FocusClassification::Allowed => add_saturating(
                        &mut session.metrics.allowed_focus_milliseconds,
                        milliseconds,
                    ),
                    FocusClassification::Distracted => {
                        add_saturating(&mut session.metrics.distraction_milliseconds, milliseconds)
                    }
                    FocusClassification::Unknown => {}
                }
            }
            TimerState::Running { .. } => {
                add_saturating(&mut session.metrics.actual_break_milliseconds, milliseconds)
            }
            TimerState::Paused {
                reason: PauseReason::Manual,
                ..
            } => add_saturating(&mut session.metrics.manual_pause_milliseconds, milliseconds),
            TimerState::Paused {
                reason: PauseReason::Afk,
                ..
            } => add_saturating(&mut session.metrics.afk_pause_milliseconds, milliseconds),
            TimerState::Paused {
                reason: PauseReason::MonitoringUnavailable,
                ..
            } => add_saturating(
                &mut session.metrics.monitoring_pause_milliseconds,
                milliseconds,
            ),
            _ => {}
        }
    }

    fn save_checkpoint(&mut self, wall_now: DateTime<Utc>) -> Result<(), ServiceError> {
        let checkpoint = self
            .current
            .as_ref()
            .filter(|session| !session.finalized)
            .map(|session| session.checkpoint(wall_now));
        self.persisted.active_session = checkpoint;
        self.file_store
            .save(&self.persisted)
            .map_err(ServiceError::Storage)?;
        if let Some(session) = self.current.as_mut() {
            session.checkpoint_anchor = session.metric_anchor;
        }
        Ok(())
    }

    fn finalize(
        &mut self,
        wall_now: DateTime<Utc>,
        interruption_reason: Option<InterruptionReason>,
    ) -> Result<(), ServiceError> {
        let session = self.current.as_mut().ok_or_else(|| {
            ServiceError::Internal("cannot finalize a missing session".to_string())
        })?;
        if session.finalized {
            return Ok(());
        }
        let status = if interruption_reason.is_some() {
            SessionStatus::Interrupted
        } else {
            SessionStatus::Completed
        };
        let history = session.history(wall_now, status, interruption_reason);
        session.finalized = true;
        self.persisted.active_session = None;
        self.persisted.pending_history.push(history);
        self.file_store
            .save(&self.persisted)
            .map_err(ServiceError::Storage)?;
        self.flush_pending_history();
        Ok(())
    }

    fn flush_pending_history(&mut self) {
        let mut changed = false;
        while let Some(session) = self.persisted.pending_history.first().cloned() {
            if self.history_store.append(&session).is_err() {
                break;
            }
            self.persisted.pending_history.remove(0);
            changed = true;
        }
        if changed {
            let _ = self.file_store.save(&self.persisted);
        }
    }
}

impl RuntimeSession {
    fn checkpoint(&self, wall_now: DateTime<Utc>) -> SessionCheckpoint {
        SessionCheckpoint {
            session_id: self.session_id.clone(),
            started_at: self.started_at,
            updated_at: wall_now,
            settings: self.settings.clone(),
            selected_categories: self.selected_categories.clone(),
            planned_focus_intervals: self.settings.work_intervals,
            completed_focus_intervals: self.timer.completed_focus_intervals(),
            metrics: self.metrics,
        }
    }

    fn history(
        &self,
        wall_now: DateTime<Utc>,
        status: SessionStatus,
        interruption_reason: Option<InterruptionReason>,
    ) -> SessionHistory {
        history_from_parts(
            self.session_id.clone(),
            self.started_at,
            wall_now,
            status,
            interruption_reason,
            self.settings.clone(),
            self.selected_categories.clone(),
            self.settings.work_intervals,
            self.timer.completed_focus_intervals(),
            self.metrics,
        )
    }
}

fn history_from_restart(checkpoint: SessionCheckpoint) -> SessionHistory {
    history_from_parts(
        checkpoint.session_id,
        checkpoint.started_at,
        checkpoint.updated_at,
        SessionStatus::Interrupted,
        Some(InterruptionReason::ActivitywatchRestart),
        checkpoint.settings,
        checkpoint.selected_categories,
        checkpoint.planned_focus_intervals,
        checkpoint.completed_focus_intervals,
        checkpoint.metrics,
    )
}

#[allow(clippy::too_many_arguments)]
fn history_from_parts(
    session_id: String,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    status: SessionStatus,
    interruption_reason: Option<InterruptionReason>,
    settings: crate::model::SessionSettings,
    selected_categories: Vec<crate::model::CategoryPath>,
    planned_focus_intervals: u32,
    completed_focus_intervals: u32,
    metrics: PersistedMetrics,
) -> SessionHistory {
    let classified_focus = metrics
        .allowed_focus_milliseconds
        .saturating_add(metrics.distraction_milliseconds);
    let focus_percentage = if classified_focus == 0 {
        None
    } else {
        Some(metrics.allowed_focus_milliseconds as f64 / classified_focus as f64 * 100.0)
    };
    SessionHistory {
        session_id,
        started_at,
        ended_at: ended_at.max(started_at),
        status,
        interruption_reason,
        settings,
        selected_categories,
        planned_focus_intervals,
        completed_focus_intervals,
        actual_focus_milliseconds: metrics.actual_focus_milliseconds,
        actual_break_milliseconds: metrics.actual_break_milliseconds,
        manual_pause_milliseconds: metrics.manual_pause_milliseconds,
        afk_pause_milliseconds: metrics.afk_pause_milliseconds,
        monitoring_pause_milliseconds: metrics.monitoring_pause_milliseconds,
        distraction_count: metrics.distraction_count,
        distraction_milliseconds: metrics.distraction_milliseconds,
        allowed_focus_milliseconds: metrics.allowed_focus_milliseconds,
        focus_percentage,
    }
}

fn phase_view(phase: Phase) -> PhaseView {
    match phase {
        Phase::Focus { number } => PhaseView {
            kind: PhaseKind::Focus,
            focus_number: Some(number),
            after_focus: None,
        },
        Phase::ShortBreak { after_focus } => PhaseView {
            kind: PhaseKind::ShortBreak,
            focus_number: None,
            after_focus: Some(after_focus),
        },
        Phase::LongBreak { after_focus } => PhaseView {
            kind: PhaseKind::LongBreak,
            focus_number: None,
            after_focus: Some(after_focus),
        },
    }
}

fn deduplicate_categories(
    categories: Vec<crate::model::CategoryPath>,
) -> Vec<crate::model::CategoryPath> {
    let mut seen = HashSet::new();
    categories
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn notification_options(settings: &crate::model::SessionSettings) -> PomodoroNotificationOptions {
    PomodoroNotificationOptions {
        system_notifications: settings.system_notifications,
        chrome_notifications: settings.chrome_notifications,
        sound_enabled: settings.sound_enabled,
    }
}

fn duration_milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn add_saturating(value: &mut u64, amount: u64) {
    *value = value.saturating_add(amount);
}

#[derive(Debug)]
pub enum ServiceError {
    Validation(String),
    Conflict(String),
    Storage(String),
    History(String),
    Timer(TimerError),
    Internal(String),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(message) => write!(f, "invalid request: {message}"),
            Self::Conflict(message) => f.write_str(message),
            Self::Storage(message) => write!(f, "storage error: {message}"),
            Self::History(message) => write!(f, "history error: {message}"),
            Self::Timer(error) => write!(f, "timer error: {error}"),
            Self::Internal(message) => write!(f, "internal error: {message}"),
        }
    }
}

impl std::error::Error for ServiceError {}
