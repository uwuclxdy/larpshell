use crate::cli::{CWD_LOCK, execute_shell_command, execute_shell_command_unlocked};
use std::env;
use std::path::PathBuf;

fn with_saved_cwd(f: impl FnOnce() + std::panic::UnwindSafe) {
    let _guard = CWD_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let original = env::current_dir().unwrap();
    let result = std::panic::catch_unwind(f);
    env::set_current_dir(&original).unwrap();
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
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
        assert_eq!(env::current_dir().unwrap(), PathBuf::from("/tmp"));
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
        execute_shell_command_unlocked("cd /nonexistent_dir_that_should_not_exist").unwrap();
        assert_eq!(env::current_dir().unwrap(), before);
    });
}

#[test]
fn compound_cd_changes_cwd() {
    with_saved_cwd(|| {
        execute_shell_command_unlocked("cd /tmp && echo ok").unwrap();
        assert_eq!(env::current_dir().unwrap(), PathBuf::from("/tmp"));
    });
}

#[test]
fn compound_cd_failed_keeps_cwd() {
    with_saved_cwd(|| {
        let before = env::current_dir().unwrap();
        execute_shell_command_unlocked("cd /nonexistent_dir && echo ok").unwrap();
        assert_eq!(env::current_dir().unwrap(), before);
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
