//! Deterministic Pomodoro timer state machine.
//!
//! The core does not read the wall clock, perform I/O, or start background
//! threads. Callers provide a monotonic [`Instant`] to every time-sensitive
//! operation. This keeps phase transitions deterministic and prevents timer
//! drift when a UI or service polls at irregular intervals.

#![forbid(unsafe_code)]

use std::fmt;
use std::time::{Duration, Instant};

/// Default length of a focus interval: 25 minutes.
pub const DEFAULT_FOCUS_DURATION_SECS: u64 = 25 * 60;
/// Default length of a short break: 5 minutes.
pub const DEFAULT_SHORT_BREAK_DURATION_SECS: u64 = 5 * 60;
/// Default length of a long break: 15 minutes.
pub const DEFAULT_LONG_BREAK_DURATION_SECS: u64 = 15 * 60;
/// Default number of focus intervals in a session.
pub const DEFAULT_WORK_INTERVALS: u32 = 8;
/// Default delay before a distraction warning: 30 seconds.
pub const DEFAULT_DISTRACTION_TIMEOUT_SECS: u64 = 30;

/// Settings captured for one Pomodoro session.
///
/// The distraction timeout belongs here so one settings snapshot contains all
/// per-session defaults, although distraction detection is handled outside the
/// timer state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PomodoroSettings {
    pub focus_duration_secs: u64,
    pub short_break_duration_secs: u64,
    pub long_break_duration_secs: u64,
    pub work_intervals: u32,
    pub distraction_timeout_secs: u64,
}

impl PomodoroSettings {
    /// Validate settings before a timer is created.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.focus_duration_secs == 0 {
            return Err(SettingsError::MustBePositive("focus_duration_secs"));
        }
        if self.short_break_duration_secs == 0 {
            return Err(SettingsError::MustBePositive("short_break_duration_secs"));
        }
        if self.long_break_duration_secs == 0 {
            return Err(SettingsError::MustBePositive("long_break_duration_secs"));
        }
        if self.work_intervals == 0 {
            return Err(SettingsError::MustBePositive("work_intervals"));
        }
        if self.distraction_timeout_secs == 0 {
            return Err(SettingsError::MustBePositive("distraction_timeout_secs"));
        }
        Ok(())
    }

    /// Duration assigned to a phase by these settings.
    pub fn duration_for(&self, phase: Phase) -> Duration {
        let seconds = match phase {
            Phase::Focus { .. } => self.focus_duration_secs,
            Phase::ShortBreak { .. } => self.short_break_duration_secs,
            Phase::LongBreak { .. } => self.long_break_duration_secs,
        };
        Duration::from_secs(seconds)
    }
}

impl Default for PomodoroSettings {
    fn default() -> Self {
        Self {
            focus_duration_secs: DEFAULT_FOCUS_DURATION_SECS,
            short_break_duration_secs: DEFAULT_SHORT_BREAK_DURATION_SECS,
            long_break_duration_secs: DEFAULT_LONG_BREAK_DURATION_SECS,
            work_intervals: DEFAULT_WORK_INTERVALS,
            distraction_timeout_secs: DEFAULT_DISTRACTION_TIMEOUT_SECS,
        }
    }
}

/// A phase in the configured session sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Focus { number: u32 },
    ShortBreak { after_focus: u32 },
    LongBreak { after_focus: u32 },
}

/// Why a running phase is paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseReason {
    Manual,
    Afk,
    MonitoringUnavailable,
}

/// Why a session ended before all focus intervals were completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptionReason {
    UserStopped,
    ActivityWatchRestart,
}

/// Observable state of a Pomodoro timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerState {
    Ready,
    Running { phase: Phase },
    Paused { phase: Phase, reason: PauseReason },
    WaitingForConfirmation { completed: Phase, next: Phase },
    Completed,
    Interrupted { reason: InterruptionReason },
}

impl TimerState {
    fn name(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Running { .. } => "running",
            Self::Paused { .. } => "paused",
            Self::WaitingForConfirmation { .. } => "waiting_for_confirmation",
            Self::Completed => "completed",
            Self::Interrupted { .. } => "interrupted",
        }
    }
}

/// Domain event produced by a successful state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerEvent {
    SessionStarted {
        phase: Phase,
    },
    PhaseCompleted {
        completed: Phase,
        next: Phase,
    },
    PhaseStarted {
        phase: Phase,
    },
    Paused {
        phase: Phase,
        reason: PauseReason,
        remaining: Duration,
    },
    Resumed {
        phase: Phase,
        remaining: Duration,
    },
    SessionCompleted {
        focus_intervals: u32,
    },
    SessionInterrupted {
        reason: InterruptionReason,
        completed_focus_intervals: u32,
    },
}

