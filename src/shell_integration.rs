use std::fs::{self, OpenOptions};
use std::io::Write;

use crate::cli::home_dir;
use crate::error::LarpshellError;

pub const fn generate_bash_autocomplete() -> &'static str {
    r#"_larpshell_completions() {
    local cur prev
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    if [ $COMP_CWORD -eq 1 ]; then
        COMPREPLY=( $(compgen -W "api agent history prompt explain uninstall --help --version" -- "$cur") )
    elif [ $COMP_CWORD -eq 2 ]; then
        case "$prev" in
            agent|history)
                COMPREPLY=( $(compgen -W "off safe on" -- "$cur") )
                ;;
            prompt)
                COMPREPLY=( $(compgen -W "system explain" -- "$cur") )
                ;;
        esac
    elif [ $COMP_CWORD -eq 3 ]; then
        case "${COMP_WORDS[1]}" in
            prompt)
                COMPREPLY=( $(compgen -W "show edit" -- "$cur") )
                ;;
        esac
    fi
    return 0
}
complete -F _larpshell_completions larpshell"#
}

pub const fn generate_zsh_autocomplete() -> &'static str {
    r#"#compdef larpshell

_larpshell() {
    local -a commands
    commands=(
        'api:configure API provider (Gemini, Ollama, OpenRouter, LM Studio, OpenAI)'
        'agent:set agent mode (off, safe, on)'
        'explain:explain a shell command'
        'history:enable or disable prompt history'
        'prompt:view or edit system/explain prompts'
        'uninstall:uninstall larpshell'
    )

    _arguments -C \
        '1: :->cmds' \
        '--help[show help information]' \
        '--version[show version information]' \
        '*::arg:->args'

    case "$state" in
        cmds)
            _describe -t commands 'larpshell commands' commands
            ;;
        args)
            case "${words[1]}" in
                agent|history)
                    local -a toggles
                    toggles=('on:enable' 'off:disable')
                    _arguments '1: :->toggle'
                    case "$state" in
                        toggle) _describe -t toggles 'toggle' toggles ;;
                    esac
                    ;;
                prompt)
                    local -a kinds actions
                    kinds=('system:system prompt' 'explain:explain prompt')
                    actions=('show:show prompt' 'edit:edit prompt')
                    _arguments \
                        '1: :->kind' \
                        '2: :->action'
                    case "$state" in
                        kind) _describe -t kinds 'prompt kind' kinds ;;
                        action) _describe -t actions 'prompt action' actions ;;
                    esac
                    ;;
            esac
            ;;
    esac
}

_larpshell"#
}

pub const fn generate_fish_autocomplete() -> &'static str {
    r#"# larpshell autocomplete
complete -c larpshell -f
complete -c larpshell -n "__fish_use_subcommand" -a api -d 'configure API provider (Gemini, Ollama, OpenRouter, LM Studio, OpenAI)'
complete -c larpshell -n "__fish_use_subcommand" -a agent -d 'set agent mode'
complete -c larpshell -n "__fish_use_subcommand" -a explain -d 'explain a shell command'
complete -c larpshell -n "__fish_use_subcommand" -a history -d 'enable or disable prompt history'
complete -c larpshell -n "__fish_use_subcommand" -a prompt -d 'view or edit system/explain prompts'
complete -c larpshell -n "__fish_use_subcommand" -a uninstall -d 'uninstall larpshell'
complete -c larpshell -l help -d 'show help information'
complete -c larpshell -l version -d 'show version information'
complete -c larpshell -n "__fish_seen_subcommand_from agent" -a "off safe on" -d 'set agent mode'
complete -c larpshell -n "__fish_seen_subcommand_from history" -a "on off" -d 'toggle history'
complete -c larpshell -n "__fish_seen_subcommand_from prompt" -a system -d 'system prompt'
complete -c larpshell -n "__fish_seen_subcommand_from prompt" -a explain -d 'explain prompt'
complete -c larpshell -n "__fish_seen_subcommand_from prompt; and __fish_seen_subcommand_from system explain" -a "show edit" -d 'prompt action'"#
}

