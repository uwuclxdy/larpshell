use crate::interactive::{NlshHelper, format_preview_row};
use rustyline::highlight::{CmdKind, Highlighter};

fn format_preview_row_plain(cmd_name: &str, typed_len: usize, description: &str) -> String {
    colored::control::set_override(false);
    let r = format_preview_row(cmd_name, typed_len, description, usize::MAX);
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
    let row = format_preview_row("/uninstall", 0, "uninstall larpshell completely", width);
    let plain = strip_ansi_escapes::strip_str(&row);
    assert!(plain.chars().count() <= width, "row too wide: {plain:?}");
    assert!(plain.ends_with('…'), "expected ellipsis: {plain:?}");
}
