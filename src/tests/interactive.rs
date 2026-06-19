use crate::interactive::{NlshHelper, format_preview_row, next_cycle_index, preview_items};
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
    let row = format_preview_row(
        "/uninstall",
        0,
        "uninstall larpshell completely",
        width,
        false,
    );
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
        let row = strip_ansi_escapes::strip_str(format_preview_row(
            "/api",
            0,
            "configure",
            usize::MAX,
            sel,
        ));
        let prefix = &row[..row.find("configure").unwrap()];
        (UnicodeWidthStr::width(prefix), row)
    };
    let (unselected_col, _) = desc_col(false);
    let (selected_col, selected) = desc_col(true);
    assert_eq!(
        unselected_col, selected_col,
        "selected row must not shift the description column"
    );
    assert!(
        selected.starts_with("❯ "),
        "selected row needs a marker: {selected:?}"
    );
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
    assert!(
        items
            .iter()
            .all(|i| i.replacement == i.name && i.name.starts_with('/'))
    );
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