pub const fn generate_bash_function() -> &'static str {
    r#"larpshell() {
    if [ $# -eq 0 ]; then
        command larpshell
        return $?
    fi

    case "$1" in
        api|agent|explain|history|uninstall|prompt|--help|-h|--version|-V)
            command larpshell "$@"
            return $?
            ;;
    esac

    local cmd=$(command larpshell "$@")
    local exit_code=$?
    if [ $exit_code -eq 0 ] && [ -n "$cmd" ]; then
        if [[ "$cmd" =~ ^(Usage:|error:|Commands:|larpshell\ [0-9]|$'\e'|$'\033'|✓|.*:$) ]]; then
            echo "$cmd"
            return 0
        fi
        eval "$cmd"
    else
        return $exit_code
    fi
}"#
}

pub const fn generate_fish_function() -> &'static str {
    r#"function larpshell
    if test (count $argv) -eq 0
        command larpshell
        return $status
    end

    switch $argv[1]
        case api explain history uninstall prompt -- help --help -h --version -V
            command larpshell $argv
            return $status
    end

    set cmd (command larpshell $argv)
    set exit_code $status
    if test $exit_code -eq 0 -a -n "$cmd"
        if string match -qr '^(Usage:|error:|Commands:|larpshell [0-9]|\x1b|\e|✓|.*:$)' -- "$cmd"
            echo "$cmd"
            return 0
        end
        eval $cmd
    else
        return $exit_code
    end
end"#
}

pub fn auto_setup_shell_function() -> Result<bool, LarpshellError> {
    verify_and_fix_integrations()?;
    let bash_added = setup_bash_integration()?;
    let fish_added = setup_fish_integration()?;
    let autocomplete_added = setup_autocomplete()?;
    Ok(bash_added || fish_added || autocomplete_added)
}

fn verify_and_fix_integrations() -> Result<(), LarpshellError> {
    verify_and_fix_bash_integration()?;
    verify_and_fix_fish_integration()?;
    verify_and_fix_autocomplete()?;
    Ok(())
}

fn verify_and_fix_autocomplete() -> Result<(), LarpshellError> {
    let home = home_dir();
    verify_and_fix_autocomplete_file(
        &home.join(".local/share/bash-completion/completions/larpshell"),
        generate_bash_autocomplete(),
        Some("# larpshell bash autocomplete"),
    )?;
    verify_and_fix_autocomplete_file(
        &home.join(".local/share/zsh/site-functions/_larpshell"),
        generate_zsh_autocomplete(),
        Some("# larpshell zsh autocomplete"),
    )?;
    verify_and_fix_autocomplete_file(
        &home.join(".config/fish/completions/larpshell.fish"),
        generate_fish_autocomplete(),
        None,
    )?;
    Ok(())
}

/// Rewrites `path` with `header` (if any) + `expected` when its content drifts.
/// No-ops when the file doesn't exist or already contains the expected content.
fn verify_and_fix_autocomplete_file(
    path: &std::path::Path,
    expected: &str,
    header: Option<&str>,
) -> Result<(), LarpshellError> {
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(path)?;
    if content.contains(expected) {
        return Ok(());
    }

    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    if let Some(h) = header {
        writeln!(file, "{h}")?;
    }
    writeln!(file, "{expected}")?;
    Ok(())
}

fn verify_and_fix_bash_integration() -> Result<(), LarpshellError> {
    let home = home_dir();
    let bashrc_path = home.join(".bashrc");

    if !bashrc_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&bashrc_path)?;

    // check if integration exists
    if !content.contains("larpshell() {") {
        return Ok(());
    }

    // extract the function and verify it matches
    let expected_function = generate_bash_function();

    if !content.contains(expected_function) {
        // function exists but doesn't match - remove and reinstall
        remove_bash_integration()?;
        setup_bash_integration()?;
    }

    Ok(())
}

