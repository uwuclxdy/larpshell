use crate::cli::{CWD_LOCK, execute_shell_command, execute_shell_command_unlocked};
use std::env;
use std::path::PathBuf;

fn with_saved_cwd(f: impl FnOnce() + std::panic::UnwindSafe) {
    let _guard = CWD_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let original = env::current_dir().unwrap();
    let result = std::panic::catch_unwind(f);
    env::set_current_dir(&original).unwrap();
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

/// Canonicalized `/tmp` so path equality holds on macOS where `/tmp` is a
/// symlink to `/private/tmp` and `current_dir()` returns the physical path.
fn tmp_dir() -> PathBuf {
    std::fs::canonicalize("/tmp").unwrap_or_else(|_| PathBuf::from("/tmp"))
}

#[test]
fn empty_command_is_noop() {
    assert!(execute_shell_command("").is_ok());
    assert!(execute_shell_command("   ").is_ok());
}

#[test]
fn cd_bare_goes_home() {
    with_saved_cwd(|| {
        let home = env::var("HOME").unwrap();
        execute_shell_command_unlocked("cd").unwrap();
        assert_eq!(env::current_dir().unwrap(), PathBuf::from(&home));
    });
}

#[test]
fn cd_absolute_path() {
    with_saved_cwd(|| {
        execute_shell_command_unlocked("cd /tmp").unwrap();
        assert_eq!(env::current_dir().unwrap(), tmp_dir());
    });
}

#[test]
fn cd_tilde_expands_to_home() {
    with_saved_cwd(|| {
        let home = env::var("HOME").unwrap();
        execute_shell_command_unlocked("cd ~").unwrap();
        assert_eq!(env::current_dir().unwrap(), PathBuf::from(&home));
    });
}

#[test]
fn cd_tilde_subdir_expands() {
    with_saved_cwd(|| {
        let home = env::var("HOME").unwrap();
        let subdir = PathBuf::from(&home);
        assert!(subdir.is_dir(), "$HOME must exist");
        execute_shell_command_unlocked("cd ~").unwrap();
        assert_eq!(env::current_dir().unwrap(), subdir);
    });
}

#[test]
fn cd_nonexistent_keeps_cwd() {
    with_saved_cwd(|| {
        let before = env::current_dir().unwrap();
        assert!(execute_shell_command_unlocked("cd /nonexistent_dir_that_should_not_exist").is_err());
        assert_eq!(env::current_dir().unwrap(), before);
    });
}

#[test]
fn compound_cd_changes_cwd() {
    with_saved_cwd(|| {
        execute_shell_command_unlocked("cd /tmp && echo ok").unwrap();
        assert_eq!(env::current_dir().unwrap(), tmp_dir());
    });
}

#[test]
fn compound_cd_failed_keeps_cwd() {
    with_saved_cwd(|| {
        let before = env::current_dir().unwrap();
        assert!(execute_shell_command_unlocked("cd /nonexistent_dir && echo ok").is_err());
        assert_eq!(env::current_dir().unwrap(), before);
    });
}

#[test]
fn positional_arg_mutation_cannot_retarget_cwd_capture() {
    with_saved_cwd(|| {
        let target = env::temp_dir().join("larpshell_should_not_receive_cwd");
        let _ = std::fs::remove_file(&target);
        execute_shell_command_unlocked(&format!("set -- {}; cd /tmp", target.display())).unwrap();
        assert_eq!(env::current_dir().unwrap(), tmp_dir());
        assert!(!target.exists());
    });
}

#[test]
fn pipe_command_runs() {
    assert!(execute_shell_command("echo hello | cat").is_ok());
}

#[test]
fn regular_command_runs_via_shell() {
    assert!(execute_shell_command("echo hello").is_ok());
}

#[test]
fn multiline_command_runs_via_shell() {
    assert!(execute_shell_command("echo line1\necho line2").is_ok());
}

#[test]
fn render_config_uses_sapphire_accent_and_dim_help() {
    use crate::cli::render_config;
    use crate::common::CTP_BLUE;
    use inquire::ui::{Color, StyleSheet};

    let cfg = render_config();
    let accent = Color::rgb(CTP_BLUE.r, CTP_BLUE.g, CTP_BLUE.b);

    // sapphire prompt prefix, never the default green
    assert_eq!(cfg.prompt_prefix.style.fg, Some(accent));
    assert_ne!(cfg.prompt_prefix.style.fg, Some(Color::LightGreen));
    // selected row + cursor render sapphire, not the default cyan
    assert_eq!(cfg.selected_option, Some(StyleSheet::new().with_fg(accent)));
    assert_eq!(cfg.highlighted_option_prefix.style.fg, Some(accent));
    // persisted answer value renders sapphire, not the default inquire cyan
    assert_eq!(cfg.answer.fg, Some(accent));
    assert_ne!(cfg.answer.fg, Some(Color::LightCyan));
    // help is dimmed
    assert_eq!(cfg.help_message.fg, Some(Color::DarkGrey));
}

#[test]
fn inquire_cancel_and_interrupt_map_to_cancelled() {
    use crate::cli::map_inquire_cancel;
    use crate::error::LarpshellError;
    use inquire::InquireError;

    assert!(matches!(map_inquire_cancel(InquireError::OperationCanceled), LarpshellError::Cancelled));
    assert!(matches!(map_inquire_cancel(InquireError::OperationInterrupted), LarpshellError::Cancelled));
    // real inquire failures keep their normal conversion, not a silent cancel
    assert!(matches!(map_inquire_cancel(InquireError::NotTTY), LarpshellError::InquireError(InquireError::NotTTY)));
}
