use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aw_pomodoro_service::{
    ApiRequest, HistoryError, HistoryStore, PomodoroApi, PomodoroService, PomodoroSettings,
    SessionHistory,
};
use chrono::{TimeZone, Utc};
use serde_json::{json, Value};
use tempfile::TempDir;

const ALLOWED_ORIGIN: &str = "http://127.0.0.1:5600";

#[derive(Clone, Default)]
struct MemoryHistory {
    items: Arc<Mutex<Vec<SessionHistory>>>,
}

impl HistoryStore for MemoryHistory {
    fn append(&mut self, session: &SessionHistory) -> Result<(), HistoryError> {
        let mut items = self.items.lock().unwrap();
        if !items
            .iter()
            .any(|item| item.session_id == session.session_id)
        {
            items.push(session.clone());
        }
        Ok(())
    }

    fn load_all(&mut self) -> Result<Vec<SessionHistory>, HistoryError> {
        Ok(self.items.lock().unwrap().clone())
    }
}

fn open_api(temp: &TempDir, history: MemoryHistory) -> PomodoroApi {
    let service =
        PomodoroService::open(temp.path().join("pomodoro-state.json"), Box::new(history)).unwrap();
    PomodoroApi::new(service, vec![ALLOWED_ORIGIN.to_string()])
}

fn call(
    api: &mut PomodoroApi,
    method: &str,
    url: &str,
    body: Value,
    now: Instant,
    wall_now: chrono::DateTime<Utc>,
) -> (u16, Value) {
    let body = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).unwrap()
    };
    let response = api.handle_at(
        ApiRequest {
            method,
            url,
            origin: Some(ALLOWED_ORIGIN),
            content_type: matches!(method, "POST" | "PUT").then_some("application/json"),
            body: &body,
        },
        now,
        wall_now,
    );
    let value = if response.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&response.body).unwrap()
    };
    (response.status_code, value)
}

fn short_settings(work_intervals: u32) -> Value {
    json!({
        "focus_duration_seconds": 1,
        "short_break_duration_seconds": 1,
        "long_break_duration_seconds": 2,
        "work_intervals": work_intervals,
        "distraction_timeout_seconds": 1,
        "system_notifications": true,
        "chrome_notifications": false,
        "sound_enabled": true
    })
}

fn start_body(work_intervals: u32) -> Value {
    json!({
        "selected_categories": [["Work"], ["Work", "Programming"]],
        "settings": short_settings(work_intervals)
    })
}

fn wall_clock() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 17, 12, 0, 0)
        .single()
        .unwrap()
}

#[test]
fn settings_are_validated_saved_and_loaded_after_restart() {
    let temp = TempDir::new().unwrap();
    let history = MemoryHistory::default();
    let now = Instant::now();
    let wall = wall_clock();

    {
        let mut api = open_api(&temp, history.clone());
        let (status, defaults) = call(
            &mut api,
            "GET",
            "/pomodoro/settings",
            Value::Null,
            now,
            wall,
        );
        assert_eq!(status, 200);
        assert_eq!(defaults["focus_duration_seconds"], 1_500);
        assert_eq!(defaults["work_intervals"], 8);

        let mut settings: PomodoroSettings = serde_json::from_value(defaults).unwrap();
        settings.focus_duration_seconds = 600;
        settings.last_selected_categories = vec![vec!["Work".into()], vec!["Study".into()]];
        let (status, saved) = call(
            &mut api,
            "PUT",
            "/pomodoro/settings",
            serde_json::to_value(settings).unwrap(),
            now,
            wall,
        );
        assert_eq!(status, 200);
        assert_eq!(saved["focus_duration_seconds"], 600);
    }

    let mut reopened = open_api(&temp, history);
    let (status, saved) = call(
        &mut reopened,
        "GET",
        "/pomodoro/settings",
        Value::Null,
        now,
        wall,
    );
    assert_eq!(status, 200);
    assert_eq!(saved["focus_duration_seconds"], 600);
    assert_eq!(saved["last_selected_categories"][1][0], "Study");
}

