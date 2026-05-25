use colored::Colorize;

use crate::cli::is_interactive_terminal;
#[cfg(unix)]
use crate::common::RawModeGuard;
use crate::common::{
    ANSI_CLEAR_LINE, CTP_BLUE, CTP_GREEN, CTP_PRIMARY, CTP_RED, CTP_TEXT, CTP_YELLOW,
    clear_n_lines, count_visual_lines, flush_stderr, show_cursor, terminal_width,
};

pub enum ConfirmResult {
    Yes,
    No,
    Explain,
    Edit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmPromptMode {
    WithExplain,
    Simple,
}

pub enum KeyEvent {
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
pub fn parse_key_from_reader(reader: &mut impl std::io::Read) -> KeyEvent {
    let mut key_byte = [0u8; 1];
    // I/O errors on a raw terminal (broken pipe, disconnected pty) are
    // indistinguishable from EOF in practice; treat both as EOF.
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
    // Attempt raw mode; fall back to plain reads (e.g. piped stdin in tests).
    if let Some(_guard) = RawModeGuard::enter() {
        return parse_key_from_reader(&mut std::io::stdin().lock()); // _guard drops here
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
    let styled = style_message_markup(text);
    let width = terminal_width();
    let mut visual = 0;
    for (index, line) in styled.lines().enumerate() {
        let prefix = if index == 0 { "● " } else { "" };
        visual += count_visual_lines(&format!("{prefix}{line}"), width);
        eprintln!(
            "{}{}",
            prefix.custom_color(prefix_color),
            line.custom_color(CTP_TEXT)
        );
    }
    visual
}

pub(crate) fn style_message_markup(text: &str) -> String {
    style_message_markup_with_color(text, colored::control::SHOULD_COLORIZE.should_colorize())
}

fn style_message_markup_with_color(text: &str, use_color: bool) -> String {
    let mut styled = String::new();

    for (index, line) in text.lines().enumerate() {
        if index > 0 {
            styled.push('\n');
        }
        styled.push_str(&style_message_line(line, use_color));
    }

    styled
}

fn style_message_line(line: &str, use_color: bool) -> String {
    let stripped = strip_markdown_prefixes(line);
    let without_links = strip_markdown_links(stripped);

    if use_color {
        let styled = apply_surrounded_style(&without_links, "`", "\x1b[7m", "\x1b[27m");
        let styled = apply_surrounded_style(&styled, "**", "\x1b[1m", "\x1b[22m");
        let styled = apply_surrounded_style(&styled, "__", "\x1b[1m", "\x1b[22m");
        let styled = apply_surrounded_style(&styled, "*", "\x1b[3m", "\x1b[23m");
        let styled = apply_surrounded_style(&styled, "_", "\x1b[3m", "\x1b[23m");
        apply_surrounded_style(&styled, "~~", "", "")
    } else {
        let styled = strip_surrounded_markers(&without_links, "`");
        let styled = strip_surrounded_markers(&styled, "**");
        let styled = strip_surrounded_markers(&styled, "__");
        let styled = strip_surrounded_markers(&styled, "*");
        let styled = strip_surrounded_markers(&styled, "_");
        strip_surrounded_markers(&styled, "~~")
    }
}

#[cfg(test)]
pub(crate) fn style_message_markup_for_test(text: &str, use_color: bool) -> String {
    style_message_markup_with_color(text, use_color)
}

#[cfg(test)]
pub(crate) fn style_html_tags_for_test(text: &str, use_color: bool) -> String {
    style_html_tags_with_color(text, use_color)
}

fn style_html_tags_with_color(text: &str, use_color: bool) -> String {
    if use_color {
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

pub fn style_html_tags(text: &str) -> String {
    style_html_tags_with_color(text, colored::control::SHOULD_COLORIZE.should_colorize())
}

fn strip_markdown_prefixes(line: &str) -> &str {
    let mut rest = line.trim_start();

    if rest.starts_with("```") || rest.starts_with("~~~") {
        return "";
    }

    if rest.len() >= 3 && rest.chars().all(|ch| matches!(ch, '-' | '*' | '_')) {
        return "";
    }

    while let Some(stripped) = rest.strip_prefix('>') {
        rest = stripped.trim_start();
    }

    let heading_len = rest.bytes().take_while(|&byte| byte == b'#').count();
    if heading_len > 0 {
        let heading_rest = &rest[heading_len..];
        if let Some(stripped) = heading_rest.strip_prefix(' ') {
            rest = stripped.trim_start();
        }
    }

    if let Some(stripped) = rest
        .strip_prefix("- ")
        .or_else(|| rest.strip_prefix("* "))
        .or_else(|| rest.strip_prefix("+ "))
    {
        rest = stripped;
    }

    rest = strip_ordered_list_marker(rest);

    if let Some(stripped) = rest
        .strip_prefix("[ ] ")
        .or_else(|| rest.strip_prefix("[x] "))
        .or_else(|| rest.strip_prefix("[X] "))
    {
        rest = stripped;
    }

    rest
}

fn strip_ordered_list_marker(line: &str) -> &str {
    let digit_count = line
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();

    if digit_count == 0 || line.len() <= digit_count + 1 {
        return line;
    }

    let marker = line.as_bytes()[digit_count];
    let separator = line.as_bytes()[digit_count + 1];

    if matches!(marker, b'.' | b')') && separator == b' ' {
        &line[digit_count + 2..]
    } else {
        line
    }
}

fn strip_markdown_links(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut stripped = String::new();
    let mut index = 0;

    for (char_index, ch) in text.char_indices() {
        if char_index < index {
            continue;
        }

        if bytes[char_index] == b'!'
            && char_index + 1 < bytes.len()
            && bytes[char_index + 1] == b'['
            && let Some((label, next_index)) = parse_markdown_link(text, char_index + 1)
        {
            stripped.push_str(label);
            index = next_index;
            continue;
        }

        if bytes[char_index] == b'['
            && let Some((label, next_index)) = parse_markdown_link(text, char_index)
        {
            stripped.push_str(label);
            index = next_index;
            continue;
        }

        stripped.push(ch);
        index = char_index + ch.len_utf8();
    }

    stripped
}

fn parse_markdown_link(text: &str, bracket_index: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    let label_start = bracket_index + 1;
    let label_end = bytes[label_start..].iter().position(|&byte| byte == b']')? + label_start;
    let paren_start = label_end + 1;

    if bytes.get(paren_start) != Some(&b'(') {
        return None;
    }

    let paren_end = bytes[paren_start + 1..]
        .iter()
        .position(|&byte| byte == b')')?
        + paren_start
        + 1;

    Some((&text[label_start..label_end], paren_end + 1))
}

fn apply_surrounded_style(text: &str, delimiter: &str, open: &str, close: &str) -> String {
    let mut styled = String::new();
    let mut rest = text;

    while let Some(start) = rest.find(delimiter) {
        let (before, after_start) = rest.split_at(start);
        styled.push_str(before);

        let after_start = &after_start[delimiter.len()..];
        let Some(end) = after_start.find(delimiter) else {
            styled.push_str(delimiter);
            styled.push_str(after_start);
            return styled;
        };

        let (inner, after_end) = after_start.split_at(end);
        if inner.is_empty()
            || inner.starts_with(char::is_whitespace)
            || inner.ends_with(char::is_whitespace)
        {
            styled.push_str(delimiter);
            styled.push_str(inner);
            styled.push_str(delimiter);
        } else {
            styled.push_str(open);
            styled.push_str(inner);
            styled.push_str(close);
        }

        rest = &after_end[delimiter.len()..];
    }

    styled.push_str(rest);
    styled
}

fn strip_surrounded_markers(text: &str, delimiter: &str) -> String {
    apply_surrounded_style(text, delimiter, "", "")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyLevel {
    Safe,
    Unsafe,
    Dangerous,
}

const fn safety_color(level: SafetyLevel) -> colored::CustomColor {
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

pub fn confirm_from_reader(
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
                return ConfirmResult::Cancel;
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
pub fn confirm_with_explain(cmd_line_count: usize) -> ConfirmResult {
    if !is_interactive_terminal() {
        return ConfirmResult::Yes;
    }

    flush_stderr();
    flush_stdin_input();

    confirm_from_reader(
        read_key_event,
        ConfirmPromptMode::WithExplain,
        cmd_line_count,
        0, // no ephemeral explanation lines in WithExplain mode
    )
}

/// Prompt without the explain option.
/// `cmd_line_count` = persistent command lines (kept on Y/Enter).
/// `expl_line_count` = ephemeral explanation lines (cleared on Y/Enter).
pub fn confirm_execution(cmd_line_count: usize, expl_line_count: usize) -> ConfirmResult {
    if !is_interactive_terminal() {
        return ConfirmResult::Yes;
    }

    flush_stderr();
    flush_stdin_input();

    confirm_from_reader(
        read_key_event,
        ConfirmPromptMode::Simple,
        cmd_line_count,
        expl_line_count,
    )
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
            KeyEvent::CtrlC | KeyEvent::Eof => {
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
            KeyEvent::ArrowUp | KeyEvent::Other => {}
        }
    }
}
