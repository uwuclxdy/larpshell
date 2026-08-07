/// Tests for the confirmation + inline-edit flow.
///
/// All tests run the real binary with `LARPSHELL_FORCE_INTERACTIVE=1` (so the confirm
/// prompts actually read from stdin instead of auto-approving) and a mock Ollama
/// server that returns a fixed command string. Stdin bytes represent raw key
/// presses fed to the confirmation and edit UI.
///
/// Key byte reference used throughout:
///   y / n / e          — confirm yes / no / explain
///   \n                 — Enter (confirm yes in confirm prompt; confirm edit in editor)
///   \x1b[A             — Arrow Up  (enter edit mode)
///   \x1b[D / \x1b[C   — Arrow Left / Right (move cursor)
///   \x1b[H / \x1b[F   — Home / End
///   \x7f               — Backspace
///   \x1b[3~            — Delete
use crate::tests;

// ── helpers ──────────────────────────────────────────────────────────────────

const CMD: &str = "echo mock";
const EXPL: &str = "✅ explain mock";

/// Set up a fresh home dir + mock server that returns `responses` in order.
/// Returns `(home, port)`.
fn setup(suffix: &str, responses: &[&str]) -> (std::path::PathBuf, u16) {
    let home = tests::temp_home(suffix);
    let port = tests::mock_ollama(responses);
    tests::write_ollama_config(&home, port);
    (home, port)
}

/// Run a single-run request with the given raw stdin bytes in interactive mode.
fn run(home: &std::path::Path, stdin: &[u8]) -> std::process::Output {
    tests::run_with_stdin_interactive(home, &["show disk usage"], stdin)
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ── basic confirmation ────────────────────────────────────────────────────────

#[test]
fn confirm_y_executes_command() {
    let (home, _) = setup("edit_confirm_y", &[CMD]);
    let out = run(&home, b"y");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock");
}

#[test]
fn confirm_enter_executes_command() {
    let (home, _) = setup("edit_confirm_enter", &[CMD]);
    let out = run(&home, b"\n");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock");
}

#[test]
fn confirm_n_prints_nothing() {
    let (home, _) = setup("edit_confirm_n", &[CMD]);
    let out = run(&home, b"n");
    assert_eq!(out.status.code(), Some(130));
    assert_eq!(stdout(&out), "");
}

// A bare ESC at the confirm prompt cancels like N (exit 130). On the piped test
// path a lone 0x1b at EOF is delivered as Esc rather than swallowing the next key.
#[test]
fn confirm_esc_cancels() {
    let (home, _) = setup("confirm_esc", &[CMD]);
    let out = run(&home, b"\x1b");
    assert_eq!(out.status.code(), Some(130));
    assert_eq!(stdout(&out), "");
}

// ── edit: basic ──────────────────────────────────────────────────────────────

// Arrow Up then Enter with no edits — command must stay identical.
#[test]
fn edit_no_change() {
    let (home, _) = setup("edit_no_change", &[CMD]);
    // \x1b[A = Arrow Up (enter edit), \n = Enter (confirm edit), y = confirm run
    let out = run(&home, b"\x1b[A\ny");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock");
}

// Arrow Up then type " suffix" then Enter — suffix is appended because cursor
// starts at the end of the command.
#[test]
fn edit_append_suffix() {
    let (home, _) = setup("edit_append_suffix", &[CMD]);
    let out = run(&home, b"\x1b[A suffix\ny");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock suffix");
}

// Arrow Up then 2× Backspace — removes the last 2 characters ("ck" from "mock").
#[test]
fn edit_backspace_removes_chars() {
    let (home, _) = setup("edit_backspace", &[CMD]);
    // "echo mock" → backspace × 2 → "echo mo"
    let out = run(&home, b"\x1b[A\x7f\x7f\ny");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mo");
}

// Arrow Up + Left + Delete — moves cursor left one, then deletes the char under
// cursor, removing the 'k' from "mock".
#[test]
fn edit_delete_key_removes_char_at_cursor() {
    let (home, _) = setup("edit_delete_key", &[CMD]);
    // cursor at end (pos=9), Left → pos=8 (on 'k'), Delete → removes 'k'
    let out = run(&home, b"\x1b[A\x1b[D\x1b[3~\ny");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "moc");
}

// Arrow Up + Left×4 + type 'X' + End — inserts 'X' before 'm' in "mock".
// "echo mock" has length 9; cursor starts at 9. Left×4 → cursor=5 (before 'm').
#[test]
fn edit_left_then_insert_char() {
    let (home, _) = setup("edit_left_insert", &[CMD]);
    // Left×4 → cursor=5 (before 'm'); type 'X' → "echo Xmock"; End keeps cursor at end
    let out = run(&home, b"\x1b[A\x1b[D\x1b[D\x1b[D\x1b[DX\x1b[F\ny");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "Xmock");
}

// Arrow Up + Home + type "prefix " — inserts "prefix " at the start.
#[test]
fn edit_home_then_insert_prefix() {
    let (home, _) = setup("edit_home_insert", &[CMD]);
    let out = run(&home, b"\x1b[A\x1b[Hprefix \ny");
    assert!(!out.status.success());
    assert_eq!(stdout(&out), "");
}

// Arrow Up + Home + type 'X' + End + Backspace — inserts at start then removes
// the last char, exercising both ends of the buffer.
#[test]
fn edit_insert_at_start_delete_from_end() {
    let (home, _) = setup("edit_both_ends", &[CMD]);
    // Home → type 'X' → "Xecho mock"; End → Backspace → "Xecho moc"
    let out = run(&home, b"\x1b[A\x1b[HX\x1b[F\x7f\ny");
    assert!(!out.status.success());
    assert_eq!(stdout(&out), "");
}

