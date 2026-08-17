# Pomodoro loopback API

The API is served by `aw-notify` and remains available while the ActivityWatch
web UI and Chrome are closed.

Base URL: `http://127.0.0.1:5667/pomodoro`

## Security

- The server binds only to `127.0.0.1`.
- Browser requests with an `Origin` header must exactly match an entry in
  `pomodoro_allowed_origins`.
- Native loopback clients without an `Origin` header are allowed.
- Every `POST` and `PUT` requires `Content-Type: application/json`.
- Request bodies are limited to 64 KiB.
- Durations are positive integers no greater than 86,400 seconds.
- `work_intervals` is between 1 and 100.
- History `page_size` is between 1 and 100.

Successful responses use JSON. Errors use this shape:

```json
{
  "error": {
    "code": "invalid_transition",
    "message": "there is no active Pomodoro session"
  }
}
```

## Endpoints

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/state` | Current timer state and remaining time. |
| `POST` | `/start` | Start a session and save its settings/category snapshot. |
| `POST` | `/pause` | Pause the current phase manually. |
| `POST` | `/resume` | Resume a paused phase. |
| `POST` | `/stop` | Interrupt the session and write history. |
| `POST` | `/confirm-next` | Start the phase waiting for manual confirmation. |
| `POST` | `/distraction/continue` | Acknowledge the current warning and start a fresh distraction timeout. |
| `GET` | `/settings` | Read saved defaults and the last category selection. |
| `PUT` | `/settings` | Validate and save defaults. Rejected during an active session. |
| `GET` | `/history?page=1&page_size=20` | Read newest-first paginated history. |
| `GET` | `/notifications?after=42` | Read Chrome notification events newer than cursor `42`. |
| `POST` | `/notifications/43/action` | Apply an action from Chrome notification `43`. |

## Chrome notification queue

`GET /notifications?after=42` returns a bounded in-memory queue:

```json
{
  "instance_id": "45c73b80-d264-4d5f-b78f-623cb8faf6b5",
  "latest_id": 43,
  "items": [
    {
      "id": 43,
      "event": {
        "type": "distraction_warning",
        "session_id": "session-id",
        "sequence": 2,
        "remaining_milliseconds": 1200000,
        "notifications": {
          "system_notifications": true,
          "chrome_notifications": true,
          "sound_enabled": true
        }
      }
    }
  ]
}
```

The extension stores both `instance_id` and `latest_id`. A changed instance ID
means `aw-notify` restarted, so the extension safely resets its cursor. Only
events with `chrome_notifications = true` enter this queue, and the newest 100
events are retained.

Actions use JSON and are validated against the exact session/warning/phase
represented by the notification:

```json
{ "action": "pause" }
```

Allowed values are `continue`, `pause`, and `stop`. The response is
`{"applied":true}` only for a current valid action. Repeated or stale actions
return `{"applied":false}` without changing timer state.

## Settings

`GET /settings` and `PUT /settings` use:

```json
{
  "focus_duration_seconds": 1500,
  "short_break_duration_seconds": 300,
  "long_break_duration_seconds": 900,
  "work_intervals": 8,
  "distraction_timeout_seconds": 30,
  "system_notifications": true,
  "chrome_notifications": true,
  "sound_enabled": true,
  "last_selected_categories": [
    ["Work"],
    ["Projects", "ActivityWatch"]
  ]
}
```

Category paths are arrays so parent/child relationships remain unambiguous.
Categorization matching is implemented in the next stage.

## Start request

`settings` is optional. If omitted, the saved settings are captured. If
provided, they become the saved defaults for the next session too.

```json
{
  "selected_categories": [
    ["Work"],
    ["Projects", "ActivityWatch"]
  ],
  "settings": {
    "focus_duration_seconds": 1500,
    "short_break_duration_seconds": 300,
    "long_break_duration_seconds": 900,
    "work_intervals": 8,
    "distraction_timeout_seconds": 30,
    "system_notifications": true,
    "chrome_notifications": true,
    "sound_enabled": true
  }
}
```

A session cannot start without at least one category, and a second active
session is rejected with HTTP `409`.

## State response

```json
{
  "state": "running_work",
  "session_id": "1b431711-4b4b-4bb4-a777-9f8436b36f8d",
  "phase": {
    "kind": "focus",
    "focus_number": 1
  },
  "remaining_milliseconds": 1499550,
  "completed_focus_intervals": 0,
  "planned_focus_intervals": 8,
  "selected_categories": [["Work"]],
  "distraction": {
    "active": false,
    "elapsed_milliseconds": 0,
    "warning_pending": false,
    "warning_sequence": 0
  },
  "afk": false,
  "monitoring_available": true
}
```

Possible `state` values:

- `idle`
- `running_work`
- `running_break`
- `paused_manual`
- `paused_afk`
- `waiting_confirmation`
- `completed`
- `interrupted`

During `waiting_confirmation`, `phase` is the completed phase and `next_phase`
is the phase that will start only after `POST /confirm-next`.

`current_category` is the deepest matching Categorization path. It is omitted
for `Uncategorized`, breaks, and pauses. Selecting a parent category includes
all descendants by exact path prefix; similarly named sibling categories are
not included.

Focus monitoring runs only during work phases. Switching between selected
categories stays focused. An unselected or uncategorized activity starts the
configured timeout; returning before it expires cancels the warning. Continuous
distraction emits a new warning after each fresh timeout. The warning's
Continue action uses `/distraction/continue`; Pause and Stop use the existing
endpoints.

`afk` is sourced from the ActivityWatch `afkstatus` bucket. A running work or
break phase is automatically paused with `pause_reason: "afk"`, and returning
does not resume it. If ActivityWatch monitoring becomes unavailable, the phase
is paused with `pause_reason: "monitoring_unavailable"`. Both cases require a
manual `/resume` after the condition has cleared.

## History

Terminal sessions are written as ActivityWatch events to bucket
`aw-pomodoro_<hostname>` with bucket type `pomodoro.session`. An active
checkpoint is also stored locally once per second.

If `aw-notify` restarts while a session is active, the next startup does not
restore the timer. Instead, it writes an `interrupted` history record with
`interruption_reason: "activitywatch_restart"`.

Each history item contains:

- session ID, start and end timestamps;
- `completed` or `interrupted` status and optional interruption reason;
- settings and selected category snapshot;
- planned and completed focus interval counts;
- actual focus, break, manual pause, AFK pause, and monitoring pause milliseconds;
- distraction count/duration, allowed focus time, and focus percentage.

`focus_percentage` is calculated from classified focus time as
`allowed / (allowed + distracted) * 100`. Initial unclassified sampling time is
excluded from this ratio.
