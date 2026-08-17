# Pomodoro focus monitoring

`aw-notify` samples ActivityWatch once per second while a Pomodoro session is
active. The timer remains owned by the background process and does not depend
on the web UI or Chrome being open.

## Sources

- the newest `currentwindow` event for the local hostname;
- the newest `afkstatus` event for the local hostname;
- the newest `web.tab.current` event matching the active browser process;
- the effective Categorization rules saved in the ActivityWatch `classes`
  setting.

Window and active-tab string fields are classified together. Rules use the
same `fancy-regex` version and deepest-category/equal-depth ordering semantics
as ActivityWatch's query engine. Rules are refreshed every five seconds and
bucket discovery every thirty seconds.

Watcher events older than fifteen seconds are treated as unavailable
monitoring rather than silently assuming focus. A running phase is then paused
and requires manual continuation after a successful observation.

## Category matching

Selected categories are immutable for the session. A current category is
allowed when its full path begins with any selected path:

- selected `Work` allows `Work > Programming > ActivityWatch`;
- selected `Study > Rust` allows `Study > Rust > Books`;
- selected `Work` does not allow the separate category `Work personal`.

`Uncategorized` cannot be selected as a focus category and is represented by a
missing `current_category` in the local API.

## Distraction timing

The timeout uses monotonic time. A transition to any unselected category starts
one continuous episode. Switching between unselected categories does not reset
it. Returning to any selected category immediately cancels the pending timeout.

When the timeout expires, the service increments `warning_sequence`, marks
`warning_pending`, and starts a fresh timeout. This repeats for as long as the
user remains distracted. `/pomodoro/distraction/continue` acknowledges only the
currently pending warning; repeated calls are idempotent and do not postpone the
next warning.

Pause and Stop use the normal Pomodoro commands. Windows Toast buttons are
implemented by `aw-notify`; Chrome delivery remains a separate integration.
See [Pomodoro Windows notifications](pomodoro-windows-notifications.md).

## AFK

AFK pauses both work and break phases. Returning only clears the AFK condition;
the frozen phase remains `paused_afk` until the user calls `/pomodoro/resume`.
Categories and distraction time are ignored during every break and pause.