// Arrow Up + Right (at end, no-op) — cursor was already at end, no change.
#[test]
fn edit_right_at_end_is_noop() {
    let (home, _) = setup("edit_right_noop", &[CMD]);
    let out = run(&home, b"\x1b[A\x1b[C\ny");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock");
}

// ── edit: modified/extended escape sequences leak no stray chars ───────────────

// ctrl+left (\x1b[1;5D) maps to a plain Left; the modifier params and final byte
// are consumed whole, so the command runs unchanged. The old parser stopped at
// the '1' and injected the leftover "5D" into the buffer.
#[test]
fn edit_ctrl_left_no_stray_chars() {
    let (home, _) = setup("edit_ctrl_left", &[CMD]);
    let out = run(&home, b"\x1b[A\x1b[1;5D\ny");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock");
}

// PgUp (\x1b[5~) is an unsupported no-op whose '~' terminator must not leak. The
// old parser left the '~' unconsumed, injecting it and producing "mock~".
#[test]
fn edit_pgup_is_noop_no_stray_chars() {
    let (home, _) = setup("edit_pgup", &[CMD]);
    let out = run(&home, b"\x1b[A\x1b[5~\ny");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock");
}

// ── edit: run twice ───────────────────────────────────────────────────────────

// Arrow Up + " a" + Enter → loop → Arrow Up + " b" + Enter → Y.
// The edit accumulates across two separate edit sessions.
#[test]
fn edit_twice_accumulates() {
    let (home, _) = setup("edit_twice", &[CMD]);
    let out = run(&home, b"\x1b[A a\n\x1b[A b\ny");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock a b");
}

// ── explain then confirm ──────────────────────────────────────────────────────

// E → explanation fetched → Y → original command executed.
#[test]
fn explain_then_y_executes_command() {
    let (home, _) = setup("explain_y", &[CMD, EXPL]);
    let out = run(&home, b"ey");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock");
}

// E → explanation fetched → N → nothing printed, exits 130.
#[test]
fn explain_then_n_prints_nothing() {
    let (home, _) = setup("explain_n", &[CMD, EXPL]);
    let out = run(&home, b"en");
    assert_eq!(out.status.code(), Some(130));
    assert_eq!(stdout(&out), "");
}

// E → explanation fetched → Enter → original command executed.
#[test]
fn explain_then_enter_executes_command() {
    let (home, _) = setup("explain_enter", &[CMD, EXPL]);
    let out = run(&home, b"e\n");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock");
}

// ── explain then edit ─────────────────────────────────────────────────────────

// E → Arrow Up → edit " fix" → Enter → Y → edited command.
// After explaining, pressing Arrow Up enters edit mode and returns to the full
// confirm_with_explain prompt (with E available again).
#[test]
fn explain_then_edit_then_y() {
    let (home, _) = setup("explain_edit_y", &[CMD, EXPL]);
    // e = explain, \x1b[A = arrow up (edit), " fix\n" = edit, y = confirm
    let out = run(&home, b"e\x1b[A fix\ny");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock fix");
}

// E → Arrow Up → edit → Enter → E (explain again) → Y → edited command.
// Exercises editing after an explanation and then requesting a second explanation.
#[test]
fn explain_then_edit_then_explain_again_then_y() {
    let (home, _) = setup("explain_edit_explain_y", &[CMD, EXPL, EXPL]);
    let out = run(&home, b"e\x1b[A fix\ney");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock fix");
}

// ── edit then explain ─────────────────────────────────────────────────────────

// Arrow Up → edit " fix" → Enter → E → Y → edited command printed.
#[test]
fn edit_then_explain_then_y() {
    let (home, _) = setup("edit_explain_y", &[CMD, EXPL]);
    let out = run(&home, b"\x1b[A fix\ney");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock fix");
}

// Arrow Up → edit " fix" → Enter → E → N → nothing printed, exits 130.
#[test]
fn edit_then_explain_then_n() {
    let (home, _) = setup("edit_explain_n", &[CMD, EXPL]);
    let out = run(&home, b"\x1b[A fix\nen");
    assert_eq!(out.status.code(), Some(130));
    assert_eq!(stdout(&out), "");
}

// Arrow Up → edit " a" → Enter → E → Arrow Up → edit " b" → Enter → Y.
// Edit, explain, edit again, confirm.
#[test]
fn edit_explain_edit_again_then_y() {
    let (home, _) = setup("edit_expl_edit_y", &[CMD, EXPL]);
    let out = run(&home, b"\x1b[A a\ne\x1b[A b\ny");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "mock a b");
}

// ── interactive mode (command is executed, not printed) ───────────────────────

// In interactive mode the generated command is executed directly. Verify that
// editing the command before confirmation causes the *edited* command to run.
#[test]
fn interactive_mode_edit_then_execute() {
    let home = tests::temp_home("interactive_edit");
    // Mock returns "echo before" as the generated command. After editing,
    // we append " after" to get "echo before after".
    let port = tests::mock_ollama(&["echo before"]);
    tests::write_ollama_config(&home, port);

    // Stdin: "show files\n" → rustyline reads the NL request;
    // then "\x1b[A after\ny" → arrow up + type " after" + Enter + Y.
    let out = tests::run_with_stdin_interactive(&home, &[], b"show files\n\x1b[A after\ny");

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let output = String::from_utf8_lossy(&out.stdout);
    assert!(
        output.contains("before after"),
        "expected executed output to contain 'before after', got: {output}"
    );
}