#[test]
fn loopback_api_rejects_untrusted_origins_and_unsafe_content_types() {
    let temp = TempDir::new().unwrap();
    let history = MemoryHistory::default();
    let mut api = open_api(&temp, history);
    let now = Instant::now();
    let wall = wall_clock();

    let forbidden = api.handle_at(
        ApiRequest {
            method: "GET",
            url: "/pomodoro/state",
            origin: Some("https://malicious.example"),
            content_type: None,
            body: &[],
        },
        now,
        wall,
    );
    assert_eq!(forbidden.status_code, 403);
    assert_eq!(forbidden.allow_origin, None);

    let unsupported = api.handle_at(
        ApiRequest {
            method: "POST",
            url: "/pomodoro/pause",
            origin: Some(ALLOWED_ORIGIN),
            content_type: Some("text/plain"),
            body: b"{}",
        },
        now,
        wall,
    );
    assert_eq!(unsupported.status_code, 415);
    assert_eq!(unsupported.allow_origin.as_deref(), Some(ALLOWED_ORIGIN));

    let preflight = api.handle_at(
        ApiRequest {
            method: "OPTIONS",
            url: "/pomodoro/start",
            origin: Some(ALLOWED_ORIGIN),
            content_type: None,
            body: &[],
        },
        now,
        wall,
    );
    assert_eq!(preflight.status_code, 204);
}

#[test]
fn lifecycle_requires_confirmation_and_writes_interrupted_history() {
    let temp = TempDir::new().unwrap();
    let history = MemoryHistory::default();
    let mut api = open_api(&temp, history.clone());
    let base = Instant::now();
    let wall = wall_clock();

    let (status, state) = call(
        &mut api,
        "POST",
        "/pomodoro/start",
        start_body(3),
        base,
        wall,
    );
    assert_eq!(status, 200);
    assert_eq!(state["state"], "running_work");
    assert_eq!(state["phase"]["focus_number"], 1);

    let (status, waiting) = call(
        &mut api,
        "GET",
        "/pomodoro/state",
        Value::Null,
        base + Duration::from_secs(1),
        wall + chrono::Duration::seconds(1),
    );
    assert_eq!(status, 200);
    assert_eq!(waiting["state"], "waiting_confirmation");
    assert_eq!(waiting["next_phase"]["kind"], "short_break");

    let (_, break_state) = call(
        &mut api,
        "POST",
        "/pomodoro/confirm-next",
        json!({}),
        base + Duration::from_secs(10),
        wall + chrono::Duration::seconds(10),
    );
    assert_eq!(break_state["state"], "running_break");

    let (_, waiting_focus) = call(
        &mut api,
        "GET",
        "/pomodoro/state",
        Value::Null,
        base + Duration::from_secs(11),
        wall + chrono::Duration::seconds(11),
    );
    assert_eq!(waiting_focus["next_phase"]["kind"], "focus");
    call(
        &mut api,
        "POST",
        "/pomodoro/confirm-next",
        json!({}),
        base + Duration::from_secs(11),
        wall + chrono::Duration::seconds(11),
    );

    let (_, paused) = call(
        &mut api,
        "POST",
        "/pomodoro/pause",
        json!({}),
        base + Duration::from_millis(11_400),
        wall + chrono::Duration::milliseconds(11_400),
    );
    assert_eq!(paused["state"], "paused_manual");
    assert_eq!(paused["remaining_milliseconds"], 600);

    let (_, still_paused) = call(
        &mut api,
        "GET",
        "/pomodoro/state",
        Value::Null,
        base + Duration::from_secs(20),
        wall + chrono::Duration::seconds(20),
    );
    assert_eq!(still_paused["remaining_milliseconds"], 600);
    call(
        &mut api,
        "POST",
        "/pomodoro/resume",
        json!({}),
        base + Duration::from_secs(20),
        wall + chrono::Duration::seconds(20),
    );
    let (_, stopped) = call(
        &mut api,
        "POST",
        "/pomodoro/stop",
        json!({}),
        base + Duration::from_millis(20_200),
        wall + chrono::Duration::milliseconds(20_200),
    );
    assert_eq!(stopped["state"], "interrupted");
    assert_eq!(stopped["interruption_reason"], "user");

    let (status, history_page) = call(
        &mut api,
        "GET",
        "/pomodoro/history?page=1&page_size=20",
        Value::Null,
        base + Duration::from_millis(20_200),
        wall + chrono::Duration::milliseconds(20_200),
    );
    assert_eq!(status, 200);
    assert_eq!(history_page["total"], 1);
    let item = &history_page["items"][0];
    assert_eq!(item["status"], "interrupted");
    assert_eq!(item["interruption_reason"], "user");
    assert_eq!(item["completed_focus_intervals"], 1);
    assert_eq!(item["actual_focus_milliseconds"], 1_600);
    assert_eq!(item["actual_break_milliseconds"], 1_000);
    assert_eq!(item["manual_pause_milliseconds"], 8_600);
}

