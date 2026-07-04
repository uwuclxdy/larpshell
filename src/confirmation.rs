use std::fmt::Write as _;

use colored::Colorize;

use crate::cli::is_interactive_terminal;
#[cfg(unix)]
use crate::common::RawModeGuard;
use crate::common::{
    CTP_BLUE, CTP_GREEN, CTP_PRIMARY, CTP_RED, CTP_TEXT, CTP_YELLOW, clear_n_lines,
    count_visual_lines, cursor_row_offset, flush_stderr, show_cursor, terminal_width,
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
    Esc,
    Eof,
    Other,
}

/// One-byte input source for key parsing. Abstracts a plain byte stream (a pipe
/// or the test `Cursor`) from a raw tty, where the byte *after* an ESC needs a
/// short read timeout so a lone ESC press doesn't block on the next key.
trait ByteSource {
    /// Blocking read of the next byte; `None` on EOF or I/O error.
    fn read_byte(&mut self) -> Option<u8>;
    /// Byte immediately following an ESC. `None` means no sequence follows: a
    /// bare ESC press (tty read timeout) or a lone `0x1b` at pipe EOF.
    fn after_esc(&mut self) -> Option<u8> {
        self.read_byte()
    }
}

/// One byte from a reader; `None` on EOF or I/O error. I/O errors on a raw
/// terminal (broken pipe, disconnected pty) are indistinguishable from EOF in
/// practice, so both end input.
fn read_one_byte(reader: &mut impl std::io::Read) -> Option<u8> {
    let mut byte = [0u8; 1];
    match reader.read(&mut byte) {
        Ok(n) if n > 0 => Some(byte[0]),
        _ => None,
    }
}

/// Adapts any [`std::io::Read`] into a [`ByteSource`]; the byte after ESC is a
/// plain read, so a lone `0x1b` at EOF becomes [`KeyEvent::Esc`].
struct ReadSource<'a, R: std::io::Read>(&'a mut R);

impl<R: std::io::Read> ByteSource for ReadSource<'_, R> {
    fn read_byte(&mut self) -> Option<u8> {
        read_one_byte(self.0)
    }
}

/// Parse one logical key event from any `Read` source. Works on both raw-mode
/// terminals and plain pipes (e.g. during tests with piped stdin).
pub fn parse_key_from_reader(reader: &mut impl std::io::Read) -> KeyEvent {
    parse_key(&mut ReadSource(reader))
}

fn parse_key(src: &mut impl ByteSource) -> KeyEvent {
    let Some(first) = src.read_byte() else {
        return KeyEvent::Eof;
    };
    match first {
        b'\n' | b'\r' => KeyEvent::Enter,
        b'\x03' => KeyEvent::CtrlC,
        127 | b'\x08' => KeyEvent::Backspace,
        b'\x1b' => parse_escape(src),
        c @ 32..=126 => KeyEvent::Char(c as char),
        // UTF-8 lead byte: pull the continuation bytes and assemble the char so
        // accented/CJK/emoji input survives (e.g. while editing a command).
        lead @ 0x80.. => read_utf8_char(src, lead),
        _ => KeyEvent::Other,
    }
}

/// Dispatches on the byte after an ESC: `None` is a bare ESC, `[` opens a CSI
/// sequence, `O` an SS3 sequence; anything else (Alt-<key>) is unsupported.
fn parse_escape(src: &mut impl ByteSource) -> KeyEvent {
    match src.after_esc() {
        None => KeyEvent::Esc,
        Some(b'[') => parse_csi(src),
        Some(b'O') => parse_ss3(src),
        Some(_) => KeyEvent::Other,
    }
}

