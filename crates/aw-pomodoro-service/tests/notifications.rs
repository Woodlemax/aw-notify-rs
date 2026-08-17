use std::time::{Duration, Instant};

use aw_pomodoro_service::{
    ActivityObservation, HistoryError, HistoryStore, PauseReasonView, PomodoroActionToken,
    PomodoroEvent, PomodoroNotificationAction, PomodoroService, PomodoroState, SessionHistory,
    SessionSettings, StartRequest,
};
use chrono::{DateTime, TimeZone, Utc};
use tempfile::TempDir;

#[derive(Default)]
struct MemoryHistory;

impl HistoryStore for MemoryHistory {
    fn append(&mut self, _session: &SessionHistory) -> Result<(), HistoryError> {
        Ok(())
    }

    fn load_all(&mut self) -> Result<Vec<SessionHistory>, HistoryError> {
        Ok(Vec::new())
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

fn settings(work_intervals: u32) -> SessionSettings {
    SessionSettings {
        focus_duration_seconds: 1,
        short_break_duration_seconds: 1,
        long_break_duration_seconds: 1,
        work_intervals,
        distraction_timeout_seconds: 1,
        system_notifications: true,
        chrome_notifications: false,
        sound_enabled: true,
    }
}

fn start(
    service: &mut PomodoroService,
    now: Instant,
    wall: DateTime<Utc>,
    work_intervals: u32,
) -> String {
    start_with_settings(service, now, wall, settings(work_intervals))
}

fn start_with_settings(
    service: &mut PomodoroService,
    now: Instant,
    wall: DateTime<Utc>,
    settings: SessionSettings,
) -> String {
    service
        .start(
            StartRequest {
                selected_categories: vec![vec!["Work".into()]],
                settings: Some(settings),
            },
            now,
            wall,
        )
        .unwrap()
        .session_id
        .unwrap()
}

#[test]
fn phase_and_session_completion_events_are_emitted_once_with_preferences() {
    let temp = TempDir::new().unwrap();
    let mut service =
        PomodoroService::open(temp.path().join("state.json"), Box::new(MemoryHistory)).unwrap();
    let now = Instant::now();
    let wall = wall_clock();
    let session_id = start(&mut service, now, wall, 1);

    service
        .tick(now + Duration::from_secs(1), at(wall, 1))
        .unwrap();
    let events = service.take_events();
    assert!(matches!(
        events.as_slice(),
        [PomodoroEvent::SessionCompleted {
            session_id: event_session,
            focus_intervals: 1,
            notifications,
        }] if event_session == &session_id
            && notifications.system_notifications
            && !notifications.chrome_notifications
            && notifications.sound_enabled
    ));

    service
        .tick(now + Duration::from_secs(2), at(wall, 2))
        .unwrap();
    assert!(service.take_events().is_empty());
}

#[test]
fn phase_action_is_atomic_and_duplicate_or_stale_clicks_are_ignored() {
    let temp = TempDir::new().unwrap();
    let mut service =
        PomodoroService::open(temp.path().join("state.json"), Box::new(MemoryHistory)).unwrap();
    let now = Instant::now();
    let wall = wall_clock();
    start(&mut service, now, wall, 2);

    service
        .tick(now + Duration::from_secs(1), at(wall, 1))
        .unwrap();
    let event = service.take_events().pop().unwrap();
    let PomodoroEvent::PhaseCompleted {
        session_id,
        completed,
        next,
        ..
    } = event
    else {
        panic!("expected phase completion event");
    };
    let token = PomodoroActionToken::PhaseCompleted {
        session_id,
        completed,
        next,
    };

    assert!(service
        .apply_notification_action(
            &token,
            PomodoroNotificationAction::Continue,
            now + Duration::from_secs(1),
            at(wall, 1),
        )
        .unwrap());
    assert_eq!(
        service.state(now + Duration::from_secs(1)).unwrap().state,
        PomodoroState::RunningBreak
    );
    assert!(!service
        .apply_notification_action(
            &token,
            PomodoroNotificationAction::Continue,
            now + Duration::from_secs(1),
            at(wall, 1),
        )
        .unwrap());
    assert!(!service
        .apply_notification_action(
            &token,
            PomodoroNotificationAction::Stop,
            now + Duration::from_secs(1),
            at(wall, 1),
        )
        .unwrap());
}

#[test]
fn distraction_buttons_allow_pause_and_reject_old_warning_sequences() {
    let temp = TempDir::new().unwrap();
    let mut service =
        PomodoroService::open(temp.path().join("state.json"), Box::new(MemoryHistory)).unwrap();
    let now = Instant::now();
    let wall = wall_clock();
    let mut distraction_settings = settings(2);
    distraction_settings.focus_duration_seconds = 10;
    let session_id = start_with_settings(&mut service, now, wall, distraction_settings);

    service
        .observe_activity(ActivityObservation::active(None), now, wall)
        .unwrap();
    service
        .observe_activity(
            ActivityObservation::active(None),
            now + Duration::from_secs(1),
            at(wall, 1),
        )
        .unwrap();
    let token = PomodoroActionToken::Distraction {
        session_id,
        sequence: 1,
    };

    assert!(service
        .apply_notification_action(
            &token,
            PomodoroNotificationAction::Pause,
            now + Duration::from_secs(1),
            at(wall, 1),
        )
        .unwrap());
    assert_eq!(
        service.state(now + Duration::from_secs(1)).unwrap().state,
        PomodoroState::PausedManual
    );
    assert!(!service
        .apply_notification_action(
            &token,
            PomodoroNotificationAction::Stop,
            now + Duration::from_secs(1),
            at(wall, 1),
        )
        .unwrap());
}

#[test]
fn stop_button_interrupts_only_the_current_notification_session() {
    let temp = TempDir::new().unwrap();
    let mut service =
        PomodoroService::open(temp.path().join("state.json"), Box::new(MemoryHistory)).unwrap();
    let now = Instant::now();
    let wall = wall_clock();
    let mut distraction_settings = settings(2);
    distraction_settings.focus_duration_seconds = 10;
    let session_id = start_with_settings(&mut service, now, wall, distraction_settings);
    service
        .observe_activity(ActivityObservation::active(None), now, wall)
        .unwrap();
    service
        .observe_activity(
            ActivityObservation::active(None),
            now + Duration::from_secs(1),
            at(wall, 1),
        )
        .unwrap();
    let token = PomodoroActionToken::Distraction {
        session_id,
        sequence: 1,
    };

    assert!(service
        .apply_notification_action(
            &token,
            PomodoroNotificationAction::Stop,
            now + Duration::from_secs(1),
            at(wall, 1),
        )
        .unwrap());
    assert_eq!(
        service.state(now + Duration::from_secs(1)).unwrap().state,
        PomodoroState::Interrupted
    );
    assert!(!service
        .apply_notification_action(
            &token,
            PomodoroNotificationAction::Stop,
            now + Duration::from_secs(1),
            at(wall, 1),
        )
        .unwrap());
}

#[test]
fn afk_continue_waits_for_return_and_then_resumes_only_matching_session() {
    let temp = TempDir::new().unwrap();
    let mut service =
        PomodoroService::open(temp.path().join("state.json"), Box::new(MemoryHistory)).unwrap();
    let now = Instant::now();
    let wall = wall_clock();
    let session_id = start(&mut service, now, wall, 2);
    service
        .observe_activity(
            ActivityObservation::afk(),
            now + Duration::from_millis(100),
            wall,
        )
        .unwrap();
    service.take_events();
    let token = PomodoroActionToken::Paused {
        session_id,
        reason: PauseReasonView::Afk,
    };

    assert!(service
        .apply_notification_action(
            &token,
            PomodoroNotificationAction::Continue,
            now + Duration::from_millis(100),
            wall,
        )
        .is_err());
    service
        .observe_activity(
            ActivityObservation::active(Some(vec!["Work".into()])),
            now + Duration::from_millis(200),
            wall,
        )
        .unwrap();
    assert!(service
        .apply_notification_action(
            &token,
            PomodoroNotificationAction::Continue,
            now + Duration::from_millis(200),
            wall,
        )
        .unwrap());
    assert_eq!(
        service
            .state(now + Duration::from_millis(200))
            .unwrap()
            .state,
        PomodoroState::RunningWork
    );
}