/// Invalid Pomodoro settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsError {
    MustBePositive(&'static str),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MustBePositive(field) => write!(f, "{field} must be greater than zero"),
        }
    }
}

impl std::error::Error for SettingsError {}

/// A rejected timer command or an impossible internal time value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerError {
    InvalidTransition {
        action: &'static str,
        state: &'static str,
    },
    PhaseAlreadyElapsed,
    TimeOverflow,
    CorruptState(&'static str),
}

impl fmt::Display for TimerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTransition { action, state } => {
                write!(f, "cannot {action} while timer is {state}")
            }
            Self::PhaseAlreadyElapsed => {
                write!(
                    f,
                    "the running phase has elapsed and must be advanced first"
                )
            }
            Self::TimeOverflow => write!(f, "timer deadline exceeds the supported time range"),
            Self::CorruptState(message) => write!(f, "invalid internal timer state: {message}"),
        }
    }
}

impl std::error::Error for TimerError {}

/// Deterministic state machine for one Pomodoro session.
#[derive(Debug)]
pub struct PomodoroTimer {
    settings: PomodoroSettings,
    state: TimerState,
    deadline: Option<Instant>,
    paused_remaining: Option<Duration>,
    completed_focus_intervals: u32,
}

impl PomodoroTimer {
    /// Create a ready timer after validating its immutable session settings.
    pub fn new(settings: PomodoroSettings) -> Result<Self, SettingsError> {
        settings.validate()?;
        Ok(Self {
            settings,
            state: TimerState::Ready,
            deadline: None,
            paused_remaining: None,
            completed_focus_intervals: 0,
        })
    }

    pub fn settings(&self) -> PomodoroSettings {
        self.settings
    }

    pub fn state(&self) -> TimerState {
        self.state
    }

    pub fn completed_focus_intervals(&self) -> u32 {
        self.completed_focus_intervals
    }

    pub fn current_phase(&self) -> Option<Phase> {
        match self.state {
            TimerState::Running { phase } | TimerState::Paused { phase, .. } => Some(phase),
            _ => None,
        }
    }

    pub fn next_phase(&self) -> Option<Phase> {
        match self.state {
            TimerState::WaitingForConfirmation { next, .. } => Some(next),
            _ => None,
        }
    }

    /// Start the first focus interval.
    pub fn start(&mut self, now: Instant) -> Result<TimerEvent, TimerError> {
        if self.state != TimerState::Ready {
            return Err(self.invalid_transition("start"));
        }

        let phase = Phase::Focus { number: 1 };
        self.start_phase(phase, now)?;
        Ok(TimerEvent::SessionStarted { phase })
    }

    /// Advance a running phase when its deadline has been reached.
    ///
    /// At most one transition occurs per call. The next phase never starts
    /// automatically: callers must invoke [`Self::confirm_next`].
    pub fn tick(&mut self, now: Instant) -> Result<Option<TimerEvent>, TimerError> {
        let phase = match self.state {
            TimerState::Running { phase } => phase,
            _ => return Ok(None),
        };
        let deadline = self
            .deadline
            .ok_or(TimerError::CorruptState("running timer has no deadline"))?;
        if now < deadline {
            return Ok(None);
        }

        self.deadline = None;
        if matches!(phase, Phase::Focus { .. }) {
            self.completed_focus_intervals += 1;
        }

        match self.phase_after(phase) {
            Some(next) => {
                self.state = TimerState::WaitingForConfirmation {
                    completed: phase,
                    next,
                };
                Ok(Some(TimerEvent::PhaseCompleted {
                    completed: phase,
                    next,
                }))
            }
            None => {
                self.state = TimerState::Completed;
                Ok(Some(TimerEvent::SessionCompleted {
                    focus_intervals: self.completed_focus_intervals,
                }))
            }
        }
    }

    /// Start the pending phase after explicit user confirmation.
    pub fn confirm_next(&mut self, now: Instant) -> Result<TimerEvent, TimerError> {
        let next = match self.state {
            TimerState::WaitingForConfirmation { next, .. } => next,
            _ => return Err(self.invalid_transition("confirm the next phase")),
        };
        self.start_phase(next, now)?;
        Ok(TimerEvent::PhaseStarted { phase: next })
    }