/// Consumes a full CSI sequence (`ESC [` already read): parameter bytes
/// (`0x30..=0x3F`), then intermediates (`0x20..=0x2F`), then the final byte
/// (`0x40..=0x7E`). Known keys are mapped; unsupported ones are discarded whole
/// so no stray byte leaks (ctrl+arrow, ctrl+del, PgUp/PgDn/Insert).
fn parse_csi(src: &mut impl ByteSource) -> KeyEvent {
    let mut params = [0u8; 8];
    let mut len = 0usize;
    loop {
        let Some(byte) = src.read_byte() else {
            return KeyEvent::Eof;
        };
        match byte {
            0x30..=0x3F => {
                if len < params.len() {
                    params[len] = byte;
                    len += 1;
                }
            }
            0x20..=0x2F => {} // intermediate bytes: consume and ignore
            0x40..=0x7E => return map_csi(byte, &params[..len]),
            _ => return KeyEvent::Other, // malformed; bail without leaking bytes
        }
    }
}

/// Maps a completed CSI sequence to a key by its final byte (and leading numeric
/// parameter for the `~`-terminated family). Modifier params (e.g. `1;5` for
/// ctrl) are ignored, so ctrl+arrow folds onto the plain arrow.
fn map_csi(final_byte: u8, params: &[u8]) -> KeyEvent {
    match final_byte {
        b'A' => KeyEvent::ArrowUp,
        b'C' => KeyEvent::Right,
        b'D' => KeyEvent::Left,
        b'H' => KeyEvent::Home,
        b'F' => KeyEvent::End,
        b'~' => match leading_param(params) {
            1 | 7 => KeyEvent::Home,
            3 => KeyEvent::Delete,
            4 | 8 => KeyEvent::End,
            _ => KeyEvent::Other, // Insert(2), PgUp(5), PgDn(6): unsupported
        },
        _ => KeyEvent::Other,
    }
}

/// Leading decimal parameter of a CSI sequence (digits up to the first `;`), or
/// 0 when there is none.
fn leading_param(params: &[u8]) -> u32 {
    let mut value = 0u32;
    for &byte in params {
        if byte.is_ascii_digit() {
            value = value * 10 + u32::from(byte - b'0');
        } else {
            break;
        }
    }
    value
}

/// Consumes an SS3 sequence (`ESC O` already read): a single final byte for the
/// application-mode arrow/navigation keys some terminals emit.
fn parse_ss3(src: &mut impl ByteSource) -> KeyEvent {
    match src.read_byte() {
        Some(b'A') => KeyEvent::ArrowUp,
        Some(b'C') => KeyEvent::Right,
        Some(b'D') => KeyEvent::Left,
        Some(b'H') => KeyEvent::Home,
        Some(b'F') => KeyEvent::End,
        _ => KeyEvent::Other,
    }
}

/// Reads the continuation bytes of a multibyte UTF-8 char whose `lead` byte was
/// already consumed, returning the assembled [`KeyEvent::Char`]. A truncated
/// sequence (stray continuation byte or EOF mid-char) maps to
/// [`KeyEvent::Other`] so the edit/confirm loop treats it as a no-op.
fn read_utf8_char(src: &mut impl ByteSource, lead: u8) -> KeyEvent {
    let extra = match lead {
        0xC0..=0xDF => 1,
        0xE0..=0xEF => 2,
        0xF0..=0xF7 => 3,
        _ => return KeyEvent::Other,
    };
    let mut bytes = [0u8; 4];
    bytes[0] = lead;
    for slot in bytes.iter_mut().take(1 + extra).skip(1) {
        let Some(byte) = src.read_byte() else {
            return KeyEvent::Other;
        };
        *slot = byte;
    }
    match std::str::from_utf8(&bytes[..1 + extra]) {
        Ok(text) => text.chars().next().map_or(KeyEvent::Other, KeyEvent::Char),
        Err(_) => KeyEvent::Other,
    }
}

#[cfg(unix)]
fn flush_stdin_input() {
    use nix::sys::termios::{FlushArg, tcflush};
    let _ = tcflush(std::io::stdin(), FlushArg::TCIFLUSH);
}

#[cfg(not(unix))]
fn flush_stdin_input() {}

/// Deciseconds to wait for the byte after an ESC before deciding the ESC was
/// pressed alone. A CSI/SS3 tail from the terminal arrives well within this;
/// only a bare ESC waits the full window.
#[cfg(unix)]
const ESC_FOLLOW_TIMEOUT_DECISECONDS: u8 = 1;