#[test]
fn final_focus_completes_session_and_writes_history() {
    let temp = TempDir::new().unwrap();
    let history = MemoryHistory::default();
    let mut api = open_api(&temp, history);
    let base = Instant::now();
    let wall = wall_clock();

    call(
        &mut api,
        "POST",
        "/pomodoro/start",
        start_body(1),
        base,
        wall,
    );
    let (_, completed) = call(
        &mut api,
        "GET",
        "/pomodoro/state",
        Value::Null,
        base + Duration::from_secs(1),
        wall + chrono::Duration::seconds(1),
    );
    assert_eq!(completed["state"], "completed");
    assert_eq!(completed["completed_focus_intervals"], 1);

    let (_, page) = call(
        &mut api,
        "GET",
        "/pomodoro/history",
        Value::Null,
        base + Duration::from_secs(1),
        wall + chrono::Duration::seconds(1),
    );
    assert_eq!(page["items"][0]["status"], "completed");
    assert!(page["items"][0]["interruption_reason"].is_null());
}

#[test]
fn delayed_poll_does_not_count_time_past_the_phase_deadline() {
    let temp = TempDir::new().unwrap();
    let history = MemoryHistory::default();
    let mut api = open_api(&temp, history);
    let base = Instant::now();
    let wall = wall_clock();

    call(
        &mut api,
        "POST",
        "/pomodoro/start",
        start_body(1),
        base,
        wall,
    );
    call(
        &mut api,
        "GET",
        "/pomodoro/state",
        Value::Null,
        base + Duration::from_secs(5),
        wall + chrono::Duration::seconds(5),
    );
    let (_, page) = call(
        &mut api,
        "GET",
        "/pomodoro/history",
        Value::Null,
        base + Duration::from_secs(5),
        wall + chrono::Duration::seconds(5),
    );
    assert_eq!(page["items"][0]["actual_focus_milliseconds"], 1_000);
}

#[test]
fn history_is_newest_first_and_paginated() {
    let temp = TempDir::new().unwrap();
    let history = MemoryHistory::default();
    let mut api = open_api(&temp, history);
    let base = Instant::now();
    let wall = wall_clock();

    for offset in 0..3 {
        let started = base + Duration::from_secs(offset * 2);
        let wall_started = wall + chrono::Duration::seconds(i64::try_from(offset * 2).unwrap());
        call(
            &mut api,
            "POST",
            "/pomodoro/start",
            start_body(2),
            started,
            wall_started,
        );
        call(
            &mut api,
            "POST",
            "/pomodoro/stop",
            json!({}),
            started + Duration::from_millis(100),
            wall_started + chrono::Duration::milliseconds(100),
        );
    }

    let (_, first_page) = call(
        &mut api,
        "GET",
        "/pomodoro/history?page=1&page_size=2",
        Value::Null,
        base + Duration::from_secs(10),
        wall + chrono::Duration::seconds(10),
    );
    let (_, second_page) = call(
        &mut api,
        "GET",
        "/pomodoro/history?page=2&page_size=2",
        Value::Null,
        base + Duration::from_secs(10),
        wall + chrono::Duration::seconds(10),
    );
    assert_eq!(first_page["total"], 3);
    assert_eq!(first_page["items"].as_array().unwrap().len(), 2);
    assert_eq!(second_page["items"].as_array().unwrap().len(), 1);
    assert!(
        first_page["items"][0]["ended_at"].as_str().unwrap()
            > first_page["items"][1]["ended_at"].as_str().unwrap()
    );
}