fn verify_and_fix_fish_integration() -> Result<(), LarpshellError> {
    let home = home_dir();
    let fish_function_path = home.join(".config/fish/functions/larpshell.fish");

    if !fish_function_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&fish_function_path)?;

    // verify the function matches expected content
    let expected_function = generate_fish_function();

    if !content.contains(expected_function) {
        // function exists but doesn't match - remove and reinstall
        remove_fish_integration()?;
        setup_fish_integration()?;
    }

    Ok(())
}

fn setup_bash_integration() -> Result<bool, LarpshellError> {
    let home = home_dir();
    let bashrc_path = home.join(".bashrc");

    if !bashrc_path.exists() {
        return Ok(false);
    }

    let content = fs::read_to_string(&bashrc_path)?;

    if content.contains("larpshell() {") || content.contains("larpshell()") {
        return Ok(false);
    }

    let mut file = OpenOptions::new().append(true).open(&bashrc_path)?;
    writeln!(file, "\n# larpshell shell integration")?;
    writeln!(file, "{}", generate_bash_function())?;
    Ok(true)
}

fn setup_fish_integration() -> Result<bool, LarpshellError> {
    let home = home_dir();
    let fish_functions_dir = home.join(".config/fish/functions");
    let fish_function_path = fish_functions_dir.join("larpshell.fish");

    if fish_function_path.exists() {
        return Ok(false);
    }

    let fish_config_dir = home.join(".config/fish");
    if !fish_config_dir.exists() {
        return Ok(false);
    }

    fs::create_dir_all(&fish_functions_dir)?;

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&fish_function_path)?;

    writeln!(file, "# larpshell shell integration")?;
    writeln!(file, "{}", generate_fish_function())?;
    Ok(true)
}

/// Removes a marked function block from shell config content.
/// Looks for `marker` as a comment line, then tracks brace depth starting from
/// the line matching `function_sig` until braces balance to zero.
/// Returns the cleaned content and whether the block was found.
fn remove_marked_function_block(content: &str, marker: &str, function_sig: &str) -> (String, bool) {
    let lines: Vec<&str> = content.lines().collect();
    let mut new_lines = Vec::new();
    let mut skip = false;
    let mut brace_depth = 0;
    let mut in_function = false;
    let mut found = false;

    for line in lines {
        if line.trim() == marker {
            skip = true;
            found = true;
            continue;
        }

        if skip {
            if !in_function && line.contains(function_sig) {
                in_function = true;
                brace_depth += i32::try_from(line.matches('{').count()).unwrap_or(i32::MAX);
            } else if in_function {
                brace_depth += i32::try_from(line.matches('{').count()).unwrap_or(i32::MAX);
                brace_depth -= i32::try_from(line.matches('}').count()).unwrap_or(i32::MAX);

                if brace_depth == 0 {
                    skip = false;
                    in_function = false;
                    continue;
                }
            }
            continue;
        }

        new_lines.push(line);
    }

    if found {
        while new_lines.last().is_some_and(|l| l.trim().is_empty()) {
            new_lines.pop();
        }
    }

    (new_lines.join("\n") + "\n", found)
}

pub fn remove_bash_integration() -> Result<bool, LarpshellError> {
    let home = home_dir();
    let bashrc_path = home.join(".bashrc");

    if !bashrc_path.exists() {
        return Ok(false);
    }

    let content = fs::read_to_string(&bashrc_path)?;

    if !content.contains("larpshell() {") && !content.contains("larpshell()") {
        return Ok(false);
    }

    let (new_content, found) =
        remove_marked_function_block(&content, "# larpshell shell integration", "larpshell()");

    if found {
        fs::write(&bashrc_path, new_content)?;
    }

    Ok(found)
}

