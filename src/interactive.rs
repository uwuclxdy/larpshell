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
    Cmd, CompletionType, ConditionalEventHandler, Config, Editor, Event, EventContext, EventHandler, Helper, KeyCode, KeyEvent, Modifiers,
    RepeatCount,
};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::common::{CTP_BLUE, CTP_OVERLAY0, CTP_PRIMARY, CTP_TEXT, current_directory_display, show_cursor, terminal_width};
use crate::config;
use crate::slash_commands;

static PREVIEW_LINE_COUNT: AtomicUsize = AtomicUsize::new(0);
static SHELL_MODE: AtomicBool = AtomicBool::new(false);

// Longest command name length (used for column alignment).
// "uninstall" = 9 chars. Column = 2 (indent) + 1 (/) + 9 (name) + 4 (gap) = 16
const PREVIEW_DESC_COL: usize = 16;

pub fn format_preview_row(cmd_name: &str, typed_len: usize, description: &str, max_width: usize, selected: bool) -> String {
    // Indent is always 2 display columns, whether or not a row is selected.
    const INDENT_COLS: usize = 2;
    let split = typed_len.min(cmd_name.len());
    let (typed_raw, untyped_raw) = cmd_name.split_at(split);
    let pad = PREVIEW_DESC_COL.saturating_sub(cmd_name.width() + 3);
    let gap = pad + 4;

    // Clamp the indent+name+gap prefix to max_width before sizing the
    // description: on a narrow terminal the prefix alone can exceed the row
    // budget, and an unclamped prefix would wrap the row onto a second
    // terminal line no matter how far the description gets truncated,
    // desyncing the preview's line count. Each half is truncated on its own
    // budget (rather than truncating the combined name and re-splitting it),
    // so a cut never lands inside the other half's own truncation ellipsis.
    let name_budget = max_width.saturating_sub(INDENT_COLS);
    let typed = truncate_to_width(typed_raw, name_budget);
    let typed_cols = typed.width();
    let untyped = truncate_to_width(untyped_raw, name_budget.saturating_sub(typed_cols));
    let name_cols = typed_cols + untyped.width();
    let gap = gap.min(max_width.saturating_sub(INDENT_COLS + name_cols));
    let prefix_cols = INDENT_COLS + name_cols + gap;
    // Truncate the description so the row never exceeds one terminal row; a
    // wrapped row would throw off the line count used to erase the preview.
    let description = truncate_to_width(description, max_width.saturating_sub(prefix_cols));

    // The selected row gets a marker and brighter text; both indents are 2 cols
    // wide so column alignment is identical either way.
    let (indent, untyped_color, desc_color) = if selected {
        ("❯ ".custom_color(CTP_BLUE).to_string(), CTP_TEXT, CTP_TEXT)
    } else {
        ("  ".to_string(), CTP_OVERLAY0, CTP_OVERLAY0)
    };
    format!(
        "{}{}{}{}{}",
        indent,
        typed.as_ref().custom_color(CTP_BLUE).bold(),
        untyped.as_ref().custom_color(untyped_color),
        " ".repeat(gap),
        description.as_ref().custom_color(desc_color),
    )
}

/// Truncates `text` to at most `max` display columns, appending `…` when cut.
fn truncate_to_width(text: &str, max: usize) -> Cow<'_, str> {
    if max == 0 {
        return Cow::Borrowed("");
    }
    if text.width() <= max {
        return Cow::Borrowed(text);
    }
    let budget = max.saturating_sub(1); // leave a column for the ellipsis; 0 when max == 1
    let mut out = String::new();
    let mut cols = 0usize;
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cols + w > budget {
            break;
        }
        out.push(ch);
        cols += w;
    }
    out.push('…');
    Cow::Owned(out)
}

/// Builds the escape sequence that erases `n` preview lines below the prompt
/// and returns the cursor to it. Column 0 must be reached before each erase:
/// erasing from wherever the cursor already sits only clears to end-of-line,
/// leaving the left half of the row on screen.
pub(crate) fn clear_preview_sequence(n: usize) -> String {
    let mut seq = String::new();
    for _ in 0..n {
        seq.push_str("\n\r\x1b[K");
    }
    let _ = write!(seq, "\x1b[{n}A\r");
    seq
}

