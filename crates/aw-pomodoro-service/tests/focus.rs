use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aw_pomodoro_service::{
    ActivityObservation, HistoryError, HistoryStore, PauseReasonView, PomodoroEvent,
    PomodoroService, PomodoroState, SessionHistory, SessionSettings, StartRequest,
};
use chrono::{DateTime, TimeZone, Utc};
use tempfile::TempDir;

#[derive(Clone, Default)]
struct MemoryHistory {
    items: Arc<Mutex<Vec<SessionHistory>>>,
}

impl HistoryStore for MemoryHistory {
    fn append(&mut self, session: &SessionHistory) -> Result<(), HistoryError> {
        self.items.lock().unwrap().push(session.clone());
        Ok(())
    }

    fn load_all(&mut self) -> Result<Vec<SessionHistory>, HistoryError> {
        Ok(self.items.lock().unwrap().clone())
    }
}

fn wall_clock() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 17, 12, 0, 0)
        .single()
        .unwrap()
}

fn at(wall: DateTime<Utc>, seconds: u64) -> DateTime<Utc> {
    wall + chrono::Duration::seconds(i64::try_from(seconds).unwrap())
}

fn settings(focus: u64, short_break: u64, timeout: u64) -> SessionSettings {
    SessionSettings {
        focus_duration_seconds: focus,
        short_break_duration_seconds: short_break,
        long_break_duration_seconds: short_break,
        work_intervals: 2,
        distraction_timeout_seconds: timeout,
        system_notifications: true,
        chrome_notifications: true,
        sound_enabled: true,
    }
}

fn start(
    service: &mut PomodoroService,
    now: Instant,
    wall: DateTime<Utc>,
    settings: SessionSettings,
) {
    service
        .start(
            StartRequest {
                selected_categories: vec![vec!["Work".into()], vec!["Study".into(), "Rust".into()]],
                settings: Some(settings),
            },
            now,
            wall,
        )
        .unwrap();
}

fn observation(parts: &[&str]) -> ActivityObservation {
    ActivityObservation::active(Some(parts.iter().map(|part| (*part).to_string()).collect()))
}

#[test]
fn selected_parents_and_multiple_branches_stay_focused() {
    let temp = TempDir::new().unwrap();
    let mut service = PomodoroService::open(
        temp.path().join("state.json"),
        Box::new(MemoryHistory::default()),
    )
    .unwrap();
    let now = Instant::now();
    let wall = wall_clock();
    start(&mut service, now, wall, settings(100, 10, 3));

    service
        .observe_activity(observation(&["Work", "Programming"]), now, wall)
        .unwrap();
    service
        .observe_activity(
            observation(&["Study", "Rust", "Books"]),
            now + Duration::from_secs(2),
            at(wall, 2),
        )
        .unwrap();

    let state = service.state(now + Duration::from_secs(2)).unwrap();
    assert_eq!(state.state, PomodoroState::RunningWork);
    assert!(!state.distraction.active);
    assert_eq!(
        state.current_category.unwrap(),
        vec!["Study", "Rust", "Books"]
    );
    assert!(service.take_events().is_empty());
}

#[test]
fn returning_before_timeout_cancels_and_continuous_distraction_repeats() {
    let temp = TempDir::new().unwrap();
    let history = MemoryHistory::default();
    let mut service =
        PomodoroService::open(temp.path().join("state.json"), Box::new(history.clone())).unwrap();
    let now = Instant::now();
    let wall = wall_clock();
    start(&mut service, now, wall, settings(100, 10, 3));

    service
        .observe_activity(observation(&["Media"]), now, wall)
        .unwrap();
    service
        .observe_activity(
            observation(&["Work"]),
            now + Duration::from_secs(2),
            at(wall, 2),
        )
        .unwrap();
    assert!(service.take_events().is_empty());

    service
        .observe_activity(
            ActivityObservation::active(None),
            now + Duration::from_secs(3),
            at(wall, 3),
        )
        .unwrap();
    service
        .observe_activity(
            ActivityObservation::active(None),
            now + Duration::from_secs(6),
            at(wall, 6),
        )
        .unwrap();
    assert!(matches!(
        service.take_events().as_slice(),
        [PomodoroEvent::DistractionWarning {
            sequence: 1,
            current_category: None,
            ..
        }]
    ));

    service
        .observe_activity(
            observation(&["Media", "Video"]),
            now + Duration::from_secs(9),
            at(wall, 9),
        )
        .unwrap();
    assert!(matches!(
        service.take_events().as_slice(),
        [PomodoroEvent::DistractionWarning { sequence: 2, .. }]
    ));

    let state = service
        .acknowledge_distraction(now + Duration::from_secs(9), at(wall, 9))
        .unwrap();
    assert!(!state.distraction.warning_pending);
    service
        .acknowledge_distraction(now + Duration::from_secs(10), at(wall, 10))
        .unwrap();
    service
        .observe_activity(
            observation(&["Media"]),
            now + Duration::from_secs(11),
            at(wall, 11),
        )
        .unwrap();
    assert!(service.take_events().is_empty());
    service
        .observe_activity(
            observation(&["Media"]),
            now + Duration::from_secs(12),
            at(wall, 12),
        )
        .unwrap();
    assert!(matches!(
        service.take_events().as_slice(),
        [PomodoroEvent::DistractionWarning { sequence: 3, .. }]
    ));

    service
        .stop(now + Duration::from_secs(13), at(wall, 13))
        .unwrap();
    let saved = history.items.lock().unwrap();
    assert_eq!(saved[0].distraction_count, 2);
    assert!(saved[0].distraction_milliseconds > 0);
    assert!(saved[0].allowed_focus_milliseconds > 0);
    assert!(saved[0].focus_percentage.is_some());
}

