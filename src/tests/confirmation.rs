use crate::confirmation::{
    ConfirmPromptMode, ConfirmResult, KeyEvent, ResponseStyle, confirm_from_reader,
    display_command, display_explanation, display_message, display_response, parse_key_from_reader,
    style_html_tags_for_test, style_message_markup_for_test,
};

#[test]
fn display_command_single_line_returns_one() {
    assert_eq!(display_command("echo hi"), 1);
}

#[test]
fn display_command_multiline_returns_n_plus_one() {
    assert_eq!(display_command("echo hi\necho bye"), 3);
    assert_eq!(display_command("a\nb\nc"), 4);
}

#[test]
fn display_message_single_line_returns_one() {
    assert_eq!(display_message("done"), 1);
}

#[test]
fn display_message_multiline_returns_line_count() {
    assert_eq!(display_message("line one\nline two\nline three"), 3);
}

#[test]
fn display_response_delegates_to_message_renderer() {
    assert_eq!(display_response("done", ResponseStyle::Message), 1);
}

#[test]
fn display_explanation_single_line_returns_one() {
    assert_eq!(display_explanation("pipes stdout to a file"), 1);
}

#[test]
fn display_explanation_multiline_returns_line_count() {
    assert_eq!(display_explanation("line one\nline two\nline three"), 3);
}

#[test]
fn style_html_tags_converts_bold() {
    let result = style_html_tags_for_test("<b>hello</b>", true);
    assert_eq!(result, "\x1b[1mhello\x1b[22m");
}

#[test]
fn style_html_tags_converts_italic() {
    let result = style_html_tags_for_test("<i>hello</i>", true);
    assert_eq!(result, "\x1b[3mhello\x1b[23m");
}

#[test]
fn style_html_tags_converts_underline() {
    let result = style_html_tags_for_test("<u>hello</u>", true);
    assert_eq!(result, "\x1b[4mhello\x1b[24m");
}

#[test]
fn style_html_tags_strips_when_no_color() {
    let result = style_html_tags_for_test("<b>bold</b> and <i>italic</i>", false);
    assert_eq!(result, "bold and italic");
}

#[test]
fn style_message_markup_converts_supported_markdown() {
    let result = style_message_markup_for_test(
        "- **bold** `code` *italic* [label](https://example.com)",
        true,
    );
    assert_eq!(
        result,
        "\x1b[1mbold\x1b[22m \x1b[7mcode\x1b[27m \x1b[3mitalic\x1b[23m label"
    );
}

#[test]
fn style_message_markup_strips_unsupported_markdown_when_no_color() {
    let result = style_message_markup_for_test(
        "> ## heading\n1. [x] **bold** and ~~gone~~ with [label](https://example.com)",
        false,
    );
    assert_eq!(result, "heading\nbold and gone with label");
}

#[test]
fn style_message_markup_drops_rules_and_fences() {
    let result = style_message_markup_for_test("---\n```\nbody\n~~~", false);
    assert_eq!(result, "\n\nbody\n");
}

#[test]
fn style_message_markup_preserves_non_ascii_outside_links() {
    // Multi-byte sequences (em-dash, CJK) must survive strip_markdown_links intact.
    let result = style_message_markup_for_test("café — 日本語 [label](url)", false);
    assert_eq!(result, "café — 日本語 label");
}

#[test]
fn parse_key_from_reader_maps_enter() {
    let mut input = std::io::Cursor::new(b"\n");
    assert!(matches!(parse_key_from_reader(&mut input), KeyEvent::Enter));
}

#[test]
fn parse_key_from_reader_maps_carriage_return() {
    let mut input = std::io::Cursor::new(b"\r");
    assert!(matches!(parse_key_from_reader(&mut input), KeyEvent::Enter));
}

#[test]
fn parse_key_from_reader_maps_char_y() {
    let mut input = std::io::Cursor::new(b"y");
    assert!(matches!(
        parse_key_from_reader(&mut input),
        KeyEvent::Char('y')
    ));
}

#[test]
fn parse_key_from_reader_maps_char_uppercase_y() {
    let mut input = std::io::Cursor::new(b"Y");
    assert!(matches!(
        parse_key_from_reader(&mut input),
        KeyEvent::Char('Y')
    ));
}

#[test]
fn parse_key_from_reader_maps_char_e() {
    let mut input = std::io::Cursor::new(b"e");
    assert!(matches!(
        parse_key_from_reader(&mut input),
        KeyEvent::Char('e')
    ));
}