/// Erase all currently-drawn preview lines below the prompt.
/// Must be called while the cursor is on the prompt line.
pub fn clear_slash_preview() {
    let n = PREVIEW_LINE_COUNT.swap(0, Ordering::Relaxed);
    if n == 0 {
        return;
    }
    print!("{}", clear_preview_sequence(n));
    let _ = io::stdout().flush();
}

/// One previewed completion row plus the full line that selecting it yields.
pub(crate) struct PreviewItem {
    /// Row label: `/command` for command names, the bare value for arguments.
    pub(crate) name: String,
    description: &'static str,
    /// Leading chars of `name` the user has already typed (rendered bold).
    typed_len: usize,
    /// Buffer contents if this item is selected.
    pub(crate) replacement: String,
}

/// Build the completion rows for `source` (command names or argument values).
/// Returns empty when there is nothing to preview.
pub(crate) fn preview_items(source: &str) -> Vec<PreviewItem> {
    if !source.starts_with('/') {
        return Vec::new();
    }
    if source.contains(' ') {
        let Some((start, choices)) = slash_commands::arg_completions(source) else {
            return Vec::new();
        };
        let typed_len = source.len() - start;
        let prefix = &source[..start];
        return choices
            .iter()
            .map(|c| PreviewItem {
                name: c.value.to_string(),
                description: c.description,
                typed_len,
                replacement: format!("{prefix}{}", c.value),
            })
            .collect();
    }
    slash_commands::filter(source)
        .iter()
        .map(|cmd| {
            let name = format!("/{}", cmd.name);
            PreviewItem { description: cmd.description, typed_len: source.len(), replacement: name.clone(), name }
        })
        .collect()
}

/// Builds the escape sequence for a from-scratch preview redraw: `max_lines`
/// rows are each erased (returning to column 0 first, for the same reason as
/// `clear_preview_sequence`) and, for rows with an item, redrawn.
pub(crate) fn render_preview_sequence(items: &[PreviewItem], selected: Option<usize>, max_lines: usize, width: usize) -> String {
    let mut seq = String::new();
    for i in 0..max_lines {
        seq.push_str("\n\r\x1b[K");
        if let Some(item) = items.get(i) {
            seq.push_str(&format_preview_row(&item.name, item.typed_len, item.description, width, selected == Some(i)));
        }
    }
    // Return cursor to the prompt line.
    let _ = write!(seq, "\x1b[{max_lines}A\r");
    seq
}

/// Redraw the preview from scratch: erase old lines, write `items` (marking
/// `selected`), and return the cursor to the prompt line.
fn render_preview(items: &[PreviewItem], selected: Option<usize>) {
    let prev_count = PREVIEW_LINE_COUNT.load(Ordering::Relaxed);
    let new_count = items.len();
    let max_lines = prev_count.max(new_count);
    if max_lines == 0 {
        return;
    }

    let width = terminal_width();
    let seq = render_preview_sequence(items, selected, max_lines, width);

    PREVIEW_LINE_COUNT.store(new_count, Ordering::Relaxed);
    print!("{seq}");
    let _ = io::stdout().flush();
}

/// Draw a filtered command or argument preview below the current prompt line.
/// While cycling, the selected row is marked; the candidate set tracks the
/// typed line (which doesn't change during cycling).
pub fn draw_slash_preview(line: &str) {
    let (source, selected) = match cycle_lock().as_ref() {
        Some(c) => (c.base.clone(), Some(c.index)),
        None => (line.to_string(), None),
    };
    render_preview(&preview_items(&source), selected);
}

/// Selection state while the arrows / tab cycle the slash preview.
///
/// The buffer itself is never mutated while cycling. Filling it would need
/// `Cmd::Replace`, whose `edit_insert_text` path inserts without advancing the
/// cursor, dropping it to column 0. Instead the selection shows as a ghost hint
/// after the cursor and is committed on submit, so `base` equals the live buffer.
struct CycleState {
    /// The typed line that defines the candidate set (equals the live buffer).
    base: String,
    index: usize,
}

