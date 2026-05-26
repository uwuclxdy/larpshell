use std::borrow::Cow;
use std::fmt::Write as _;
use std::io;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use colored::Colorize;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{
    Cmd, CompletionType, ConditionalEventHandler, Config, Editor, Event, EventContext,
    EventHandler, Helper, KeyCode, KeyEvent, Modifiers, RepeatCount,
};

use crate::common::{CTP_BLUE, CTP_OVERLAY0, CTP_PRIMARY, current_directory_display, show_cursor};
use crate::config;
use crate::slash_commands;

static PREVIEW_LINE_COUNT: AtomicUsize = AtomicUsize::new(0);
static SHELL_MODE: AtomicBool = AtomicBool::new(false);

// Longest command name length (used for column alignment).
// "uninstall" = 9 chars. Column = 2 (indent) + 1 (/) + 9 (name) + 4 (gap) = 16
const PREVIEW_DESC_COL: usize = 16;

pub fn format_preview_row(cmd_name: &str, typed_len: usize, description: &str) -> String {
    let split = typed_len.min(cmd_name.len());
    let (typed, untyped) = cmd_name.split_at(split);
    let pad = PREVIEW_DESC_COL.saturating_sub(cmd_name.len() + 3);
    format!(
        "  {}{}{}{}",
        typed.custom_color(CTP_PRIMARY).bold(),
        untyped.custom_color(CTP_OVERLAY0),
        " ".repeat(pad + 4),
        description.custom_color(CTP_OVERLAY0),
    )
}

/// Erase all currently-drawn preview lines below the prompt.
/// Must be called while the cursor is on the prompt line.
pub fn clear_slash_preview() {
    let n = PREVIEW_LINE_COUNT.swap(0, Ordering::Relaxed);
    if n == 0 {
        return;
    }
    // Move down to each preview line and erase it, then return to prompt line.
    let mut seq = String::new();
    for _ in 0..n {
        seq.push_str("\n\x1b[K");
    }
    let _ = write!(seq, "\x1b[{n}A\r");
    print!("{seq}");
    let _ = io::stdout().flush();
}

/// Draw a filtered command or argument preview below the current prompt line.
/// Redraws from scratch: erases old lines, writes new ones, returns cursor to prompt line.
pub fn draw_slash_preview(line: &str) {
    if line.contains(' ') {
        draw_arg_preview(line);
        return;
    }

    let matches = slash_commands::filter(line);
    let prev_count = PREVIEW_LINE_COUNT.load(Ordering::Relaxed);
    let new_count = matches.len();
    let max_lines = prev_count.max(new_count);

    if max_lines == 0 {
        return;
    }

    let typed_len = line.len();
    let mut seq = String::new();

    // Erase old lines and write new ones in a single downward pass.
    for i in 0..max_lines {
        seq.push_str("\n\x1b[K"); // move down one line, erase it
        if let Some(cmd) = matches.get(i) {
            let row = format_preview_row(&format!("/{}", cmd.name), typed_len, cmd.description);
            seq.push('\r');
            seq.push_str(&row);
        }
    }
    // Return cursor to the prompt line.
    let _ = write!(seq, "\x1b[{max_lines}A\r");

    PREVIEW_LINE_COUNT.store(new_count, Ordering::Relaxed);
    print!("{seq}");
    let _ = io::stdout().flush();
}

fn draw_arg_preview(line: &str) {
    let prev_count = PREVIEW_LINE_COUNT.load(Ordering::Relaxed);

    let Some((start, choices)) = slash_commands::arg_completions(line) else {
        clear_slash_preview();
        return;
    };

    let partial_len = line.len() - start;
    let new_count = choices.len();
    let max_lines = prev_count.max(new_count);

    if max_lines == 0 {
        return;
    }

    let mut seq = String::new();
    for i in 0..max_lines {
        seq.push_str("\n\x1b[K");
        if let Some(choice) = choices.get(i) {
            seq.push('\r');
            seq.push_str(&format_preview_row(
                choice.value,
                partial_len,
                choice.description,
            ));
        }
    }
    let _ = write!(seq, "\x1b[{max_lines}A\r");

    PREVIEW_LINE_COUNT.store(new_count, Ordering::Relaxed);
    print!("{seq}");
    let _ = io::stdout().flush();
}

/// Print blank lines below the current cursor to guarantee preview space.
pub fn reserve_preview_space() {
    let n = slash_commands::COMMANDS.len();
    let mut seq = String::new();
    for _ in 0..n {
        seq.push('\n');
    }
    let _ = write!(seq, "\x1b[{n}A");
    print!("{seq}");
    let _ = io::stdout().flush();
}

pub struct NlshHelper;

impl Helper for NlshHelper {}

impl Completer for NlshHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        _pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        if !line.starts_with('/') {
            return Ok((0, vec![]));
        }
        if line.contains(' ') {
            if let Some((start, choices)) = slash_commands::arg_completions(line) {
                let candidates = choices
                    .iter()
                    .map(|c| Pair {
                        display: c.value.to_string(),
                        replacement: format!("{} ", c.value),
                    })
                    .collect();
                return Ok((start, candidates));
            }
            return Ok((0, vec![]));
        }
        let matches = slash_commands::filter(line);
        let candidates: Vec<Pair> = matches
            .iter()
            .map(|cmd| {
                let name = format!("/{}", cmd.name);
                Pair {
                    display: name.clone(),
                    replacement: format!("{name} "),
                }
            })
            .collect();
        Ok((0, candidates))
    }
}

impl Hinter for NlshHelper {
    type Hint = String;
}

impl Validator for NlshHelper {}