    /// Pause the running phase and freeze its exact remaining duration.
    pub fn pause(&mut self, now: Instant, reason: PauseReason) -> Result<TimerEvent, TimerError> {
        let phase = match self.state {
            TimerState::Running { phase } => phase,
            _ => return Err(self.invalid_transition("pause")),
        };
        let deadline = self
            .deadline
            .ok_or(TimerError::CorruptState("running timer has no deadline"))?;
        if now >= deadline {
            return Err(TimerError::PhaseAlreadyElapsed);
        }
        let remaining = deadline.duration_since(now);

        self.state = TimerState::Paused { phase, reason };
        self.deadline = None;
        self.paused_remaining = Some(remaining);
        Ok(TimerEvent::Paused {
            phase,
            reason,
            remaining,
        })
    }

    /// Resume a paused phase from its frozen remaining duration.
    pub fn resume(&mut self, now: Instant) -> Result<TimerEvent, TimerError> {
        let phase = match self.state {
            TimerState::Paused { phase, .. } => phase,
            _ => return Err(self.invalid_transition("resume")),
        };
        let remaining = self.paused_remaining.ok_or(TimerError::CorruptState(
            "paused timer has no remaining time",
        ))?;
        let deadline = now.checked_add(remaining).ok_or(TimerError::TimeOverflow)?;

        self.state = TimerState::Running { phase };
        self.deadline = Some(deadline);
        self.paused_remaining = None;
        Ok(TimerEvent::Resumed { phase, remaining })
    }

    /// End an active session without completing it.
    pub fn stop(&mut self, reason: InterruptionReason) -> Result<TimerEvent, TimerError> {
        if !matches!(
            self.state,
            TimerState::Running { .. }
                | TimerState::Paused { .. }
                | TimerState::WaitingForConfirmation { .. }
        ) {
            return Err(self.invalid_transition("stop"));
        }

        self.state = TimerState::Interrupted { reason };
        self.deadline = None;
        self.paused_remaining = None;
        Ok(TimerEvent::SessionInterrupted {
            reason,
            completed_focus_intervals: self.completed_focus_intervals,
        })
    }

    /// Remaining phase time, frozen while paused.
    pub fn remaining(&self, now: Instant) -> Result<Option<Duration>, TimerError> {
        match self.state {
            TimerState::Running { .. } => {
                let deadline = self
                    .deadline
                    .ok_or(TimerError::CorruptState("running timer has no deadline"))?;
                Ok(Some(deadline.saturating_duration_since(now)))
            }
            TimerState::Paused { .. } => {
                self.paused_remaining
                    .map(Some)
                    .ok_or(TimerError::CorruptState(
                        "paused timer has no remaining time",
                    ))
            }
            _ => Ok(None),
        }
    }

    fn start_phase(&mut self, phase: Phase, now: Instant) -> Result<(), TimerError> {
        let deadline = now
            .checked_add(self.settings.duration_for(phase))
            .ok_or(TimerError::TimeOverflow)?;
        self.state = TimerState::Running { phase };
        self.deadline = Some(deadline);
        self.paused_remaining = None;
        Ok(())
    }

    fn phase_after(&self, phase: Phase) -> Option<Phase> {
        match phase {
            Phase::Focus { number } if number == self.settings.work_intervals => None,
            Phase::Focus { number } if number % 2 == 1 => Some(Phase::ShortBreak {
                after_focus: number,
            }),
            Phase::Focus { number } => Some(Phase::LongBreak {
                after_focus: number,
            }),
            Phase::ShortBreak { after_focus } | Phase::LongBreak { after_focus } => {
                Some(Phase::Focus {
                    number: after_focus + 1,
                })
            }
        }
    }

