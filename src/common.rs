use std::borrow::Cow;
use std::env;
use std::fs;
use std::io;
use std::process::Command;
use std::sync::LazyLock;

use nix::libc;
use strip_ansi_escapes::strip;
use unicode_width::UnicodeWidthStr;

pub const EXIT_SIGINT: i32 = 130;
pub const DEFAULT_PROVIDER_TIMEOUT_SECS: u64 = 30;

// Catppuccin Mocha palette — terminal truecolor tokens
pub const CTP_PRIMARY: colored::CustomColor = colored::CustomColor {
    r: 0xE0,
    g: 0x7B,
    b: 0x53,
}; // Claude Orange
pub const CTP_BLUE: colored::CustomColor = colored::CustomColor {
    r: 0x89,
    g: 0xb4,
    b: 0xfa,
};
pub const CTP_GREEN: colored::CustomColor = colored::CustomColor {
    r: 0xa6,
    g: 0xe3,
    b: 0xa1,
};
pub const CTP_RED: colored::CustomColor = colored::CustomColor {
    r: 0xf3,
    g: 0x8b,
    b: 0xa8,
};
pub const CTP_YELLOW: colored::CustomColor = colored::CustomColor {
    r: 0xf9,
    g: 0xe2,
    b: 0xaf,
};
pub const CTP_TEXT: colored::CustomColor = colored::CustomColor {
    r: 0xcd,
    g: 0xd6,
    b: 0xf4,
};
pub const CTP_OVERLAY0: colored::CustomColor = colored::CustomColor {
    r: 0x6c,
    g: 0x70,
    b: 0x86,
};

pub const ANSI_SHOW_CURSOR: &str = "\x1b[?25h";
pub const ANSI_HIDE_CURSOR: &str = "\x1b[?25l";
pub const ANSI_CLEAR_LINE: &str = "\r\x1b[K";
pub const ANSI_CURSOR_UP_CLEAR: &str = "\x1b[1A\r\x1b[K";

pub fn current_directory() -> String {
    env::current_dir().map_or_else(|_| "/".to_string(), |p| p.display().to_string())
}

pub fn current_directory_display() -> String {
    let cwd = current_directory();
    if let Some(home) = dirs::home_dir() {
        let home_str = home.display().to_string();
        if let Some(rel) = cwd.strip_prefix(&home_str) {
            return format!("~{rel}");
        }
    }
    cwd
}

static LINUX_INFO: LazyLock<String> = LazyLock::new(|| {
    let distro = linux_distro();
    let kernel = kernel_version();
    format!("linux ({distro}; kernel: {kernel})")
});

static SHELL_NAME: LazyLock<String> = LazyLock::new(|| {
    env::var("SHELL")
        .ok()
        .and_then(|s| s.split('/').next_back().map(str::to_string))
        .unwrap_or_else(|| "sh".to_string())
});

static USERNAME: LazyLock<String> = LazyLock::new(|| {
    env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string())
});

pub fn os_name() -> Cow<'static, str> {
    if cfg!(target_os = "linux") {
        Cow::Borrowed(LINUX_INFO.as_str())
    } else if cfg!(target_os = "macos") {
        "macOS".into()
    } else if cfg!(windows) {
        "Windows".into()
    } else {
        "Unix".into()
    }
}

pub fn shell_name() -> String {
    SHELL_NAME.as_str().to_string()
}

pub fn username() -> String {
    USERNAME.as_str().to_string()
}

/// reads /etc/os-release to get the distro name and version.
fn linux_distro() -> String {
    fs::read_to_string("/etc/os-release").map_or_else(
        |_| "linux".to_string(),
        |contents| {
            let mut name = None;
            let mut version = None;

            for line in contents.lines() {
                if let Some(value) = line.strip_prefix("NAME=") {
                    name = Some(value.trim_matches('"').to_string());
                } else if let Some(value) = line.strip_prefix("VERSION_ID=") {
                    version = Some(value.trim_matches('"').to_string());
                }
            }

            match (name, version) {
                (Some(n), Some(v)) => format!("{n} {v}"),
                (Some(n), None) => n,
                _ => "linux".to_string(),
            }
        },
    )
}

/// gets the kernel version from `uname -r` or `/proc/sys/kernel/osrelease`.
fn kernel_version() -> String {
    Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.trim().to_string())
        .or_else(|| fs::read_to_string("/proc/sys/kernel/osrelease").ok())
        .map_or_else(|| "unknown".to_string(), |s| s.trim().to_string())
}

pub fn show_cursor() {
    eprint!("{ANSI_SHOW_CURSOR}");
    flush_stderr();
}

