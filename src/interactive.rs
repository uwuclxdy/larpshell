use std::borrow::Cow;
use std::io;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use colored::*;
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

use crate::common::{
    CTP_BLUE, CTP_OVERLAY0, CTP_PRIMARY, EXIT_SIGINT, exit_with_code, get_current_directory,
    show_cursor,
};
use crate::slash_commands;

// Number of preview lines currently drawn below the prompt.
static PREVIEW_LINE_COUNT: AtomicUsize = AtomicUsize::new(0);

// Longest command name length (used for column alignment).
// "uninstall" = 9 chars. Column = 2 (indent) + 1 (/) + 9 (name) + 4 (gap) = 16
const PREVIEW_DESC_COL: usize = 16;

/// Formats one preview row with ANSI coloring.
/// `cmd_name` includes the leading `/`.
/// `typed_len` is how many chars of `cmd_name` the user has already typed.
fn format_preview_row(cmd_name: &str, typed_len: usize, description: &str) -> String {
    let typed = &cmd_name[..typed_len.min(cmd_name.len())];
    let untyped = &cmd_name[typed_len.min(cmd_name.len())..];
    let name_display_len = 1 + cmd_name.len(); // "  " + "/" + name
    let pad = PREVIEW_DESC_COL.saturating_sub(name_display_len + 2);
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
    seq.push_str(&format!("\x1b[{}A\r", n));
    eprint!("{seq}");
    let _ = io::stderr().flush();
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
    seq.push_str(&format!("\x1b[{}A\r", max_lines));

    PREVIEW_LINE_COUNT.store(new_count, Ordering::Relaxed);
    eprint!("{seq}");
    let _ = io::stderr().flush();
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
    seq.push_str(&format!("\x1b[{}A\r", max_lines));

    PREVIEW_LINE_COUNT.store(new_count, Ordering::Relaxed);
    eprint!("{seq}");
    let _ = io::stderr().flush();
}

/// Print blank lines below the current cursor to guarantee preview space.
pub fn reserve_preview_space() {
    let n = slash_commands::COMMANDS.len();
    let mut seq = String::new();
    for _ in 0..n {
        seq.push('\n');
    }
    seq.push_str(&format!("\x1b[{}A", n));
    eprint!("{seq}");
    let _ = io::stderr().flush();
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
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        if !line.starts_with('/') {
            // Clear any stale preview when user switches away from /commands.
            clear_slash_preview();
            return Cow::Borrowed(line);
        }
        // Draw preview after rustyline redraws the prompt line.
        draw_slash_preview(line);
        Cow::Owned(line.custom_color(CTP_BLUE).to_string())
    }

    fn highlight_char(&self, line: &str, _pos: usize, _kind: CmdKind) -> bool {
        line.starts_with('/')
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
                    let char_start = line[..pos]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
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
    let mut editor_lock = EDITOR.lock().unwrap_or_else(|e| e.into_inner());
    let editor = editor_lock.get_or_insert_with(|| {
        let mut ed = Editor::<NlshHelper, DefaultHistory>::with_config(
            Config::builder()
                .completion_type(CompletionType::Circular)
                .build(),
        )
        .unwrap();
        ed.set_helper(Some(NlshHelper));
        ed.bind_sequence(
            Event::Any,
            EventHandler::Conditional(Box::new(SlashPreviewHandler)),
        );
        ed
    });
    let cwd = get_current_directory();
    let prompt = format!(
        "{}:{}{} ",
        "larpshell".custom_color(CTP_BLUE).bold(),
        cwd.custom_color(CTP_OVERLAY0).bold(),
        ">".bold()
    );
    match readline_fn(editor, &prompt) {
        Ok(line) => {
            clear_slash_preview();
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let _ = editor.add_history_entry(&line);
                Ok(Some(trimmed.to_string()))
            } else {
                Ok(None)
            }
        }
        Err(ReadlineError::Interrupted) => {
            clear_slash_preview();
            show_cursor();
            exit_with_code(EXIT_SIGINT);
        }
        Err(ReadlineError::Eof) => {
            clear_slash_preview();
            show_cursor();
            exit_with_code(0);
        }
        Err(err) => {
            clear_slash_preview();
            Err(io::Error::other(err))
        }
    }
}

pub fn get_user_input_prefilled(initial: &str) -> Result<Option<String>, io::Error> {
    with_editor(|editor, prompt| editor.readline_with_initial(prompt, (initial, "")))
}

pub fn get_user_input() -> Result<Option<String>, io::Error> {
    with_editor(|editor, prompt| editor.readline(prompt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustyline::highlight::{CmdKind, Highlighter};

    #[test]
    fn highlight_slash_prefix_colors_typed_part() {
        colored::control::set_override(true);
        let helper = NlshHelper;
        let result = helper.highlight("/pr", 3);
        assert!(result.contains("\x1b["), "expected ANSI codes in: {result}");
        assert!(result.contains("/pr"), "typed part must appear in output");
    }

    #[test]
    fn highlight_non_slash_line_is_unchanged() {
        let helper = NlshHelper;
        let result = helper.highlight("list files", 10);
        assert_eq!(result.as_ref(), "list files");
    }

    #[test]
    fn highlight_char_true_for_slash_line() {
        let helper = NlshHelper;
        assert!(helper.highlight_char("/api", 4, CmdKind::Other));
    }

    #[test]
    fn highlight_char_false_for_normal_line() {
        let helper = NlshHelper;
        assert!(!helper.highlight_char("list files", 10, CmdKind::Other));
    }

    #[test]
    fn format_preview_row_pads_to_column() {
        colored::control::set_override(false);
        let row = format_preview_row_plain("/api", 0, "configure API provider");
        assert!(row.contains("configure API provider"), "row: {row}");
        let row2 = format_preview_row_plain("/uninstall", 0, "uninstall larpshell");
        let desc_pos1 = row.find("configure").unwrap();
        let desc_pos2 = row2.find("uninstall larpshell").unwrap();
        assert_eq!(desc_pos1, desc_pos2, "descriptions must align");
    }

    fn format_preview_row_plain(cmd_name: &str, typed_len: usize, description: &str) -> String {
        colored::control::set_override(false);
        let r = format_preview_row(cmd_name, typed_len, description);
        strip_ansi_escapes::strip_str(&r)
    }
}
