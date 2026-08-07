//! Single source of truth for the command/argument vocabulary shared by the
//! clap CLI (`cli.rs`), the in-REPL slash commands (`slash_commands.rs`), and
//! the generated shell autocompletions (`shell_integration.rs`).
//!
//! The clap enums and the shell-completion scripts cannot be built directly
//! from these slices — clap needs real `ValueEnum` variants and the completion
//! generators emit fixed string literals. Instead, every consumer is pinned to
//! these slices by `#[cfg(test)]` guards in its own module, so adding or
//! removing a token here turns any forgotten consumer into a failing test
//! rather than silent drift.

/// Top-level subcommand names, in the order the bash first-word completion
/// lists them. Used only by the cross-module drift guards: clap derives these
/// names from its `Commands` variants and the completion scripts spell them out
/// as literals, so nothing reads this slice at runtime.
#[cfg(test)]
pub const SUBCOMMANDS: &[&str] = &["api", "agent", "history", "verbose", "prompt", "explain", "uninstall"];

/// `agent` mode toggle values, in `off, safe, on` order.
pub const AGENT_TOGGLES: &[&str] = &["off", "safe", "on"];

/// Boolean on/off toggle values, in `on, off` order (shared by `history` and
/// `verbose`).
pub const BOOL_TOGGLES: &[&str] = &["on", "off"];

/// `prompt` kind argument values.
pub const PROMPT_KINDS: &[&str] = &["system", "explain", "agent", "agent-safe"];

/// `prompt` action argument values.
pub const PROMPT_ACTIONS: &[&str] = &["show", "edit", "reset"];