/// Byte after an ESC on a raw tty, read with a `VMIN=0`/`VTIME` timeout so a
/// lone ESC returns `None` instead of blocking on the next key. Reads through
/// the same buffered stdin as [`read_one_byte`], so a terminal that delivered
/// the whole escape sequence in one burst still yields the buffered byte at once
/// and only a genuinely lone ESC hits the timeout.
#[cfg(unix)]
fn read_esc_follow_byte(reader: &mut impl std::io::Read) -> Option<u8> {
    use nix::sys::termios::{SetArg, SpecialCharacterIndices, tcgetattr, tcsetattr};
    let stdin = std::io::stdin();
    let Ok(saved) = tcgetattr(&stdin) else {
        return read_one_byte(reader);
    };
    let mut timed = saved.clone();
    timed.control_chars[SpecialCharacterIndices::VMIN as usize] = 0;
    timed.control_chars[SpecialCharacterIndices::VTIME as usize] = ESC_FOLLOW_TIMEOUT_DECISECONDS;
    if tcsetattr(&stdin, SetArg::TCSANOW, &timed).is_err() {
        return read_one_byte(reader);
    }
    let byte = read_one_byte(reader);
    let _ = tcsetattr(&stdin, SetArg::TCSANOW, &saved);
    byte
}

/// Raw-tty [`ByteSource`]: a lone ESC is disambiguated with a short read timeout
/// (see [`read_esc_follow_byte`]).
#[cfg(unix)]
struct TtySource<'a, R: std::io::Read>(&'a mut R);

#[cfg(unix)]
impl<R: std::io::Read> ByteSource for TtySource<'_, R> {
    fn read_byte(&mut self) -> Option<u8> {
        read_one_byte(self.0)
    }
    fn after_esc(&mut self) -> Option<u8> {
        read_esc_follow_byte(self.0)
    }
}

/// Runs `run` with a `read_key` closure over stdin, holding a single raw-mode
/// guard for the whole confirm/edit interaction so keys are read without
/// toggling the tty back to cooked mode between presses (a paste stays intact, a
/// mid-redraw Ctrl-C parses as cancel instead of terminating the process). A
/// non-tty (piped stdin in tests) falls back to plain buffered reads.
#[cfg(unix)]
fn read_confirm<T>(run: impl FnOnce(&mut dyn FnMut() -> KeyEvent) -> T) -> T {
    if let Some(_guard) = RawModeGuard::enter() {
        let mut stdin = std::io::stdin().lock();
        let mut read = move || parse_key(&mut TtySource(&mut stdin));
        run(&mut read)
    } else {
        let mut stdin = std::io::stdin().lock();
        let mut read = move || parse_key_from_reader(&mut stdin);
        run(&mut read)
    }
}