#[test]
fn parse_key_from_reader_maps_char_n() {
    let mut input = std::io::Cursor::new(b"n");
    assert!(matches!(
        parse_key_from_reader(&mut input),
        KeyEvent::Char('n')
    ));
}

#[test]
fn parse_key_from_reader_maps_ctrl_c() {
    let mut input = std::io::Cursor::new(b"\x03");
    assert!(matches!(parse_key_from_reader(&mut input), KeyEvent::CtrlC));
}

#[test]
fn parse_key_from_reader_maps_backspace() {
    let mut input = std::io::Cursor::new(b"\x7f");
    assert!(matches!(
        parse_key_from_reader(&mut input),
        KeyEvent::Backspace
    ));
}

#[test]
fn parse_key_from_reader_maps_arrow_up_escape_sequence() {
    let mut input = std::io::Cursor::new(b"\x1b[A");
    assert!(matches!(
        parse_key_from_reader(&mut input),
        KeyEvent::ArrowUp
    ));
}

#[test]
fn parse_key_from_reader_maps_arrow_down_escape_sequence() {
    let mut input = std::io::Cursor::new(b"\x1b[B");
    assert!(matches!(parse_key_from_reader(&mut input), KeyEvent::Other));
}

#[test]
fn parse_key_from_reader_maps_arrow_right_escape_sequence() {
    let mut input = std::io::Cursor::new(b"\x1b[C");
    assert!(matches!(parse_key_from_reader(&mut input), KeyEvent::Right));
}

#[test]
fn parse_key_from_reader_maps_arrow_left_escape_sequence() {
    let mut input = std::io::Cursor::new(b"\x1b[D");
    assert!(matches!(parse_key_from_reader(&mut input), KeyEvent::Left));
}

#[test]
fn parse_key_from_reader_maps_delete_key() {
    let mut input = std::io::Cursor::new(b"\x1b[3~");
    assert!(matches!(
        parse_key_from_reader(&mut input),
        KeyEvent::Delete
    ));
}

#[test]
fn parse_key_from_reader_maps_eof() {
    let mut input = std::io::Cursor::new(b"");
    assert!(matches!(parse_key_from_reader(&mut input), KeyEvent::Eof));
}

#[test]
fn confirm_from_reader_with_explain_on_enter_returns_yes() {
    let mut keys = vec![KeyEvent::Enter].into_iter();
    let result = confirm_from_reader(
        || keys.next().unwrap(),
        ConfirmPromptMode::WithExplain,
        1,
        0,
    );
    assert!(matches!(result, ConfirmResult::Yes));
}

#[test]
fn confirm_from_reader_with_explain_on_y_returns_yes() {
    let mut keys = vec![KeyEvent::Char('y')].into_iter();
    let result = confirm_from_reader(
        || keys.next().unwrap(),
        ConfirmPromptMode::WithExplain,
        1,
        0,
    );
    assert!(matches!(result, ConfirmResult::Yes));
}

#[test]
fn confirm_from_reader_with_explain_on_e_returns_explain() {
    let mut keys = vec![KeyEvent::Char('e')].into_iter();
    let result = confirm_from_reader(
        || keys.next().unwrap(),
        ConfirmPromptMode::WithExplain,
        1,
        0,
    );
    assert!(matches!(result, ConfirmResult::Explain));
}

#[test]
fn confirm_from_reader_with_explain_on_n_returns_cancel() {
    let mut keys = vec![KeyEvent::Char('n')].into_iter();
    let result = confirm_from_reader(
        || keys.next().unwrap(),
        ConfirmPromptMode::WithExplain,
        1,
        0,
    );
    assert!(matches!(result, ConfirmResult::Cancel));
}

#[test]
fn confirm_from_reader_on_arrow_up_returns_edit() {
    let mut keys = vec![KeyEvent::ArrowUp].into_iter();
    let result = confirm_from_reader(|| keys.next().unwrap(), ConfirmPromptMode::Simple, 1, 1);
    assert!(matches!(result, ConfirmResult::Edit));
}

#[test]
fn confirm_from_reader_on_eof_returns_no() {
    let mut keys = vec![KeyEvent::Eof].into_iter();
    let result = confirm_from_reader(|| keys.next().unwrap(), ConfirmPromptMode::Simple, 1, 0);
    assert!(matches!(result, ConfirmResult::No));
}

#[test]
fn confirm_from_reader_ignores_e_in_simple_mode() {
    let mut keys = vec![KeyEvent::Char('e'), KeyEvent::Enter].into_iter();
    let result = confirm_from_reader(|| keys.next().unwrap(), ConfirmPromptMode::Simple, 1, 0);
    assert!(matches!(result, ConfirmResult::Yes));
}
