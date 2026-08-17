# Pomodoro Windows notifications

`aw-notify` owns Pomodoro Windows Toast notifications. They continue to work
when the ActivityWatch web UI, Chrome, or the whole browser is closed, because
the notification callback sends commands directly back to the background
Pomodoro service.

## Events and actions

| Event | Windows Toast actions |
| --- | --- |
| Distraction timeout | Continue, Pause, Stop |
| Work or break phase completed | Start next phase, Stop |
| AFK pause | Continue after returning, Stop |
| ActivityWatch monitoring unavailable | Continue after recovery, Stop |
| Pomodoro session completed | No actions |

Continue on a distraction acknowledges only the current warning. If the user
remains outside the selected categories, the next notification appears after a
new distraction timeout. Pause freezes the timer and Stop writes an interrupted
history record.

Every actionable Toast carries the session ID and the exact warning, phase, or
pause reason it represents. The background service validates that token before
applying the command. A repeated click, an old Toast from an earlier phase, or
a Toast from an earlier session is ignored.

## Notification and sound settings

- `system_notifications = true` shows Windows Toast notifications.
- `sound_enabled = true` plays one Windows notification sound per event.
- When both settings are enabled, the Toast itself owns the sound.
- When visual system notifications are disabled but sound remains enabled,
  `aw-notify` plays one system signal without creating a hidden Toast.
- Chrome notification delivery is controlled independently and is implemented
  by the Chrome integration.

Notification text follows the ActivityWatch `locale` setting for Russian and
English. Other locale values currently fall back to English.

## Windows build

Build with an installed Windows Rust toolchain:

```powershell
cargo build --release
```

For a cross-build from Linux, install the `x86_64-pc-windows-gnu` Rust target
and a MinGW-w64 compiler, then run:

```bash
cargo build --release --target x86_64-pc-windows-gnu
```

The resulting executable is standalone and imports only Windows system DLLs;
it does not require a separately shipped `libunwind.dll`.

## Manual check

1. Start `aw-notify` and ActivityWatch.
2. Start a Pomodoro session with system notifications enabled.
3. Switch to an unselected category until the distraction timeout expires.
4. Confirm that Continue, Pause, and Stop are present in the Toast.
5. Select Pause and verify that `/pomodoro/state` becomes `paused_manual`.
6. Start the next phase from a phase-complete Toast and verify that a second
   click on the old Toast has no effect.
7. Close Chrome and the web UI, then repeat the test to confirm that delivery
   is owned by `aw-notify`.
