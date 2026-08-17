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
| `GET` | `/settings` | Read saved defaults and the last category selection. |
| `PUT` | `/settings` | Validate and save defaults. Rejected during an active session. |
| `GET` | `/history?page=1&page_size=20` | Read newest-first paginated history. |

Notification queue endpoints are intentionally deferred to the Chrome
integration stage.

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
    "elapsed_milliseconds": 0
  }
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

The current category and distraction fields are neutral until Categorization
integration is added.

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
- actual focus, break, manual pause, and AFK pause milliseconds;
- distraction count/duration, allowed focus time, and focus percentage.

Distraction-related metrics and `focus_percentage` remain unset until the
Categorization stage supplies those measurements.