pub fn remove_fish_integration() -> Result<bool, LarpshellError> {
    let home = home_dir();
    remove_file_if_exists(&home.join(".config/fish/functions/larpshell.fish"))
}

fn setup_autocomplete() -> Result<bool, LarpshellError> {
    let bash_added = setup_bash_autocomplete()?;
    let zsh_added = setup_zsh_autocomplete()?;
    let fish_added = setup_fish_autocomplete()?;
    Ok(bash_added || zsh_added || fish_added)
}

fn setup_bash_autocomplete() -> Result<bool, LarpshellError> {
    let home = home_dir();
    let completion_dir = home.join(".local/share/bash-completion/completions");
    let completion_path = completion_dir.join("larpshell");

    if completion_path.exists() {
        return Ok(false); // already handled by verify_and_fix_bash_autocomplete
    }

    fs::create_dir_all(&completion_dir)?;

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&completion_path)?;

    writeln!(file, "# larpshell bash autocomplete")?;
    writeln!(file, "{}", generate_bash_autocomplete())?;

    Ok(true)
}

fn setup_zsh_autocomplete() -> Result<bool, LarpshellError> {
    let home = home_dir();
    let zsh_config = home.join(".zshrc");
    if !zsh_config.exists() {
        return Ok(false);
    }

    let completion_dir = home.join(".local/share/zsh/site-functions");
    let completion_path = completion_dir.join("_larpshell");

    if completion_path.exists() {
        return Ok(false); // already handled by verify_and_fix_zsh_autocomplete
    }

    fs::create_dir_all(&completion_dir)?;

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&completion_path)?;

    writeln!(file, "# larpshell zsh autocomplete")?;
    writeln!(file, "{}", generate_zsh_autocomplete())?;

    let zshrc_content = fs::read_to_string(&zsh_config)?;
    if !zshrc_content.contains(".local/share/zsh/site-functions") {
        let mut file = OpenOptions::new().append(true).open(&zsh_config)?;
        writeln!(file, "\n# larpshell autocomplete")?;
        writeln!(file, "fpath=(~/.local/share/zsh/site-functions $fpath)")?;
        writeln!(file, "autoload -Uz compinit && compinit")?;
    }

    Ok(true)
}

fn setup_fish_autocomplete() -> Result<bool, LarpshellError> {
    let home = home_dir();
    let fish_config_dir = home.join(".config/fish");
    if !fish_config_dir.exists() {
        return Ok(false);
    }

    let completion_dir = home.join(".config/fish/completions");
    let completion_path = completion_dir.join("larpshell.fish");

    if completion_path.exists() {
        return Ok(false); // already handled by verify_and_fix_fish_autocomplete
    }

    fs::create_dir_all(&completion_dir)?;

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&completion_path)?;

    writeln!(file, "{}", generate_fish_autocomplete())?;

    Ok(true)
}