pub fn hide_cursor() {
    eprint!("{ANSI_HIDE_CURSOR}");
    flush_stderr();
}

pub fn clear_line() {
    eprint!("{ANSI_CLEAR_LINE}");
    flush_stderr();
}

/// clears exactly `n` visual lines from the terminal, starting at the current
/// cursor line and moving upward. the cursor is assumed to be on the last of
/// these `n` lines (e.g. after an `eprint!` without newline).
pub fn clear_n_lines(n: usize) {
    if n == 0 {
        return;
    }
    eprint!("{ANSI_CLEAR_LINE}");
    for _ in 0..n.saturating_sub(1) {
        eprint!("{ANSI_CURSOR_UP_CLEAR}");
    }
    flush_stderr();
}

pub fn eprint_flush(msg: &str) {
    eprint!("{msg}");
    flush_stderr();
}

pub fn flush_stderr() {
    let _ = io::Write::flush(&mut io::stderr());
}

/// gets the terminal width in columns.
#[cfg(unix)]
pub fn terminal_width() -> usize {
    // SAFETY: `winsize` is a POD C struct; zeroing it is a valid initialised
    // state. `ioctl` writes into `ws` only on success (return value 0), and
    // we check that before reading `ws_col`.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            return ws.ws_col as usize;
        }
    }
    80
}

#[cfg(not(unix))]
pub fn terminal_width() -> usize {
    Command::new("tput")
        .arg("cols")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(80)
}

/// counts the number of visual lines a string will occupy when printed to terminal.
pub fn count_visual_lines(text: &str, width: usize) -> usize {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                1
            } else {
                // Strip ANSI escape codes to get only visible characters
                let stripped = strip(line.as_bytes());
                let visible_line = String::from_utf8_lossy(&stripped);
                // Calculate visual width accounting for wide characters
                let visual_width = visible_line.width();
                visual_width.div_ceil(width)
            }
        })
        .sum()
}

/// sets up terminal to hide control characters.
#[cfg(unix)]
pub fn setup_terminal() {
    use nix::sys::termios::{LocalFlags, SetArg, tcgetattr, tcsetattr};

    let stdin = std::io::stdin();
    if let Ok(mut termios) = tcgetattr(&stdin) {
        termios.local_flags.remove(LocalFlags::ECHOCTL);
        let _ = tcsetattr(&stdin, SetArg::TCSANOW, &termios);
    }
}

/// Disables terminal echo during loading states so typed input is not displayed,
/// keeping the cursor locked in place. Returns the saved state for [`restore_terminal_echo`].
#[cfg(unix)]
pub fn disable_terminal_echo() -> Option<nix::sys::termios::Termios> {
    use nix::sys::termios::{LocalFlags, SetArg, tcgetattr, tcsetattr};
    let stdin = std::io::stdin();
    let Ok(original) = tcgetattr(&stdin) else {
        return None;
    };
    let mut noecho = original.clone();
    noecho
        .local_flags
        .remove(LocalFlags::ECHO | LocalFlags::ECHOE);
    if tcsetattr(&stdin, SetArg::TCSANOW, &noecho).is_ok() {
        Some(original)
    } else {
        None
    }
}

/// Restores terminal echo state saved by [`disable_terminal_echo`].
#[cfg(unix)]
pub fn restore_terminal_echo(saved: &nix::sys::termios::Termios) {
    use nix::sys::termios::{SetArg, tcsetattr};
    let _ = tcsetattr(std::io::stdin(), SetArg::TCSANOW, saved);
}

/// RAII guard that puts stdin into raw mode (no ICANON, ECHO, or ISIG) and
/// restores the original termios on drop.
///
/// Returns `None` when stdin is not a tty or `tcsetattr` fails (e.g. piped
/// stdin in tests), so callers can fall back to plain reads.
#[cfg(unix)]
pub struct RawModeGuard {
    original: nix::sys::termios::Termios,
}

#[cfg(unix)]
impl RawModeGuard {
    /// Enters raw mode. Returns `None` when stdin is not a tty or the mode
    /// change fails.
    pub fn enter() -> Option<Self> {
        use nix::sys::termios::{LocalFlags, SetArg, tcgetattr, tcsetattr};
        let stdin = std::io::stdin();
        let original = tcgetattr(&stdin).ok()?;
        let mut raw = original.clone();
        raw.local_flags
            .remove(LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ISIG);
        tcsetattr(&stdin, SetArg::TCSANOW, &raw).ok()?;
        Some(Self { original })
    }
}

#[cfg(unix)]
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        use nix::sys::termios::{SetArg, tcsetattr};
        let _ = tcsetattr(std::io::stdin(), SetArg::TCSANOW, &self.original);
    }
}
