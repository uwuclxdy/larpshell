use super::{run, temp_home, write_ollama_config};
use std::fs;
use std::path::PathBuf;

fn shell_config_home(suffix: &str) -> PathBuf {
    let home = temp_home(suffix);
    // temp_home pre-creates the bash completion marker so first-run install exits
    // early; drop it so the install actually fires.
    fs::remove_file(home.join(".local/share/bash-completion/completions/larpshell")).unwrap();
    fs::write(home.join(".bashrc"), "# user bashrc\n").unwrap();
    fs::write(home.join(".zshrc"), "# user zshrc\n").unwrap();
    fs::create_dir_all(home.join(".config/fish")).unwrap();
    home
}

#[test]
fn first_run_installs_wrapper_functions_for_bash_zsh_and_fish() {
    let home = shell_config_home("first_run_all_shells");

    let out = run(&home, &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "first-run install must exit 0; stderr: {stderr}");

    let bashrc = fs::read_to_string(home.join(".bashrc")).unwrap();
    assert!(bashrc.contains("larpshell() {"), "bash wrapper must be installed; bashrc: {bashrc}");
    assert!(bashrc.contains("# user bashrc"), "installing must not drop the user's own bashrc content; bashrc: {bashrc}");

    let zshrc = fs::read_to_string(home.join(".zshrc")).unwrap();
    assert!(zshrc.contains("larpshell() {"), "zsh wrapper must be installed; zshrc: {zshrc}");
    assert!(zshrc.contains("# user zshrc"), "installing must not drop the user's own zshrc content; zshrc: {zshrc}");

    assert!(home.join(".config/fish/functions/larpshell.fish").exists(), "fish wrapper must be installed");

    assert!(home.join(".local/share/bash-completion/completions/larpshell").exists(), "bash completion must be installed");
    assert!(home.join(".local/share/zsh/site-functions/_larpshell").exists(), "zsh completion must be installed");
    assert!(home.join(".config/fish/completions/larpshell.fish").exists(), "fish completion must be installed");
}

#[test]
fn second_run_does_not_duplicate_installed_wrappers() {
    let home = shell_config_home("second_run_no_dupes");

    let first = run(&home, &[]);
    let stderr = String::from_utf8_lossy(&first.stderr);
    assert!(first.status.success(), "first run must exit 0; stderr: {stderr}");

    // The second run installs nothing, so it falls through to the piped flow:
    // a provider config lets it reach the empty-stdin no-op and exit 0.
    write_ollama_config(&home, 11434);

    let second = run(&home, &[]);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(second.status.success(), "second run must exit 0; stderr: {stderr}");

    let bashrc = fs::read_to_string(home.join(".bashrc")).unwrap();
    assert_eq!(bashrc.matches("larpshell() {").count(), 1, "bash wrapper must not duplicate; bashrc: {bashrc}");
    let zshrc = fs::read_to_string(home.join(".zshrc")).unwrap();
    assert_eq!(zshrc.matches("larpshell() {").count(), 1, "zsh wrapper must not duplicate; zshrc: {zshrc}");
    let fish = fs::read_to_string(home.join(".config/fish/functions/larpshell.fish")).unwrap();
    assert_eq!(fish.matches("function larpshell").count(), 1, "fish wrapper must not duplicate; fish: {fish}");
}
