use colored::*;

use crate::cli::is_interactive_terminal;
use crate::common::{
    ANSI_CLEAR_LINE, CTP_BLUE, CTP_GREEN, CTP_PRIMARY, CTP_RED, CTP_TEXT, CTP_YELLOW, EXIT_SIGINT,
    clear_n_lines, count_visual_lines, exit_with_code, flush_stderr, show_cursor, terminal_width,
};
use crate::error::LarpshellError;

pub enum ConfirmResult {
    Yes,
    No,
    Explain,
    Edit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ConfirmPromptMode {
    WithExplain,
    Simple,
}

pub(crate) enum KeyEvent {
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Enter,
    CtrlC,
    ArrowUp,
    Eof,
    Other,
}

/// Parse one logical key event from any `Read` source. Works on both raw-mode
/// terminals and plain pipes (e.g. during tests with piped stdin).
pub(crate) fn parse_key_from_reader(reader: &mut impl std::io::Read) -> KeyEvent {
    let mut key_byte = [0u8; 1];
    if reader.read(&mut key_byte).unwrap_or(0) == 0 {
        return KeyEvent::Eof;
    }
    match key_byte[0] {
        b'\n' | b'\r' => KeyEvent::Enter,
        b'\x03' => KeyEvent::CtrlC,
        127 | b'\x08' => KeyEvent::Backspace,
        b'\x1b' => {
            if reader.read(&mut key_byte).unwrap_or(0) == 0 {
                return KeyEvent::Eof;
            }
            if key_byte[0] != b'[' {
                return KeyEvent::Other;
            }
            if reader.read(&mut key_byte).unwrap_or(0) == 0 {
                return KeyEvent::Eof;
            }
            match key_byte[0] {
                b'A' => KeyEvent::ArrowUp,
                b'C' => KeyEvent::Right,
                b'D' => KeyEvent::Left,
                b'H' => KeyEvent::Home,
                b'F' => KeyEvent::End,
                b'3' => {
                    let _ = reader.read(&mut key_byte); // consume '~'
                    KeyEvent::Delete
                }
                b'1' => {
                    let _ = reader.read(&mut key_byte); // consume '~'
                    KeyEvent::Home
                }
                b'4' => {
                    let _ = reader.read(&mut key_byte); // consume '~'
                    KeyEvent::End
                }
                _ => KeyEvent::Other,
            }
        }
        c @ 32..=126 => KeyEvent::Char(c as char),
        _ => KeyEvent::Other,
    }
}

#[cfg(unix)]
fn flush_stdin_input() {
    use nix::sys::termios::{FlushArg, tcflush};
    let _ = tcflush(std::io::stdin(), FlushArg::TCIFLUSH);
}

#[cfg(not(unix))]
fn flush_stdin_input() {}

#[cfg(unix)]
fn read_key_event() -> KeyEvent {
    use nix::sys::termios::{LocalFlags, SetArg, tcgetattr, tcsetattr};

    let stdin_handle = std::io::stdin();

    // Attempt raw mode; fall back to plain reads (e.g. piped stdin in tests).
    if let Ok(original) = tcgetattr(&stdin_handle) {
        let mut raw = original.clone();
        raw.local_flags
            .remove(LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ISIG);
        if tcsetattr(&stdin_handle, SetArg::TCSANOW, &raw).is_ok() {
            let result = parse_key_from_reader(&mut stdin_handle.lock());
            let _ = tcsetattr(&stdin_handle, SetArg::TCSANOW, &original);
            return result;
        }
        let _ = tcsetattr(&stdin_handle, SetArg::TCSANOW, &original);
    }

    parse_key_from_reader(&mut std::io::stdin().lock())
}

#[cfg(not(unix))]
fn read_key_event() -> KeyEvent {
    parse_key_from_reader(&mut std::io::stdin().lock())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseStyle {
    Command,
    Message,
}

pub fn display_response(text: &str, style: ResponseStyle) -> usize {
    match style {
        ResponseStyle::Command => display_command(text),
        ResponseStyle::Message => display_message(text),
    }
}

pub fn display_command(command: &str) -> usize {
    let width = terminal_width();
    if command.lines().count() == 1 {
        let visual = count_visual_lines(&format!("$ {command}"), width);
        eprintln!(
            "{} {}",
            "$".custom_color(CTP_PRIMARY),
            command.custom_color(CTP_TEXT).bold()
        );
        visual
    } else {
        let mut visual = count_visual_lines("> multiline command:", width);
        eprintln!(
            "{} {}",
            ">".custom_color(CTP_PRIMARY),
            "multiline command:".custom_color(CTP_TEXT).bold()
        );
        for line in command.lines() {
            visual += count_visual_lines(&format!("$ {line}"), width);
            eprintln!(
                "{} {}",
                "$".custom_color(CTP_PRIMARY),
                line.custom_color(CTP_TEXT)
            );
        }
        visual
    }
}

pub fn display_message(message: &str) -> usize {
    display_bulleted(message, CTP_BLUE)
}

fn display_bulleted(text: &str, prefix_color: colored::CustomColor) -> usize {
    let width = terminal_width();
    let mut visual = 0;
    for (index, line) in text.lines().enumerate() {
        let prefix = if index == 0 { "● " } else { "  " };
        visual += count_visual_lines(&format!("{prefix}{line}"), width);
        eprintln!(
            "{}{}",
            prefix.custom_color(prefix_color),
            line.custom_color(CTP_TEXT)
        );
    }
    visual
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyLevel {
    Safe,
    Unsafe,
    Dangerous,
}

fn safety_color(level: SafetyLevel) -> colored::CustomColor {
    match level {
        SafetyLevel::Safe => CTP_GREEN,
        SafetyLevel::Unsafe => CTP_YELLOW,
        SafetyLevel::Dangerous => CTP_RED,
    }
}

fn parse_safety_level(text: &str) -> (SafetyLevel, &str) {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("SAFE:") {
        return (SafetyLevel::Safe, rest.trim_start());
    }
    if let Some(rest) = trimmed.strip_prefix("UNSURE:") {
        return (SafetyLevel::Unsafe, rest.trim_start());
    }
    if let Some(rest) = trimmed.strip_prefix("DANGEROUS:") {
        return (SafetyLevel::Dangerous, rest.trim_start());
    }
    (SafetyLevel::Unsafe, trimmed)
}

pub fn display_explanation(explanation: &str) -> usize {
    let styled = style_html_tags(explanation);
    let (first, tail) = styled.split_once('\n').unwrap_or((&styled, ""));
    let (level, rest) = parse_safety_level(first);
    let body = if tail.is_empty() {
        rest.to_string()
    } else {
        format!("{rest}\n{tail}")
    };
    display_bulleted(&body, safety_color(level))
}

pub(crate) fn style_html_tags(text: &str) -> String {
    if colored::control::SHOULD_COLORIZE.should_colorize() {
        text.replace("<b>", "\x1b[1m")
            .replace("</b>", "\x1b[22m")
            .replace("<i>", "\x1b[3m")
            .replace("</i>", "\x1b[23m")
            .replace("<u>", "\x1b[4m")
            .replace("</u>", "\x1b[24m")
    } else {
        text.replace("<b>", "")
            .replace("</b>", "")
            .replace("<i>", "")
            .replace("</i>", "")
            .replace("<u>", "")
            .replace("</u>", "")
    }
}

pub(crate) fn confirm_from_reader(
    mut read_key: impl FnMut() -> KeyEvent,
    mode: ConfirmPromptMode,
    cmd_line_count: usize,
    expl_line_count: usize,
) -> ConfirmResult {
    let prompt_lines = confirmation_prompt(mode);
    let lines_to_clear = cmd_line_count + expl_line_count + prompt_lines;

    loop {
        match read_key() {
            KeyEvent::Enter | KeyEvent::Char('y' | 'Y') => {
                // WithExplain clears only prompt (keeps command + explanation).
                // Simple clears explanation + prompt (keeps command).
                let clear_count = if matches!(mode, ConfirmPromptMode::WithExplain) {
                    prompt_lines
                } else {
                    expl_line_count + prompt_lines
                };
                clear_n_lines(clear_count);
                return ConfirmResult::Yes;
            }
            KeyEvent::Char('e' | 'E') if matches!(mode, ConfirmPromptMode::WithExplain) => {
                clear_n_lines(prompt_lines);
                return ConfirmResult::Explain;
            }
            KeyEvent::ArrowUp => {
                clear_n_lines(lines_to_clear);
                return ConfirmResult::Edit;
            }
            KeyEvent::Char('n' | 'N') => {
                clear_n_lines(lines_to_clear);
                return ConfirmResult::Cancel;
            }
            KeyEvent::CtrlC => {
                clear_n_lines(lines_to_clear);
                show_cursor();
                exit_with_code(EXIT_SIGINT);
            }
            KeyEvent::Eof => {
                clear_n_lines(lines_to_clear);
                show_cursor();
                return ConfirmResult::No;
            }
            _ => {}
        }
    }
}

/// Prompt for confirmation with explain option
pub fn confirm_with_explain(cmd_line_count: usize) -> Result<ConfirmResult, LarpshellError> {
    if !is_interactive_terminal() {
        return Ok(ConfirmResult::Yes);
    }

    flush_stderr();
    flush_stdin_input();

    let result = confirm_from_reader(
        read_key_event,
        ConfirmPromptMode::WithExplain,
        cmd_line_count,
        0, // no ephemeral explanation lines in WithExplain mode
    );
    Ok(result)
}

/// Prompt without the explain option.
/// `cmd_line_count` = persistent command lines (kept on Y/Enter).
/// `expl_line_count` = ephemeral explanation lines (cleared on Y/Enter).
pub fn confirm_execution(
    cmd_line_count: usize,
    expl_line_count: usize,
) -> Result<ConfirmResult, LarpshellError> {
    if !is_interactive_terminal() {
        return Ok(ConfirmResult::Yes);
    }

    flush_stderr();
    flush_stdin_input();

    let result = confirm_from_reader(
        read_key_event,
        ConfirmPromptMode::Simple,
        cmd_line_count,
        expl_line_count,
    );
    Ok(result)
}

fn confirmation_prompt(mode: ConfirmPromptMode) -> usize {
    let width = terminal_width();
    let header = "Run this?".custom_color(CTP_YELLOW).to_string();
    let mut visual = count_visual_lines(&header, width);
    eprintln!("{header}");
    let hint = if matches!(mode, ConfirmPromptMode::WithExplain) {
        format!(
            "[{}] to execute, [{}] to explain, [{}] to edit, [{}] to cancel",
            "Y/Enter".custom_color(CTP_PRIMARY).bold(),
            "E".custom_color(CTP_PRIMARY).bold(),
            "Arrow Up".custom_color(CTP_PRIMARY).bold(),
            "N".custom_color(CTP_PRIMARY).bold()
        )
    } else {
        format!(
            "[{}] to execute, [{}] to edit, [{}] to cancel",
            "Y/Enter".custom_color(CTP_PRIMARY).bold(),
            "Arrow Up".custom_color(CTP_PRIMARY).bold(),
            "N".custom_color(CTP_PRIMARY).bold()
        )
    };
    visual += count_visual_lines(&hint, width);
    eprint!("{}", hint.custom_color(CTP_BLUE));
    visual
}

/// Presents the command for inline editing. The caller must have already cleared the
/// confirmation prompt lines from the terminal. Returns the edited command on Enter,
/// or exits with code 130 on Ctrl+C.
pub fn edit_command(current: &str) -> Option<String> {
    let width = terminal_width();
    let mut buf: Vec<char> = current.chars().collect();
    let mut pos = buf.len();

    let hint_text = format!(
        "[{}] to confirm, [{}] to cancel",
        "Enter".custom_color(CTP_PRIMARY).bold(),
        "Ctrl+C".custom_color(CTP_PRIMARY).bold()
    );
    let hint_rows = count_visual_lines("[Enter] to confirm, [Ctrl+C] to cancel", width);

    // Draw: command on current line (no newline), hint on the line below.
    // Then move cursor back up to the command line.
    let init: String = buf.iter().collect();
    eprint!(
        "{} {}",
        "$".custom_color(CTP_PRIMARY),
        init.custom_color(CTP_TEXT).bold()
    );
    eprintln!(); // move to hint line
    eprint!("{}", hint_text.custom_color(CTP_BLUE));
    // cursor up 1 line, then set absolute column: "$ " = 2 visible chars, 1-indexed
    eprint!("\x1b[1A\x1b[{}G", 3 + pos);
    flush_stderr();

    // clear the editor display (command + hint) from the terminal.
    // cursor is on the first command row; move to last hint row then clear upward.
    let clear_editor = |buf: &[char]| {
        let cmd_text = format!("$ {}", buf.iter().collect::<String>());
        let cmd_rows = count_visual_lines(&cmd_text, width);
        let total = cmd_rows + hint_rows;
        // move cursor from first command row to last hint row
        for _ in 0..total.saturating_sub(1) {
            eprint!("\x1b[1B");
        }
        clear_n_lines(total);
    };

    let redraw = |buf: &[char], pos: usize| {
        let s: String = buf.iter().collect();
        // cursor is on the command line; clear it and redraw
        eprint!(
            "{}{} {}",
            ANSI_CLEAR_LINE,
            "$".custom_color(CTP_PRIMARY),
            s.custom_color(CTP_TEXT).bold()
        );
        eprint!("\x1b[{}G", 3 + pos);
        flush_stderr();
    };

    loop {
        match read_key_event() {
            KeyEvent::Enter => {
                clear_editor(&buf);
                flush_stderr();
                return Some(buf.into_iter().collect());
            }
            KeyEvent::CtrlC => {
                clear_editor(&buf);
                flush_stderr();
                return None;
            }
            KeyEvent::Backspace => {
                if pos > 0 {
                    buf.remove(pos - 1);
                    pos -= 1;
                    redraw(&buf, pos);
                }
            }
            KeyEvent::Delete => {
                if pos < buf.len() {
                    buf.remove(pos);
                    redraw(&buf, pos);
                }
            }
            KeyEvent::Left => {
                if pos > 0 {
                    pos -= 1;
                    eprint!("\x1b[1D");
                    flush_stderr();
                }
            }
            KeyEvent::Right => {
                if pos < buf.len() {
                    pos += 1;
                    eprint!("\x1b[1C");
                    flush_stderr();
                }
            }
            KeyEvent::Home => {
                pos = 0;
                eprint!("\x1b[3G"); // column 3: after "$ "
                flush_stderr();
            }
            KeyEvent::End => {
                pos = buf.len();
                eprint!("\x1b[{}G", 3 + pos);
                flush_stderr();
            }
            KeyEvent::Char(c) => {
                buf.insert(pos, c);
                pos += 1;
                redraw(&buf, pos);
            }
            KeyEvent::Eof => {
                clear_editor(&buf);
                flush_stderr();
                return None;
            }
            KeyEvent::ArrowUp | KeyEvent::Other => {}
        }
    }
}
