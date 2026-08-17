use std::error::Error;
use std::time::Instant;

use aw_pomodoro_core::{PomodoroSettings, PomodoroTimer, TimerState};

fn main() -> Result<(), Box<dyn Error>> {
    let settings = PomodoroSettings {
        focus_duration_secs: 4,
        short_break_duration_secs: 1,
        long_break_duration_secs: 2,
        work_intervals: 4,
        distraction_timeout_secs: 1,
    };
    let mut timer = PomodoroTimer::new(settings)?;
    let mut now = Instant::now();

    println!("{:?}", timer.start(now)?);
    while timer.state() != TimerState::Completed {
        let phase = timer
            .current_phase()
            .expect("the simulation only advances running phases");
        now += settings.duration_for(phase);
        println!("{:?}", timer.tick(now)?.expect("phase must finish"));

        if timer.state() != TimerState::Completed {
            println!("Waiting for user confirmation: {:?}", timer.next_phase());
            println!("{:?}", timer.confirm_next(now)?);
        }
    }

    Ok(())
}