fn remove_file_if_exists(path: &std::path::Path) -> Result<bool, LarpshellError> {
    if path.exists() {
        fs::remove_file(path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn remove_bash_autocomplete() -> Result<bool, LarpshellError> {
    let home = home_dir();
    remove_file_if_exists(&home.join(".local/share/bash-completion/completions/larpshell"))
}

fn remove_zsh_completion_file() -> Result<bool, LarpshellError> {
    let home = home_dir();
    remove_file_if_exists(&home.join(".local/share/zsh/site-functions/_larpshell"))
}

fn remove_zsh_fpath_block(zshrc: &std::path::Path, marker: &str) -> Result<bool, LarpshellError> {
    if !zshrc.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(zshrc)?;
    if !content.contains(marker) {
        return Ok(false);
    }

    let lines: Vec<&str> = content.lines().collect();
    let mut new_lines = Vec::new();
    let mut skip = false;
    let mut removed = false;

    for line in lines {
        if line.trim() == marker {
            skip = true;
            removed = true;
            continue;
        }
        if skip {
            if line.contains(".local/share/zsh/site-functions")
                || line.contains("autoload -Uz compinit")
            {
                continue;
            }
            skip = false;
        }
        new_lines.push(line);
    }

    if removed {
        while new_lines.last().is_some_and(|l| l.trim().is_empty()) {
            new_lines.pop();
        }
        fs::write(zshrc, new_lines.join("\n") + "\n")?;
    }

    Ok(removed)
}

fn remove_zsh_fpath() -> Result<bool, LarpshellError> {
    let home = home_dir();
    remove_zsh_fpath_block(&home.join(".zshrc"), "# larpshell autocomplete")
}

fn remove_zsh_autocomplete() -> Result<bool, LarpshellError> {
    let file_removed = remove_zsh_completion_file()?;
    let zshrc_cleaned = remove_zsh_fpath()?;
    Ok(file_removed || zshrc_cleaned)
}

fn remove_fish_autocomplete() -> Result<bool, LarpshellError> {
    let home = home_dir();
    remove_file_if_exists(&home.join(".config/fish/completions/larpshell.fish"))
}

fn remove_autocomplete() -> Result<bool, LarpshellError> {
    let bash_removed = remove_bash_autocomplete()?;
    let zsh_removed = remove_zsh_autocomplete()?;
    let fish_removed = remove_fish_autocomplete()?;
    Ok(bash_removed || zsh_removed || fish_removed)
}

pub fn remove_shell_integration() -> Result<bool, LarpshellError> {
    let bash_removed = remove_bash_integration()?;
    let fish_removed = remove_fish_integration()?;
    let autocomplete_removed = remove_autocomplete()?;
    Ok(bash_removed || fish_removed || autocomplete_removed)
}

// ── nlsh-rs → larpshell migration ──────────────────────────────────────────

/// Removes all shell artifacts left behind by the old `nlsh-rs` binary.
/// Called once at startup; safe to call when nothing is present.
pub fn migrate_nlsh_rs_shell() -> Result<bool, LarpshellError> {
    let bash = migrate_nlsh_rs_bash()?;
    let fish = migrate_nlsh_rs_fish_fn()?;
    let completions = migrate_nlsh_rs_completions()?;
    let zsh = migrate_nlsh_rs_zsh_comment()?;
    Ok(bash || fish || completions || zsh)
}

fn migrate_nlsh_rs_bash() -> Result<bool, LarpshellError> {
    let home = home_dir();
    let bashrc = home.join(".bashrc");
    if !bashrc.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(&bashrc)?;
    if !content.contains("nlsh-rs()") && !content.contains("# nlsh-rs shell integration") {
        return Ok(false);
    }
    let (new_content, found) =
        remove_marked_function_block(&content, "# nlsh-rs shell integration", "nlsh-rs()");
    if found {
        fs::write(&bashrc, new_content)?;
    }
    Ok(found)
}

fn migrate_nlsh_rs_fish_fn() -> Result<bool, LarpshellError> {
    let home = home_dir();
    remove_file_if_exists(&home.join(".config/fish/functions/nlsh-rs.fish"))
}

fn migrate_nlsh_rs_completions() -> Result<bool, LarpshellError> {
    let home = home_dir();
    let mut removed = false;
    for path in [
        home.join(".local/share/bash-completion/completions/nlsh-rs"),
        home.join(".local/share/zsh/site-functions/_nlsh-rs"),
        home.join(".config/fish/completions/nlsh-rs.fish"),
    ] {
        if path.exists() {
            fs::remove_file(&path)?;
            removed = true;
        }
    }
    Ok(removed)
}

fn migrate_nlsh_rs_zsh_comment() -> Result<bool, LarpshellError> {
    let home = home_dir();
    remove_zsh_fpath_block(&home.join(".zshrc"), "# nlsh-rs autocomplete")
}