    fn invalid_transition(&self, action: &'static str) -> TimerError {
        TimerError::InvalidTransition {
            action,
            state: self.state.name(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short_settings() -> PomodoroSettings {
        PomodoroSettings {
            focus_duration_secs: 25,
            short_break_duration_secs: 5,
            long_break_duration_secs: 15,
            work_intervals: 8,
            distraction_timeout_secs: 30,
        }
    }

    fn running_timer(now: Instant) -> PomodoroTimer {
        let mut timer = PomodoroTimer::new(short_settings()).unwrap();
        assert_eq!(
            timer.start(now).unwrap(),
            TimerEvent::SessionStarted {
                phase: Phase::Focus { number: 1 }
            }
        );
        timer
    }

    #[test]
    fn defaults_match_the_agreed_session() {
        assert_eq!(
            PomodoroSettings::default(),
            PomodoroSettings {
                focus_duration_secs: 1_500,
                short_break_duration_secs: 300,
                long_break_duration_secs: 900,
                work_intervals: 8,
                distraction_timeout_secs: 30,
            }
        );
    }

    #[test]
    fn every_numeric_setting_must_be_positive() {
        let mut settings = short_settings();
        settings.focus_duration_secs = 0;
        assert_eq!(
            settings.validate(),
            Err(SettingsError::MustBePositive("focus_duration_secs"))
        );

        let mut settings = short_settings();
        settings.short_break_duration_secs = 0;
        assert_eq!(
            settings.validate(),
            Err(SettingsError::MustBePositive("short_break_duration_secs"))
        );

        let mut settings = short_settings();
        settings.long_break_duration_secs = 0;
        assert_eq!(
            settings.validate(),
            Err(SettingsError::MustBePositive("long_break_duration_secs"))
        );

        let mut settings = short_settings();
        settings.work_intervals = 0;
        assert_eq!(
            settings.validate(),
            Err(SettingsError::MustBePositive("work_intervals"))
        );

        let mut settings = short_settings();
        settings.distraction_timeout_secs = 0;
        assert_eq!(
            settings.validate(),
            Err(SettingsError::MustBePositive("distraction_timeout_secs"))
        );
    }

    #[test]
    fn phase_ends_exactly_at_its_deadline() {
        let now = Instant::now();
        let mut timer = running_timer(now);

        assert_eq!(timer.tick(now + Duration::from_secs(24)).unwrap(), None);
        assert_eq!(
            timer.tick(now + Duration::from_secs(25)).unwrap(),
            Some(TimerEvent::PhaseCompleted {
                completed: Phase::Focus { number: 1 },
                next: Phase::ShortBreak { after_focus: 1 },
            })
        );
        assert_eq!(timer.completed_focus_intervals(), 1);
    }

    #[test]
    fn next_phase_waits_for_manual_confirmation() {
        let now = Instant::now();
        let mut timer = running_timer(now);
        timer.tick(now + Duration::from_secs(25)).unwrap();

        assert_eq!(timer.tick(now + Duration::from_secs(10_000)).unwrap(), None);
        assert_eq!(
            timer.state(),
            TimerState::WaitingForConfirmation {
                completed: Phase::Focus { number: 1 },
                next: Phase::ShortBreak { after_focus: 1 },
            }
        );

        let confirmed_at = now + Duration::from_secs(10_001);
        assert_eq!(
            timer.confirm_next(confirmed_at).unwrap(),
            TimerEvent::PhaseStarted {
                phase: Phase::ShortBreak { after_focus: 1 }
            }
        );
        assert_eq!(
            timer.remaining(confirmed_at).unwrap(),
            Some(Duration::from_secs(5))
        );
    }

    #[test]
    fn breaks_alternate_short_then_long() {
        let mut settings = short_settings();
        settings.work_intervals = 3;
        let mut timer = PomodoroTimer::new(settings).unwrap();
        let mut now = Instant::now();
        timer.start(now).unwrap();

        now += Duration::from_secs(25);
        timer.tick(now).unwrap();
        assert_eq!(
            timer.next_phase(),
            Some(Phase::ShortBreak { after_focus: 1 })
        );
        timer.confirm_next(now).unwrap();

        now += Duration::from_secs(5);
        timer.tick(now).unwrap();
        timer.confirm_next(now).unwrap();

        now += Duration::from_secs(25);
        timer.tick(now).unwrap();
        assert_eq!(
            timer.next_phase(),
            Some(Phase::LongBreak { after_focus: 2 })
        );
    }

    #[test]
    fn eight_focus_intervals_finish_without_a_trailing_break() {
        let mut timer = PomodoroTimer::new(short_settings()).unwrap();
        let mut now = Instant::now();
        timer.start(now).unwrap();

        for focus_number in 1..=8 {
            assert_eq!(
                timer.current_phase(),
                Some(Phase::Focus {
                    number: focus_number
                })
            );
            now += Duration::from_secs(25);
            let event = timer.tick(now).unwrap().unwrap();

            if focus_number == 8 {
                assert_eq!(event, TimerEvent::SessionCompleted { focus_intervals: 8 });
                break;
            }

            let expected_break = if focus_number % 2 == 1 {
                Phase::ShortBreak {
                    after_focus: focus_number,
                }
            } else {
                Phase::LongBreak {
                    after_focus: focus_number,
                }
            };
            assert_eq!(timer.next_phase(), Some(expected_break));
            timer.confirm_next(now).unwrap();

            let break_seconds = if focus_number % 2 == 1 { 5 } else { 15 };
            now += Duration::from_secs(break_seconds);
            timer.tick(now).unwrap();
            assert_eq!(
                timer.next_phase(),
                Some(Phase::Focus {
                    number: focus_number + 1
                })
            );
            timer.confirm_next(now).unwrap();
        }

        assert_eq!(timer.state(), TimerState::Completed);
        assert_eq!(timer.completed_focus_intervals(), 8);
        assert_eq!(timer.next_phase(), None);
    }

    #[test]
    fn pause_freezes_remaining_time_and_resume_uses_a_new_deadline() {
        let now = Instant::now();
        let mut timer = running_timer(now);
        let paused_at = now + Duration::from_secs(10);

        assert_eq!(
            timer.pause(paused_at, PauseReason::Manual).unwrap(),
            TimerEvent::Paused {
                phase: Phase::Focus { number: 1 },
                reason: PauseReason::Manual,
                remaining: Duration::from_secs(15),
            }
        );
        assert_eq!(
            timer.remaining(now + Duration::from_secs(1_000)).unwrap(),
            Some(Duration::from_secs(15))
        );
        assert_eq!(timer.tick(now + Duration::from_secs(1_000)).unwrap(), None);

        let resumed_at = now + Duration::from_secs(1_000);
        assert_eq!(
            timer.resume(resumed_at).unwrap(),
            TimerEvent::Resumed {
                phase: Phase::Focus { number: 1 },
                remaining: Duration::from_secs(15),
            }
        );
        assert_eq!(
            timer.tick(resumed_at + Duration::from_secs(14)).unwrap(),
            None
        );
        assert!(matches!(
            timer.tick(resumed_at + Duration::from_secs(15)).unwrap(),
            Some(TimerEvent::PhaseCompleted { .. })
        ));
    }

    #[test]
    fn remaining_time_is_derived_from_one_deadline_without_polling_drift() {
        let now = Instant::now();
        let timer = running_timer(now);

        assert_eq!(
            timer.remaining(now + Duration::from_secs(3)).unwrap(),
            Some(Duration::from_secs(22))
        );
        assert_eq!(
            timer.remaining(now + Duration::from_secs(17)).unwrap(),
            Some(Duration::from_secs(8))
        );
        assert_eq!(
            timer.remaining(now + Duration::from_secs(26)).unwrap(),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn elapsed_phase_must_be_advanced_before_it_can_be_paused() {
        let now = Instant::now();
        let mut timer = running_timer(now);
        assert_eq!(
            timer.pause(now + Duration::from_secs(25), PauseReason::Manual),
            Err(TimerError::PhaseAlreadyElapsed)
        );
        assert!(matches!(timer.state(), TimerState::Running { .. }));
    }

    #[test]
    fn afk_pause_uses_the_same_frozen_timer_semantics() {
        let now = Instant::now();
        let mut timer = running_timer(now);
        timer
            .pause(now + Duration::from_secs(4), PauseReason::Afk)
            .unwrap();

        assert_eq!(
            timer.state(),
            TimerState::Paused {
                phase: Phase::Focus { number: 1 },
                reason: PauseReason::Afk,
            }
        );
        assert_eq!(
            timer.remaining(now + Duration::from_secs(500)).unwrap(),
            Some(Duration::from_secs(21))
        );
    }

    #[test]
    fn stop_interrupts_running_paused_and_waiting_sessions() {
        let now = Instant::now();

        let mut running = running_timer(now);
        assert_eq!(
            running.stop(InterruptionReason::UserStopped).unwrap(),
            TimerEvent::SessionInterrupted {
                reason: InterruptionReason::UserStopped,
                completed_focus_intervals: 0,
            }
        );

        let mut paused = running_timer(now);
        paused
            .pause(now + Duration::from_secs(1), PauseReason::Manual)
            .unwrap();
        paused.stop(InterruptionReason::UserStopped).unwrap();
        assert_eq!(
            paused.state(),
            TimerState::Interrupted {
                reason: InterruptionReason::UserStopped
            }
        );

        let mut waiting = running_timer(now);
        waiting.tick(now + Duration::from_secs(25)).unwrap();
        waiting
            .stop(InterruptionReason::ActivityWatchRestart)
            .unwrap();
        assert_eq!(
            waiting.state(),
            TimerState::Interrupted {
                reason: InterruptionReason::ActivityWatchRestart
            }
        );
    }

    #[test]
    fn invalid_commands_do_not_change_state() {
        let now = Instant::now();
        let mut timer = PomodoroTimer::new(short_settings()).unwrap();

        assert!(matches!(
            timer.resume(now),
            Err(TimerError::InvalidTransition {
                action: "resume",
                state: "ready"
            })
        ));
        assert_eq!(timer.state(), TimerState::Ready);

        timer.start(now).unwrap();
        assert!(matches!(
            timer.confirm_next(now),
            Err(TimerError::InvalidTransition { .. })
        ));
        assert!(matches!(timer.state(), TimerState::Running { .. }));
    }
}
