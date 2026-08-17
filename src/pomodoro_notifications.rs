use anyhow::Result;
use aw_pomodoro_service::{
    PauseReasonView, PhaseKind, PhaseView, PomodoroActionToken, PomodoroEvent,
    PomodoroNotificationAction, PomodoroNotificationOptions,
};
use crossbeam_channel::Sender;
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct PomodoroActionRequest {
    pub token: PomodoroActionToken,
    pub action: PomodoroNotificationAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationLocale {
    English,
    Russian,
}

impl NotificationLocale {
    pub fn from_code(code: Option<&str>) -> Self {
        if code.is_some_and(|value| value.eq_ignore_ascii_case("ru")) {
            Self::Russian
        } else {
            Self::English
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotificationButton {
    pub label: String,
    #[serde(skip)]
    pub action: PomodoroNotificationAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PomodoroNotificationSpec {
    pub title: String,
    pub message: String,
    pub buttons: Vec<NotificationButton>,
    pub token: Option<PomodoroActionToken>,
    pub options: PomodoroNotificationOptions,
}

pub fn notification_spec(
    event: &PomodoroEvent,
    locale: NotificationLocale,
) -> PomodoroNotificationSpec {
    match event {
        PomodoroEvent::DistractionWarning {
            session_id,
            sequence,
            current_category,
            remaining_milliseconds,
            notifications,
        } => {
            let category = current_category.as_ref().map(|path| path.join(" > "));
            let remaining = format_duration(*remaining_milliseconds, locale);
            let message = match (locale, category) {
                (NotificationLocale::Russian, Some(category)) => format!(
                    "Вы переключились на «{category}». До конца рабочего этапа {remaining}."
                ),
                (NotificationLocale::Russian, None) => format!(
                    "Текущая активность не имеет категории. До конца рабочего этапа {remaining}."
                ),
                (NotificationLocale::English, Some(category)) => format!(
                    "You switched to “{category}”. The work phase has {remaining} remaining."
                ),
                (NotificationLocale::English, None) => format!(
                    "The current activity is uncategorized. The work phase has {remaining} remaining."
                ),
            };
            PomodoroNotificationSpec {
                title: translated(
                    locale,
                    "ActivityWatch · Focus lost",
                    "ActivityWatch · Фокус потерян",
                ),
                message,
                buttons: vec![
                    button(locale, PomodoroNotificationAction::Continue),
                    button(locale, PomodoroNotificationAction::Pause),
                    button(locale, PomodoroNotificationAction::Stop),
                ],
                token: Some(PomodoroActionToken::Distraction {
                    session_id: session_id.clone(),
                    sequence: *sequence,
                }),
                options: *notifications,
            }
        }
        PomodoroEvent::PhaseCompleted {
            session_id,
            completed,
            next,
            notifications,
        } => {
            let (title_en, title_ru) = if completed.kind == PhaseKind::Focus {
                (
                    "ActivityWatch · Work phase complete",
                    "ActivityWatch · Работа завершена",
                )
            } else {
                (
                    "ActivityWatch · Break complete",
                    "ActivityWatch · Перерыв завершён",
                )
            };
            PomodoroNotificationSpec {
                title: translated(locale, title_en, title_ru),
                message: next_phase_message(next, locale),
                buttons: vec![
                    start_button(locale),
                    button(locale, PomodoroNotificationAction::Stop),
                ],
                token: Some(PomodoroActionToken::PhaseCompleted {
                    session_id: session_id.clone(),
                    completed: completed.clone(),
                    next: next.clone(),
                }),
                options: *notifications,
            }
        }
        PomodoroEvent::SessionCompleted {
            focus_intervals,
            notifications,
            ..
        } => PomodoroNotificationSpec {
            title: translated(
                locale,
                "ActivityWatch · Pomodoro complete",
                "ActivityWatch · Томато завершён",
            ),
            message: match locale {
                NotificationLocale::English => {
                    format!("Completed work phases: {focus_intervals}.")
                }
                NotificationLocale::Russian => {
                    format!("Выполнено рабочих этапов: {focus_intervals}.")
                }
            },
            buttons: Vec::new(),
            token: None,
            options: *notifications,
        },
        PomodoroEvent::AfkPaused {
            session_id,
            notifications,
        } => paused_spec(session_id, PauseReasonView::Afk, *notifications, locale),
        PomodoroEvent::MonitoringUnavailablePaused {
            session_id,
            notifications,
        } => paused_spec(
            session_id,
            PauseReasonView::MonitoringUnavailable,
            *notifications,
            locale,
        ),
    }
}

pub fn show_notification(
    event: &PomodoroEvent,
    locale: NotificationLocale,
    action_tx: &Sender<PomodoroActionRequest>,
    output_only: bool,
) -> Result<()> {
    let spec = notification_spec(event, locale);
    if !spec.options.system_notifications && !spec.options.sound_enabled {
        return Ok(());
    }

    if output_only {
        let value = serde_json::json!({
            "type": "pomodoro",
            "title": spec.title,
            "message": spec.message,
            "buttons": spec.buttons,
            "system_notification": spec.options.system_notifications,
            "sound": spec.options.sound_enabled,
        });
        println!("{}", serde_json::to_string(&value)?);
        return Ok(());
    }

    show_platform_notification(spec, action_tx)
}

fn paused_spec(
    session_id: &str,
    reason: PauseReasonView,
    options: PomodoroNotificationOptions,
    locale: NotificationLocale,
) -> PomodoroNotificationSpec {
    let message = match (locale, reason) {
        (NotificationLocale::Russian, PauseReasonView::Afk) => {
            "Таймер остановлен, пока вы отсутствуете. Вернитесь и нажмите «Продолжить»."
        }
        (NotificationLocale::Russian, PauseReasonView::MonitoringUnavailable) => {
            "Нет связи с ActivityWatch. После восстановления нажмите «Продолжить»."
        }
        (NotificationLocale::English, PauseReasonView::Afk) => {
            "The timer is paused while you are away. Return and select Continue."
        }
        (NotificationLocale::English, PauseReasonView::MonitoringUnavailable) => {
            "ActivityWatch monitoring is unavailable. Select Continue after it recovers."
        }
        (_, PauseReasonView::Manual) => "The Pomodoro timer is paused.",
    };
    PomodoroNotificationSpec {
        title: translated(
            locale,
            "ActivityWatch · Pomodoro paused",
            "ActivityWatch · Томато на паузе",
        ),
        message: message.to_string(),
        buttons: vec![
            button(locale, PomodoroNotificationAction::Continue),
            button(locale, PomodoroNotificationAction::Stop),
        ],
        token: Some(PomodoroActionToken::Paused {
            session_id: session_id.to_string(),
            reason,
        }),
        options,
    }
}

fn next_phase_message(next: &PhaseView, locale: NotificationLocale) -> String {
    match (locale, next.kind) {
        (NotificationLocale::Russian, PhaseKind::Focus) => format!(
            "Готовы начать рабочий этап {}?",
            next.focus_number.unwrap_or(1)
        ),
        (NotificationLocale::Russian, PhaseKind::ShortBreak) => {
            "Готовы начать короткий перерыв?".to_string()
        }
        (NotificationLocale::Russian, PhaseKind::LongBreak) => {
            "Готовы начать длинный перерыв?".to_string()
        }
        (NotificationLocale::English, PhaseKind::Focus) => format!(
            "Ready to start work phase {}?",
            next.focus_number.unwrap_or(1)
        ),
        (NotificationLocale::English, PhaseKind::ShortBreak) => {
            "Ready to start a short break?".to_string()
        }
        (NotificationLocale::English, PhaseKind::LongBreak) => {
            "Ready to start a long break?".to_string()
        }
    }
}

fn format_duration(milliseconds: u64, locale: NotificationLocale) -> String {
    let total_seconds = milliseconds.div_ceil(1_000);
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    match locale {
        NotificationLocale::Russian => format!("{minutes}:{seconds:02}"),
        NotificationLocale::English => format!("{minutes}:{seconds:02}"),
    }
}

fn translated(locale: NotificationLocale, english: &str, russian: &str) -> String {
    match locale {
        NotificationLocale::English => english,
        NotificationLocale::Russian => russian,
    }
    .to_string()
}

fn button(locale: NotificationLocale, action: PomodoroNotificationAction) -> NotificationButton {
    let label = match (locale, action) {
        (NotificationLocale::Russian, PomodoroNotificationAction::Continue) => "Продолжить",
        (NotificationLocale::Russian, PomodoroNotificationAction::Pause) => "Пауза",
        (NotificationLocale::Russian, PomodoroNotificationAction::Stop) => "Стоп",
        (NotificationLocale::English, PomodoroNotificationAction::Continue) => "Continue",
        (NotificationLocale::English, PomodoroNotificationAction::Pause) => "Pause",
        (NotificationLocale::English, PomodoroNotificationAction::Stop) => "Stop",
    };
    NotificationButton {
        label: label.to_string(),
        action,
    }
}

fn start_button(locale: NotificationLocale) -> NotificationButton {
    NotificationButton {
        label: translated(locale, "Start", "Начать"),
        action: PomodoroNotificationAction::Continue,
    }
}

#[cfg(windows)]
fn action_argument(action: PomodoroNotificationAction) -> &'static str {
    match action {
        PomodoroNotificationAction::Continue => "continue",
        PomodoroNotificationAction::Pause => "pause",
        PomodoroNotificationAction::Stop => "stop",
    }
}

#[cfg(any(windows, test))]
fn parse_action(value: &str) -> Option<PomodoroNotificationAction> {
    match value {
        "continue" => Some(PomodoroNotificationAction::Continue),
        "pause" => Some(PomodoroNotificationAction::Pause),
        "stop" => Some(PomodoroNotificationAction::Stop),
        _ => None,
    }
}

#[cfg(windows)]
fn show_platform_notification(
    spec: PomodoroNotificationSpec,
    action_tx: &Sender<PomodoroActionRequest>,
) -> Result<()> {
    use tauri_winrt_notification::{Duration, Sound, Toast};

    if !spec.options.system_notifications {
        if spec.options.sound_enabled {
            play_sound_only();
        }
        return Ok(());
    }

    let mut toast = Toast::new(Toast::POWERSHELL_APP_ID)
        .title(&spec.title)
        .text1(&spec.message)
        .duration(Duration::Long)
        .sound(spec.options.sound_enabled.then_some(Sound::Reminder));
    for button in &spec.buttons {
        toast = toast.add_button(&button.label, action_argument(button.action));
    }
    if let Some(token) = spec.token {
        let action_tx = action_tx.clone();
        toast = toast.on_activated(move |argument| {
            if let Some(action) = argument.as_deref().and_then(parse_action) {
                let _ = action_tx.send(PomodoroActionRequest {
                    token: token.clone(),
                    action,
                });
            }
            Ok(())
        });
    }
    toast.show()?;
    Ok(())
}

#[cfg(windows)]
fn play_sound_only() {
    use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
    use windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION;

    // MessageBeep is used only when visual system notifications are disabled.
    // A visible toast plays its own sound, so the two paths never duplicate it.
    unsafe {
        MessageBeep(MB_ICONINFORMATION);
    }
}

#[cfg(not(windows))]
fn show_platform_notification(
    spec: PomodoroNotificationSpec,
    _action_tx: &Sender<PomodoroActionRequest>,
) -> Result<()> {
    if spec.options.system_notifications {
        notify_rust::Notification::new()
            .summary(&spec.title)
            .body(&spec.message)
            .appname("ActivityWatch")
            .timeout(25_000)
            .show()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(system_notifications: bool, sound_enabled: bool) -> PomodoroNotificationOptions {
        PomodoroNotificationOptions {
            system_notifications,
            chrome_notifications: true,
            sound_enabled,
        }
    }

    #[test]
    fn distraction_warning_has_all_three_actions_and_exact_token() {
        let spec = notification_spec(
            &PomodoroEvent::DistractionWarning {
                session_id: "session-1".into(),
                sequence: 4,
                current_category: Some(vec!["Media".into(), "Video".into()]),
                remaining_milliseconds: 64_100,
                notifications: options(true, true),
            },
            NotificationLocale::Russian,
        );

        assert!(spec.message.contains("Media > Video"));
        assert!(spec.message.contains("1:05"));
        assert_eq!(
            spec.buttons
                .iter()
                .map(|button| button.action)
                .collect::<Vec<_>>(),
            vec![
                PomodoroNotificationAction::Continue,
                PomodoroNotificationAction::Pause,
                PomodoroNotificationAction::Stop,
            ]
        );
        assert_eq!(
            spec.token,
            Some(PomodoroActionToken::Distraction {
                session_id: "session-1".into(),
                sequence: 4,
            })
        );
    }

    #[test]
    fn phase_completion_offers_start_and_stop_while_completion_has_no_actions() {
        let completed = PhaseView {
            kind: PhaseKind::Focus,
            focus_number: Some(1),
            after_focus: None,
        };
        let next = PhaseView {
            kind: PhaseKind::ShortBreak,
            focus_number: None,
            after_focus: Some(1),
        };
        let phase = notification_spec(
            &PomodoroEvent::PhaseCompleted {
                session_id: "session-1".into(),
                completed,
                next,
                notifications: options(true, false),
            },
            NotificationLocale::English,
        );
        assert_eq!(phase.buttons.len(), 2);
        assert_eq!(phase.buttons[0].label, "Start");
        assert!(!phase.options.sound_enabled);

        let complete = notification_spec(
            &PomodoroEvent::SessionCompleted {
                session_id: "session-1".into(),
                focus_intervals: 8,
                notifications: options(true, true),
            },
            NotificationLocale::Russian,
        );
        assert!(complete.buttons.is_empty());
        assert!(complete.token.is_none());
        assert!(complete.message.contains('8'));
    }

    #[test]
    fn locale_falls_back_to_english() {
        assert_eq!(
            NotificationLocale::from_code(Some("ru")),
            NotificationLocale::Russian
        );
        assert_eq!(
            NotificationLocale::from_code(Some("de")),
            NotificationLocale::English
        );
        assert_eq!(
            NotificationLocale::from_code(None),
            NotificationLocale::English
        );
    }

    #[test]
    fn only_known_button_arguments_are_accepted() {
        assert_eq!(
            parse_action("pause"),
            Some(PomodoroNotificationAction::Pause)
        );
        assert_eq!(parse_action("old-notification"), None);
    }
}