#[test]
fn categories_are_ignored_during_breaks() {
    let temp = TempDir::new().unwrap();
    let mut service = PomodoroService::open(
        temp.path().join("state.json"),
        Box::new(MemoryHistory::default()),
    )
    .unwrap();
    let now = Instant::now();
    let wall = wall_clock();
    start(&mut service, now, wall, settings(1, 10, 2));
    service
        .tick(now + Duration::from_secs(1), at(wall, 1))
        .unwrap();
    assert!(matches!(
        service.take_events().as_slice(),
        [PomodoroEvent::PhaseCompleted { .. }]
    ));
    service
        .confirm_next(now + Duration::from_secs(1), at(wall, 1))
        .unwrap();

    service
        .observe_activity(
            ActivityObservation::active(None),
            now + Duration::from_secs(2),
            at(wall, 2),
        )
        .unwrap();
    service
        .observe_activity(
            observation(&["Media"]),
            now + Duration::from_secs(7),
            at(wall, 7),
        )
        .unwrap();
    let state = service.state(now + Duration::from_secs(7)).unwrap();
    assert_eq!(state.state, PomodoroState::RunningBreak);
    assert!(!state.distraction.active);
    assert!(state.current_category.is_none());
    assert!(service.take_events().is_empty());

    service
        .observe_activity(
            ActivityObservation::afk(),
            now + Duration::from_secs(8),
            at(wall, 8),
        )
        .unwrap();
    assert_eq!(
        service.state(now + Duration::from_secs(8)).unwrap().state,
        PomodoroState::PausedAfk
    );
    service
        .observe_activity(
            observation(&["Media"]),
            now + Duration::from_secs(9),
            at(wall, 9),
        )
        .unwrap();
    assert_eq!(
        service
            .resume(now + Duration::from_secs(9), at(wall, 9))
            .unwrap()
            .state,
        PomodoroState::RunningBreak
    );
}

#[test]
fn afk_pauses_and_requires_manual_resume_after_return() {
    let temp = TempDir::new().unwrap();
    let history = MemoryHistory::default();
    let mut service =
        PomodoroService::open(temp.path().join("state.json"), Box::new(history.clone())).unwrap();
    let now = Instant::now();
    let wall = wall_clock();
    start(&mut service, now, wall, settings(100, 10, 3));

    service
        .observe_activity(
            ActivityObservation::afk(),
            now + Duration::from_secs(1),
            at(wall, 1),
        )
        .unwrap();
    assert!(matches!(
        service.take_events().as_slice(),
        [PomodoroEvent::AfkPaused { .. }]
    ));
    let paused = service.state(now + Duration::from_secs(1)).unwrap();
    assert_eq!(paused.state, PomodoroState::PausedAfk);
    assert_eq!(paused.pause_reason, Some(PauseReasonView::Afk));
    assert!(service
        .resume(now + Duration::from_secs(2), at(wall, 2))
        .is_err());

    service
        .observe_activity(
            observation(&["Work"]),
            now + Duration::from_secs(5),
            at(wall, 5),
        )
        .unwrap();
    assert_eq!(
        service.state(now + Duration::from_secs(5)).unwrap().state,
        PomodoroState::PausedAfk
    );
    assert_eq!(
        service
            .resume(now + Duration::from_secs(5), at(wall, 5))
            .unwrap()
            .state,
        PomodoroState::RunningWork
    );
    service
        .stop(now + Duration::from_secs(6), at(wall, 6))
        .unwrap();
    assert!(history.items.lock().unwrap()[0].afk_pause_milliseconds >= 4_000);
}

#[test]
fn monitoring_failure_pauses_until_recovery_and_manual_resume() {
    let temp = TempDir::new().unwrap();
    let mut service = PomodoroService::open(
        temp.path().join("state.json"),
        Box::new(MemoryHistory::default()),
    )
    .unwrap();
    let now = Instant::now();
    let wall = wall_clock();
    start(&mut service, now, wall, settings(100, 10, 3));

    service
        .monitoring_failed(now + Duration::from_secs(1), at(wall, 1))
        .unwrap();
    assert!(matches!(
        service.take_events().as_slice(),
        [PomodoroEvent::MonitoringUnavailablePaused { .. }]
    ));
    let paused = service.state(now + Duration::from_secs(1)).unwrap();
    assert_eq!(paused.state, PomodoroState::PausedManual);
    assert_eq!(
        paused.pause_reason,
        Some(PauseReasonView::MonitoringUnavailable)
    );
    assert!(!paused.monitoring_available);
    assert!(service
        .resume(now + Duration::from_secs(2), at(wall, 2))
        .is_err());

    service
        .observe_activity(
            observation(&["Work"]),
            now + Duration::from_secs(4),
            at(wall, 4),
        )
        .unwrap();
    assert!(
        service
            .state(now + Duration::from_secs(4))
            .unwrap()
            .monitoring_available
    );
    assert_eq!(
        service
            .resume(now + Duration::from_secs(4), at(wall, 4))
            .unwrap()
            .state,
        PomodoroState::RunningWork
    );
}