static CYCLE: Mutex<Option<CycleState>> = Mutex::new(None);

fn cycle_lock() -> std::sync::MutexGuard<'static, Option<CycleState>> {
    CYCLE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn reset_cycle() {
    *cycle_lock() = None;
}

/// Next selection index when cycling `count` items. With no current selection,
/// forward starts at the top and backward at the bottom; otherwise it steps and
/// wraps around either end.
pub(crate) fn next_cycle_index(current: Option<usize>, count: usize, forward: bool) -> usize {
    debug_assert!(count > 0);
    match current {
        Some(i) => (i as i64 + if forward { 1 } else { -1 }).rem_euclid(count as i64) as usize,
        None if forward => 0,
        None => count - 1,
    }
}

/// Advance the slash-preview selection by one and repaint so the ghost hint and
/// highlighted row update, without touching the buffer (which keeps the cursor
/// at end). The picked command is committed on submit via `selected_replacement`.
/// Returns `None` when `line` isn't a slash command or has no matches, so the key
/// falls through to its default binding (history nav for the arrows, native
/// completion for tab — a no-op here since the completer is empty off a match).
pub(crate) fn cycle_slash_preview(line: &str, forward: bool) -> Option<Cmd> {
    if !line.starts_with('/') {
        reset_cycle();
        return None;
    }
    let mut guard = cycle_lock();
    // The buffer is left untouched while cycling, so the candidate set is the
    // live line itself.
    let items = preview_items(line);
    if items.is_empty() {
        *guard = None;
        return None;
    }
    let index = next_cycle_index(guard.as_ref().map(|c| c.index), items.len(), forward);
    *guard = Some(CycleState { base: line.to_string(), index });
    Some(Cmd::Repaint)
}

/// The full line the current cycle selection commits to, if any.
fn selected_replacement() -> Option<String> {
    let guard = cycle_lock();
    let c = guard.as_ref()?;
    preview_items(&c.base).into_iter().nth(c.index).map(|i| i.replacement)
}

/// Ghost suffix to show after the cursor: the selected completion with the
/// typed `line` stripped off. `None` when nothing is selected or already typed
/// in full.
pub(crate) fn selection_ghost(line: &str, base: &str, index: usize) -> Option<String> {
    let item = preview_items(base).into_iter().nth(index)?;
    let ghost = item.replacement.strip_prefix(line)?;
    (!ghost.is_empty()).then(|| ghost.to_string())
}

pub struct NlshHelper;

impl Helper for NlshHelper {}

impl Completer for NlshHelper {
    type Candidate = Pair;

    fn complete(&self, line: &str, _pos: usize, _ctx: &rustyline::Context<'_>) -> rustyline::Result<(usize, Vec<Pair>)> {
        if !line.starts_with('/') {
            return Ok((0, vec![]));
        }
        if line.contains(' ') {
            if let Some((start, choices)) = slash_commands::arg_completions(line) {
                let candidates =
                    choices.iter().map(|c| Pair { display: c.value.to_string(), replacement: format!("{} ", c.value) }).collect();
                return Ok((start, candidates));
            }
            return Ok((0, vec![]));
        }
        let matches = slash_commands::filter(line);
        let candidates: Vec<Pair> = matches
            .iter()
            .map(|cmd| {
                let name = format!("/{}", cmd.name);
                Pair { display: name.clone(), replacement: format!("{name} ") }
            })
            .collect();
        Ok((0, candidates))
    }
}

impl Hinter for NlshHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<String> {
        // Hints only render at end of line; the cursor stays there while cycling.
        if pos != line.len() {
            return None;
        }
        let guard = cycle_lock();
        let c = guard.as_ref()?;
        selection_ghost(line, &c.base, c.index)
    }
}

impl Validator for NlshHelper {}

