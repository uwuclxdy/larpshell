use crate::interactive::{
    NlshHelper, clear_preview_sequence, cycle_slash_preview, format_preview_row, next_cycle_index, preview_items, render_preview_sequence,
    selection_ghost,
};
use crate::tests;
use rustyline::Cmd;
use rustyline::completion::Completer;
use rustyline::highlight::{CmdKind, Highlighter};
use unicode_width::UnicodeWidthStr;

fn format_preview_row_plain(cmd_name: &str, typed_len: usize, description: &str) -> String {
    colored::control::set_override(false);
    let r = format_preview_row(cmd_name, typed_len, description, usize::MAX, false);
    strip_ansi_escapes::strip_str(&r)
}

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

#[test]
fn format_preview_row_truncates_to_width() {
    // Strips ANSI below, so it stays agnostic to the process-global color
    // override other tests may have toggled.
    let width = 24;
    let row = format_preview_row("/uninstall", 0, "uninstall larpshell completely", width, false);
    let plain = strip_ansi_escapes::strip_str(&row);
    assert!(plain.chars().count() <= width, "row too wide: {plain:?}");
    assert!(plain.ends_with('…'), "expected ellipsis: {plain:?}");
}

#[test]
fn selected_row_keeps_description_alignment() {
    // The selection marker must stay 2 columns wide so the description column
    // lines up whether or not a row is highlighted. Strip ANSI (override-
    // agnostic) and measure display columns, since `❯` is 3 bytes but 1 column.
    let desc_col = |sel| {
        let row = strip_ansi_escapes::strip_str(format_preview_row("/api", 0, "configure", usize::MAX, sel));
        let prefix = &row[..row.find("configure").unwrap()];
        (UnicodeWidthStr::width(prefix), row)
    };
    let (unselected_col, _) = desc_col(false);
    let (selected_col, selected) = desc_col(true);
    assert_eq!(unselected_col, selected_col, "selected row must not shift the description column");
    assert!(selected.starts_with("❯ "), "selected row needs a marker: {selected:?}");
}

#[test]
fn next_cycle_index_wraps_both_ways() {
    // First arrow: forward picks the top, backward the bottom.
    assert_eq!(next_cycle_index(None, 3, true), 0);
    assert_eq!(next_cycle_index(None, 3, false), 2);
    // Stepping wraps at each end.
    assert_eq!(next_cycle_index(Some(2), 3, true), 0);
    assert_eq!(next_cycle_index(Some(0), 3, false), 2);
    assert_eq!(next_cycle_index(Some(1), 3, true), 2);
}

#[test]
fn preview_items_prefill_command_names() {
    let items = preview_items("/");
    assert!(!items.is_empty(), "slash alone lists every command");
    // Selecting a command prefills the full `/name` into the buffer.
    assert!(items.iter().all(|i| i.replacement == i.name && i.name.starts_with('/')));
    assert!(items.iter().any(|i| i.replacement == "/agent"));
}

#[test]
fn preview_items_prefill_arguments_preserve_prefix() {
    let items = preview_items("/agent ");
    assert!(!items.is_empty(), "agent takes a mode argument");
    // The argument value is appended after the command, not replacing it.
    assert!(items.iter().all(|i| i.replacement.starts_with("/agent ")));
    assert!(items.iter().any(|i| i.replacement == "/agent off"));
}

#[test]
fn preview_items_empty_for_non_slash() {
    assert!(preview_items("list files").is_empty());
}

#[test]
fn selection_ghost_is_untyped_suffix() {
    // base "/" lists all commands; index 1 is `/provider` (after `/api`).
    assert_eq!(selection_ghost("/", "/", 1).as_deref(), Some("provider"));
    // The typed prefix is stripped, leaving only the part to suggest.
    assert_eq!(selection_ghost("/ag", "/ag", 0).as_deref(), Some("ent"));
    // No ghost once the selection is fully typed.
    assert_eq!(selection_ghost("/agent", "/agent", 0), None);
    // Argument selections ghost the value tail too.
    assert_eq!(selection_ghost("/agent ", "/agent ", 0).as_deref(), Some("off"));
}

// ── history load/save ─────────────────────────────────────────────────────

#[test]
fn history_survives_enabling_mid_session_after_starting_disabled() {
    // Regression: history load used to be skipped whenever history-saving
    // was disabled at session start, so flipping it on mid-session
    // (`/history on`) and submitting a line saved the in-memory history —
    // missing every prior entry — straight over the on-disk file, wiping
    // everything a previous session had written.
    let home = tests::temp_home("interactive_history_reload");
    let port = tests::mock_ollama(&[]);
    tests::write_ollama_config(&home, port);

    let config_dir = home.join("config").join("larpshell");
    std::fs::write(config_dir.join(".history-disabled"), "").unwrap();
    std::fs::write(config_dir.join(".history"), "prior session command\n").unwrap();

    let out = tests::run_with_stdin_interactive(&home, &[], b"/history on\n/quit\n");
    assert!(out.status.success(), "REPL session should exit cleanly; stderr: {}", String::from_utf8_lossy(&out.stderr));

    let contents = std::fs::read_to_string(config_dir.join(".history")).unwrap();
    assert!(contents.contains("prior session command"), "enabling history mid-session must not wipe prior entries: {contents:?}");
}

// ── preview clear/redraw escape sequences ───────────────────────────────────

#[test]
fn clear_preview_sequence_returns_to_column_zero_before_each_erase() {
    // Regression: erasing without a leading `\r` only clears from wherever
    // the cursor already sits to end-of-line, leaving the left half of a
    // removed row on screen.
    let seq = clear_preview_sequence(3);
    assert_eq!(seq.matches("\n\r\x1b[K").count(), 3, "each erased line must return to column 0 first: {seq:?}");
    assert!(!seq.contains("\n\x1b[K"), "no erase may happen without a preceding \\r: {seq:?}");
}