#[cfg(not(unix))]
fn read_confirm<T>(run: impl FnOnce(&mut dyn FnMut() -> KeyEvent) -> T) -> T {
    let mut stdin = std::io::stdin().lock();
    let mut read = move || parse_key_from_reader(&mut stdin);
    run(&mut read)
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
            KeyEvent::Char('n' | 'N') | KeyEvent::CtrlC | KeyEvent::Esc => {
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

    read_confirm(|read_key| {
        confirm_from_reader(
            read_key,
            ConfirmPromptMode::WithExplain,
            cmd_line_count,
            0, // no ephemeral explanation lines in WithExplain mode
        )
    })
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

    read_confirm(|read_key| {
        confirm_from_reader(
            read_key,
            ConfirmPromptMode::Simple,
            cmd_line_count,
            expl_line_count,
        )
    })
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
/// or `None` when the edit is cancelled (Ctrl+C, Esc, or EOF).
pub fn edit_command(current: &str) -> Option<String> {
    read_confirm(|read_key| edit_loop(current, read_key))
}

fn edit_loop(current: &str, read_key: &mut dyn FnMut() -> KeyEvent) -> Option<String> {
    let mut buf: Vec<char> = current.chars().collect();
    let mut pos = buf.len();

    let hint = format!(
        "[{}] to confirm, [{}] to cancel",
        "Enter".custom_color(CTP_PRIMARY).bold(),
        "Ctrl+C".custom_color(CTP_PRIMARY).bold()
    );

    // Row offset (from the region's first row) where the edit cursor currently
    // sits. Lets the next redraw walk back to the top before erasing.
    let mut cursor_row = 0usize;

    // Redraw the whole editor region and leave the cursor at the edit point.
    // The terminal does the wrapping, so a command that wraps, contains wide
    // (CJK/emoji) characters, or spans multiple lines all render correctly.
    let draw = |buf: &[char], pos: usize, cursor_row: &mut usize| {
        let width = terminal_width();
        let prefix: String = buf[..pos].iter().collect();
        let rest: String = buf[pos..].iter().collect();

        // Walk to the region's top-left, then clear it and everything below.
        let mut seq = String::new();
        if *cursor_row > 0 {
            let _ = write!(seq, "\x1b[{cursor_row}A");
        }
        seq.push_str("\r\x1b[J");
        eprint!("{seq}");

        // Draw the whole region: the command line, then the hint on the line
        // below.
        eprint!(
            "{} {}{}\n{}",
            "$".custom_color(CTP_PRIMARY),
            prefix.custom_color(CTP_TEXT).bold(),
            rest.custom_color(CTP_TEXT).bold(),
            hint.custom_color(CTP_BLUE)
        );

        // Return to the edit point with relative moves only. The cursor now sits
        // on the hint's last row; walk up to the region top and reprint the
        // prefix to land back at the edit column. An absolute DECSC/DECRC save
        // would drift here: printing the hint can scroll the screen and
        // invalidate a saved position, whereas the row distance between two
        // printed points survives a scroll.
        let cmd_rows = count_visual_lines(&format!("$ {prefix}{rest}"), width);
        let hint_rows = count_visual_lines(&hint, width);
        let rows_to_top = cmd_rows.saturating_sub(1) + hint_rows;
        let mut back = String::new();
        if rows_to_top > 0 {
            let _ = write!(back, "\x1b[{rows_to_top}A");
        }
        back.push('\r');
        eprint!("{back}");
        eprint!(
            "{} {}",
            "$".custom_color(CTP_PRIMARY),
            prefix.custom_color(CTP_TEXT).bold()
        );
        flush_stderr();

        *cursor_row = cursor_row_offset(&format!("$ {prefix}"), width);
    };

    // Erase the editor region, leaving the cursor at its top-left.
    let clear_editor = |cursor_row: usize| {
        let mut seq = String::new();
        if cursor_row > 0 {
            let _ = write!(seq, "\x1b[{cursor_row}A");
        }
        seq.push_str("\r\x1b[J");
        eprint!("{seq}");
        flush_stderr();
    };

    draw(&buf, pos, &mut cursor_row);

    loop {
        match read_key() {
            KeyEvent::Enter => {
                clear_editor(cursor_row);
                return Some(buf.into_iter().collect());
            }
            KeyEvent::CtrlC | KeyEvent::Eof | KeyEvent::Esc => {
                clear_editor(cursor_row);
                return None;
            }
            KeyEvent::Backspace => {
                if pos > 0 {
                    buf.remove(pos - 1);
                    pos -= 1;
                    draw(&buf, pos, &mut cursor_row);
                }
            }
            KeyEvent::Delete => {
                if pos < buf.len() {
                    buf.remove(pos);
                    draw(&buf, pos, &mut cursor_row);
                }
            }
            KeyEvent::Left => {
                if pos > 0 {
                    pos -= 1;
                    draw(&buf, pos, &mut cursor_row);
                }
            }
            KeyEvent::Right => {
                if pos < buf.len() {
                    pos += 1;
                    draw(&buf, pos, &mut cursor_row);
                }
            }
            KeyEvent::Home => {
                pos = 0;
                draw(&buf, pos, &mut cursor_row);
            }
            KeyEvent::End => {
                pos = buf.len();
                draw(&buf, pos, &mut cursor_row);
            }
            KeyEvent::Char(c) => {
                buf.insert(pos, c);
                pos += 1;
                draw(&buf, pos, &mut cursor_row);
            }
            KeyEvent::ArrowUp | KeyEvent::Other => {}
        }
    }
}