impl Highlighter for NlshHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(&'s self, prompt: &'p str, _default: bool) -> Cow<'b, str> {
        if !SHELL_MODE.load(Ordering::Relaxed) {
            return Cow::Borrowed(prompt);
        }
        let cwd = current_directory_display();
        Cow::Owned(format!(
            "{}:{}{} ",
            "larpshell".custom_color(CTP_PRIMARY),
            cwd.custom_color(CTP_OVERLAY0),
            "$".custom_color(CTP_BLUE).bold()
        ))
    }

    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        if SHELL_MODE.load(Ordering::Relaxed) {
            clear_slash_preview();
            // "! " stays in buffer for history/execution; dim it so it reads as a
            // prompt-side indicator, then color the command in the sapphire accent.
            if let Some(cmd) = line.strip_prefix("! ") {
                return Cow::Owned(format!("{}{}", "! ".custom_color(CTP_OVERLAY0), cmd));
            }
            return Cow::Owned(line.custom_color(CTP_BLUE).to_string());
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

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(hint.custom_color(CTP_OVERLAY0).to_string())
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
    fn handle(&self, evt: &Event, _n: RepeatCount, _positive: bool, ctx: &EventContext<'_>) -> Option<Cmd> {
        let line = ctx.line();
        let pos = ctx.pos();

        // Suppress slash preview in shell mode.
        if line.starts_with("! ") {
            reset_cycle();
            clear_slash_preview();
            return None;
        }

        // Arrow keys and tab/shift+tab cycle the slash preview and ghost the
        // selection; outside slash mode they fall through to history navigation.
        // Enter keeps the cycle state so the selection is committed on submit.
        //
        // Tab drives the same cycle instead of rustyline's built-in completion.
        // The native completer runs a nested input loop of its own; on some
        // terminals the keypress that ends it (Enter) is buffered there and
        // replayed onto the next one, so the first Enter after tab-complete does
        // nothing and both land on the second. Routing tab through the one
        // preview cycle keeps a single input loop and commits on one Enter. On a
        // non-slash line `cycle_slash_preview` returns `None`, so tab still falls
        // through to the default binding there.
        if let Event::KeySeq(keys) = evt {
            match keys.first() {
                Some(KeyEvent(KeyCode::Down | KeyCode::Tab, Modifiers::NONE)) => {
                    if let Some(cmd) = cycle_slash_preview(line, true) {
                        return Some(cmd);
                    }
                }
                Some(KeyEvent(KeyCode::Up, Modifiers::NONE)) | Some(KeyEvent(KeyCode::BackTab, _)) => {
                    if let Some(cmd) = cycle_slash_preview(line, false) {
                        return Some(cmd);
                    }
                }
                Some(KeyEvent(KeyCode::Enter, _)) => return None,
                _ => {}
            }
        }

        // Any other key ends an active cycle so editing resumes from the buffer.
        reset_cycle();

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
    reset_cycle();
    let mut editor_lock = EDITOR.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if editor_lock.is_none() {
        let mut editor =
            Editor::<NlshHelper, DefaultHistory>::with_config(Config::builder().completion_type(CompletionType::Circular).build())
                .map_err(io::Error::other)?;
        editor.set_helper(Some(NlshHelper));
        editor.bind_sequence(Event::Any, EventHandler::Conditional(Box::new(SlashPreviewHandler)));
        // Load unconditionally: a session that starts with history-saving
        // disabled must still see prior entries, otherwise flipping it on
        // mid-session (`/history on`) and saving would overwrite the file
        // with only this session's entries, wiping everything earlier.
        // Saving stays gated on the toggle (below).
        if let Ok(path) = config::history_path() {
            let _ = editor.load_history(&path);
        }
        *editor_lock = Some(editor);
    }
    let Some(editor) = editor_lock.as_mut() else {
        return Err(io::Error::other("failed to initialize rustyline editor"));
    };
    let cwd = current_directory_display();
    let prompt = format!("{}:{}{} ", "larpshell".custom_color(CTP_PRIMARY), cwd.custom_color(CTP_OVERLAY0), "❯".custom_color(CTP_BLUE));
    match readline_fn(editor, &prompt) {
        Ok(line) => {
            clear_slash_preview();
            // A live cycle selection commits its full command, not the typed stem.
            let line = selected_replacement().unwrap_or(line);
            reset_cycle();
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