#[test]
fn render_preview_sequence_returns_to_column_zero_before_each_erase() {
    let items = preview_items("/");
    assert!(!items.is_empty());
    // A previous frame had more rows than this one (narrowing the candidate
    // list), which exercises the erase-only rows too.
    let max_lines = items.len() + 2;
    let seq = render_preview_sequence(&items, None, max_lines, 80);
    assert_eq!(
        seq.matches("\n\r\x1b[K").count(),
        max_lines,
        "every row, with or without new content, must return to column 0 before erasing: {seq:?}"
    );
}

// ── argument completion offset ──────────────────────────────────────────────

#[test]
fn preview_items_handles_trailing_non_ascii_whitespace_without_panicking() {
    // Regression: only a trailing ASCII space was treated as "argument
    // complete"; a trailing tab/NBSP desynced the byte offset used to slice
    // the line, which could panic on a non-char-boundary slice.
    assert!(preview_items("/agent on\t").is_empty());
    assert!(preview_items("/agent on\u{a0}").is_empty());
    assert!(preview_items("/agent on ").is_empty());
}

#[test]
fn preview_items_handles_partial_before_trailing_multibyte_whitespace_without_panicking() {
    // The exact panic case: a real partial ("s", a prefix of "safe" so the
    // candidate list is non-empty) followed by a multi-byte trailing whitespace
    // that split_whitespace strips but `ends_with(' ')` missed. The old
    // `line.len() - partial.len()` offset then overshot into the NBSP/EM-SPACE
    // bytes and panicked when the caller sliced the line.
    assert!(preview_items("/agent s\u{a0}").is_empty());
    assert!(preview_items("/agent s\u{2003}").is_empty());
}

#[test]
fn completer_complete_handles_trailing_non_ascii_whitespace_without_panicking() {
    let helper = NlshHelper;
    let history = rustyline::history::DefaultHistory::new();
    let ctx = rustyline::Context::new(&history);
    let line = "/agent on\u{a0}";
    let result = helper.complete(line, line.len(), &ctx);
    let (start, candidates) = result.expect("must not panic or error");
    assert_eq!(start, 0);
    assert!(candidates.is_empty());
}

// ── narrow-terminal preview row ─────────────────────────────────────────────

#[test]
fn format_preview_row_description_ellipsis_fits_a_single_free_column() {
    // Regression: the description truncation forced at least 1 real
    // character before the ellipsis even when only 1 column was free,
    // overflowing the row by a column.
    let max_width = 20; // leaves exactly 1 free column for "/api"'s description
    let row = format_preview_row("/api", 0, "configure API provider", max_width, false);
    let plain = strip_ansi_escapes::strip_str(&row);
    assert!(UnicodeWidthStr::width(plain.as_str()) <= max_width, "row must not exceed the terminal width: {plain:?}");
    assert!(plain.ends_with('…'), "expected ellipsis: {plain:?}");
}

#[test]
fn format_preview_row_clamps_prefix_on_narrow_terminal() {
    // Regression: the fixed indent+name+gap prefix was never clamped, so on
    // a narrow terminal it alone could exceed max_width and wrap the row
    // onto a second terminal line no matter how far the description was cut.
    let max_width = 5;
    let row = format_preview_row("/uninstall", 0, "uninstall larpshell", max_width, false);
    let plain = strip_ansi_escapes::strip_str(&row);
    assert!(UnicodeWidthStr::width(plain.as_str()) <= max_width, "row must fit in {max_width} columns: {plain:?}");
}

#[test]
fn format_preview_row_column_padding_uses_display_width_not_byte_length() {
    // Regression: the padding was computed from the command name's byte
    // length while every other width computation here uses display width,
    // so a name with a multi-byte-but-narrow character misaligned its
    // description column relative to an ASCII name of the same width.
    colored::control::set_override(false);
    // "café" is 5 bytes but 4 display columns, same as "abcd".
    let ascii = strip_ansi_escapes::strip_str(format_preview_row("abcd", 0, "same width as café", usize::MAX, false));
    let multibyte = strip_ansi_escapes::strip_str(format_preview_row("café", 0, "same width as abcd", usize::MAX, false));
    let ascii_desc_col = UnicodeWidthStr::width(&ascii[..ascii.find("same").unwrap()]);
    let multibyte_desc_col = UnicodeWidthStr::width(&multibyte[..multibyte.find("same").unwrap()]);
    assert_eq!(ascii_desc_col, multibyte_desc_col, "description column must align by display width, not byte length");
}

// ── cycle keeps the cursor at line end ──────────────────────────────────────

#[test]
fn cycle_slash_preview_repaints_without_touching_the_buffer() {
    // Regression: filling the buffer with the picked command (via `Cmd::Replace`)
    // dropped the cursor to column 0, because rustyline's insert-text path never
    // advances it. Cycling must only repaint the ghost + dropdown so the cursor
    // stays where the user typed. A non-slash call first clears any leftover
    // cycle state from other tests sharing the global.
    assert_eq!(cycle_slash_preview("find files", true), None);
    assert_eq!(
        cycle_slash_preview("/", true),
        Some(Cmd::Repaint),
        "cycling a slash line must repaint, never emit a buffer-mutating command"
    );
    // No matches and non-slash lines fall through to the default key binding.
    assert_eq!(cycle_slash_preview("/zzzznope", true), None);
    assert_eq!(cycle_slash_preview("find files", false), None);
}