#[test]
fn active_session_becomes_restart_interruption_on_next_open() {
    let temp = TempDir::new().unwrap();
    let history = MemoryHistory::default();
    let base = Instant::now();
    let wall = wall_clock();

    {
        let mut api = open_api(&temp, history.clone());
        let mut body = start_body(2);
        body["settings"]["focus_duration_seconds"] = json!(10);
        call(&mut api, "POST", "/pomodoro/start", body, base, wall);
        call(
            &mut api,
            "GET",
            "/pomodoro/state",
            Value::Null,
            base + Duration::from_secs(2),
            wall + chrono::Duration::seconds(2),
        );
    }

    let mut reopened = open_api(&temp, history);
    let (_, state) = call(
        &mut reopened,
        "GET",
        "/pomodoro/state",
        Value::Null,
        base + Duration::from_secs(3),
        wall + chrono::Duration::seconds(3),
    );
    assert_eq!(state["state"], "idle");

    let (_, page) = call(
        &mut reopened,
        "GET",
        "/pomodoro/history",
        Value::Null,
        base + Duration::from_secs(3),
        wall + chrono::Duration::seconds(3),
    );
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["status"], "interrupted");
    assert_eq!(
        page["items"][0]["interruption_reason"],
        "activitywatch_restart"
    );
    assert_eq!(page["items"][0]["actual_focus_milliseconds"], 2_000);
}

#[test]
fn invalid_transitions_and_session_mutations_return_conflict() {
    let temp = TempDir::new().unwrap();
    let history = MemoryHistory::default();
    let mut api = open_api(&temp, history);
    let base = Instant::now();
    let wall = wall_clock();

    let (status, _) = call(&mut api, "POST", "/pomodoro/resume", json!({}), base, wall);
    assert_eq!(status, 409);

    call(
        &mut api,
        "POST",
        "/pomodoro/start",
        start_body(2),
        base,
        wall,
    );
    let (status, _) = call(
        &mut api,
        "POST",
        "/pomodoro/start",
        start_body(2),
        base,
        wall,
    );
    assert_eq!(status, 409);

    let (status, _) = call(
        &mut api,
        "PUT",
        "/pomodoro/settings",
        serde_json::to_value(PomodoroSettings::default()).unwrap(),
        base,
        wall,
    );
    assert_eq!(status, 409);

    call(&mut api, "POST", "/pomodoro/stop", json!({}), base, wall);
    let (status, _) = call(&mut api, "POST", "/pomodoro/stop", json!({}), base, wall);
    assert_eq!(status, 409);
}

#[test]
fn missing_categories_and_invalid_ranges_are_rejected() {
    let temp = TempDir::new().unwrap();
    let history = MemoryHistory::default();
    let mut api = open_api(&temp, history);
    let now = Instant::now();
    let wall = wall_clock();

    let (status, response) = call(
        &mut api,
        "POST",
        "/pomodoro/start",
        json!({"selected_categories": []}),
        now,
        wall,
    );
    assert_eq!(status, 400);
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("at least one category"));

    let settings = PomodoroSettings {
        work_intervals: 0,
        ..PomodoroSettings::default()
    };
    let (status, _) = call(
        &mut api,
        "PUT",
        "/pomodoro/settings",
        serde_json::to_value(settings).unwrap(),
        now,
        wall,
    );
    assert_eq!(status, 400);
}
