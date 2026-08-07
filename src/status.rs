use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use colored::Colorize;

use crate::common::{CTP_BLUE, CTP_OVERLAY0, clear_line, eprint_flush, hide_cursor, show_cursor};

const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const TICK_MS: u64 = 80;

/// Animated status line: braille spinner + label + elapsed timer.
///
/// Spawns a background thread that overwrites the current terminal line every
/// [`TICK_MS`]. The spinner runs in [`CTP_BLUE`] (sapphire); label and timer in
/// [`CTP_OVERLAY0`] (subtle).
///
/// `finish()` stops the animation, clears the line, and restores the cursor.
/// `Drop` does the same if `finish()` was not called explicitly.
pub struct StatusLine {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl StatusLine {
    pub fn start(label: &str) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let r = running.clone();
        let label = label.to_string();

        hide_cursor();

        let handle = thread::spawn(move || {
            let start = Instant::now();
            let mut frame = 0usize;

            while r.load(Ordering::Relaxed) {
                let elapsed = start.elapsed();
                let spinner = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()];
                frame = frame.wrapping_add(1);

                let line = format!(
                    "{} {} · {}",
                    spinner.to_string().custom_color(CTP_BLUE),
                    label.custom_color(CTP_OVERLAY0),
                    format_elapsed(elapsed).custom_color(CTP_OVERLAY0),
                );
                eprint_flush(&format!("\r\x1b[K{line}"));

                thread::sleep(Duration::from_millis(TICK_MS));
            }
        });

        Self { running, handle: Some(handle) }
    }

    /// Stops the animation, joins the background thread, clears the status line,
    /// and restores the terminal cursor.
    pub fn finish(mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        clear_line();
        show_cursor();
    }
}

impl Drop for StatusLine {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        // Don't join — the thread will exit on its own after the next tick.
        clear_line();
        show_cursor();
    }
}

fn format_elapsed(duration: Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs < 10.0 {
        format!("{secs:.1}s")
    } else if secs < 60.0 {
        format!("{}s", secs as u64)
    } else {
        let mins = secs as u64 / 60;
        let remain = secs as u64 % 60;
        format!("{mins}m{remain}s")
    }
}