impl Highlighter for NlshHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        if !SHELL_MODE.load(Ordering::Relaxed) {
            return Cow::Borrowed(prompt);
        }
        let cwd = current_directory_display();
        Cow::Owned(format!(
            "{}:{}{} ",
            "larpshell".custom_color(CTP_BLUE).bold(),
            cwd.custom_color(CTP_OVERLAY0),
            "$".custom_color(CTP_PRIMARY).bold()
        ))
    }

    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        if SHELL_MODE.load(Ordering::Relaxed) {
            clear_slash_preview();
            // "! " stays in buffer for history/execution; dim it so it reads as a
            // prompt-side indicator, then color the command in orange.
            if let Some(cmd) = line.strip_prefix("! ") {
                return Cow::Owned(format!("{}{}", "! ".custom_color(CTP_OVERLAY0), cmd));
            }
            return Cow::Owned(line.custom_color(CTP_PRIMARY).to_string());
        }
        if !line.starts_with('/') {
            clear_slash_preview();
            return Cow::Borrowed(line);
        }
        draw_slash_preview(line);
        if let Some((cmd, args)) = line.split_once(' ') {
            Cow::Owned(format!("{} {}", cmd.custom_color(CTP_BLUE), args))
        } else {
            Cow::Owned(line.custom_color(CTP_BLUE).to_string())
        }
    }

    fn highlight_char(&self, line: &str, _pos: usize, _kind: CmdKind) -> bool {
        // Derive shell mode from buffer content so history restore works automatically.
        let shell = line.starts_with("! ");
        SHELL_MODE.store(shell, Ordering::Relaxed);
        shell || line.starts_with('/')
    }
}

struct SlashPreviewHandler;

impl ConditionalEventHandler for SlashPreviewHandler {
    fn handle(
        &self,
        evt: &Event,
        _n: RepeatCount,
        _positive: bool,
        ctx: &EventContext<'_>,
    ) -> Option<Cmd> {
        let line = ctx.line();
        let pos = ctx.pos();

        // Suppress slash preview in shell mode.
        if line.starts_with("! ") {
            clear_slash_preview();
            return None;
        }

        // Compute what the line will look like after this keypress,
        // so we can clear preview early when switching away from /commands.
        let effective = match evt {
            Event::KeySeq(keys) => match keys.first() {
                Some(KeyEvent(KeyCode::Char(c), Modifiers::NONE)) => {
                    let mut s = line.to_string();
                    s.insert(pos, *c);
                    s
                }
                Some(KeyEvent(KeyCode::Backspace, _)) if pos > 0 => {
                    let char_start = line[..pos].char_indices().next_back().map_or(0, |(i, _)| i);
                    let mut s = line.to_string();
                    s.replace_range(char_start..pos, "");
                    s
                }
                _ => line.to_string(),
            },
            _ => line.to_string(),
        };

        // If the line will no longer start with '/', clear preview now
        // (highlight won't be called for non-slash lines).
        if !effective.starts_with('/') {
            clear_slash_preview();
        }

        None
    }
}

type NlshEditor = Editor<NlshHelper, DefaultHistory>;

static EDITOR: Mutex<Option<NlshEditor>> = Mutex::new(None);

fn with_editor<F>(readline_fn: F) -> Result<Option<String>, io::Error>
where
    F: FnOnce(&mut NlshEditor, &str) -> rustyline::Result<String>,
{
    SHELL_MODE.store(false, Ordering::Relaxed);
    let mut editor_lock = EDITOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if editor_lock.is_none() {
        let mut editor = Editor::<NlshHelper, DefaultHistory>::with_config(
            Config::builder()
                .completion_type(CompletionType::Circular)
                .build(),
        )
        .map_err(io::Error::other)?;
        editor.set_helper(Some(NlshHelper));
        editor.bind_sequence(
            Event::Any,
            EventHandler::Conditional(Box::new(SlashPreviewHandler)),
        );
        if config::history_enabled()
            && let Ok(path) = config::history_path()
        {
            let _ = editor.load_history(&path);
        }
        *editor_lock = Some(editor);
    }
    let Some(editor) = editor_lock.as_mut() else {
        return Err(io::Error::other("failed to initialize rustyline editor"));
    };
    let cwd = current_directory_display();
    let prompt = format!(
        "{}:{}{} ",
        "larpshell".custom_color(CTP_BLUE).bold(),
        cwd.custom_color(CTP_OVERLAY0),
        "❯".custom_color(CTP_BLUE)
    );
    match readline_fn(editor, &prompt) {
        Ok(line) => {
            clear_slash_preview();
            let trimmed = line.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                let _ = editor.add_history_entry(&line);
                if config::history_enabled()
                    && let Ok(path) = config::history_path()
                {
                    let _ = editor.save_history(&path);
                }
                Ok(Some(trimmed.to_string()))
            }
        }
        Err(ReadlineError::Interrupted) => {
            clear_slash_preview();
            show_cursor();
            Err(io::Error::from(io::ErrorKind::Interrupted))
        }
        Err(ReadlineError::Eof) => {
            clear_slash_preview();
            show_cursor();
            Err(io::Error::from(io::ErrorKind::UnexpectedEof))
        }
        Err(err) => {
            clear_slash_preview();
            Err(io::Error::other(err))
        }
    }
}

pub fn user_input_prefilled(initial: &str) -> Result<Option<String>, io::Error> {
    with_editor(|editor, prompt| editor.readline_with_initial(prompt, (initial, "")))
}

pub fn user_input() -> Result<Option<String>, io::Error> {
    with_editor(rustyline::Editor::readline)
}
