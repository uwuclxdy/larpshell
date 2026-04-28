# larpshell - use terminal with natural language

<img src="media/larplarplarpsahur.png" alt="larpshell" align="right" width="260" />

Why learn shell commands when you can just larp ts? Takuji and larptok approved.

[![Crates.io](https://img.shields.io/crates/v/larpshell)](https://crates.io/crates/larpshell)
[![Release](https://github.com/uwuclxdy/larpshell/actions/workflows/release.yml/badge.svg)](https://github.com/uwuclxdy/larpshell/actions/workflows/release.yml)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)

---

`larpshell` translates natural language into shell commands using AI, then executes them in your terminal (not without asking for confirmation of course). You can also edit the commands before running, or ask for an explanation of what they do. Both prompt templates for command generation and explanation are customizable.

## Usage Demo

![asciicast](media/larp.gif)

>[!IMPORTANT]
> Review every generated command before you run it. `larpshell` asks first, but the command still executes on your machine.

## What it does

- one-shot mode: `larpshell show disk usage`
- REPL mode: `larpshell`
- stdin mode: `echo "show disk usage" | larpshell`
- command explanation: `larpshell explain df -h`
- editable prompts for generation, explanation, and agent mode
- optional agent mode with built-in tools and MCP servers

## Install

### Requirements

[Rust Language](https://www.rust-lang.org/tools/install)

From crates.io (**recommended**):

```bash
cargo install larpshell
```

From source:

```bash
curl -sSL https://raw.githubusercontent.com/uwuclxdy/larpshell/mommy/install.sh | sh
```

### Packaged

#### Arch Linux

From [AUR](https://aur.archlinux.org/packages/larpshell) (bin; latest release, no Rust needed): `yay -S larpshell`

From [AUR](https://aur.archlinux.org/packages/larpshell-git) (git; latest commit): `yay -S larpshell-git`

## Configure a provider

Run:

```bash
larpshell api
```

`larpshell` stores config in `~/.config/larpshell/config.toml`.

### Supported providers

| *Provider* | *Notes* |
|----------|-------|
| **Gemini** | Free API key at [aistudio.google.com/apikey](https://aistudio.google.com/apikey) |
| **OpenRouter** | Free models available with `openrouter/auto` (default, [free models list](https://openrouter.ai/models?q=free)) |
| **Ollama** | API key optional |
| **OpenAI-compatible** | any compatible API (custom base URL support) |

## Usage

### One-shot

```text
$ larpshell show disk usage
> df -h
Run this?
[Y/Enter] to execute, [E] to explain, [Arrow Up] to edit, [N] to cancel
```

### Interactive REPL

```text
$ larpshell
larpshell> show disk usage
> df -h
Run this?
[Y/Enter] to execute, [E] to explain, [Arrow Up] to edit, [N] to cancel
```

### Stdin / pipe

```bash
echo "show disk usage" | larpshell
```

If piped input starts with `/`, `larpshell` treats it as a slash command.

### Explain a command

```text
$ larpshell explain df -h
> df -h
✅ Displays free disk space of mounted filesystems in a human readable format.
Run this?
[Y/Enter] to execute, [Arrow Up] to edit, [N] to cancel
```

### Edit before running

Press Arrow Up at the confirmation prompt to resend the generated command into the input editor.

## Agent mode

Agent mode lets the model gather context before it returns a final command.

Built-in tools:

- `read_file`
- `list_files`
- `search_files`
- `run_command`

Every tool call asks for confirmation before it runs.

Modes:

- `larpshell agent off` disables agent mode
- `larpshell agent safe` enables restricted tools
- `larpshell agent on` enables unrestricted agent tools

Safe mode keeps `run_command` on a read-only allowlist and blocks dangerous flags, shell metacharacters, and mutating git subcommands.

Example:

```text
$ larpshell what's the largest file in this repo
  tool Allow listing files in .?
  result (14 lines)
  tool Allow running du -ah --max-depth=2?
  result (28 lines)
> du -ah . | sort -rh | head -n 1
Run this?
[Y/Enter] to execute, [E] to explain, [Arrow Up] to edit, [N] to cancel
```

### MCP servers

`larpshell` loads MCP servers from `~/.config/larpshell/mcp.json`.

```json
{
  "mcpServers": {
    "git": {
      "command": "mcp-server-git",
      "args": ["--repository", "."]
    }
  }
}
```

## Slash commands

Interactive mode supports:

- `/api`
- `/agent [off|safe|on]`
- `/explain <command>`
- `/history [on|off]`
- `/prompt [system|explain|agent|agent-safe] [show|edit|reset]`
- `/help`
- `/quit`
- `/uninstall`

In the REPL, `! <command>` runs a shell command directly.

## Prompt files and history

`larpshell` stores prompt templates in `~/.config/larpshell/`:

- `sys-prompt.md`
- `explain-prompt.md`
- `agent-prompt.md`
- `agent-safe-prompt.md`

Use the CLI or slash commands to show, edit, or reset them.

Prompt history is on by default. Toggle it with `larpshell history on|off` or `/history on|off`.

## Shell integration

On the first interactive run without a subcommand, `larpshell` tries to install shell integration automatically.

Current support:

- bash function in `~/.bashrc`
- fish function in `~/.config/fish/functions/larpshell.fish`
- bash, zsh, and fish completions

If setup changes your shell files, restart your shell or source the updated config.

The test suite builds the binary and runs it against mock provider servers. It does not need real provider credentials.

## License

[GPL-3.0](LICENSE)
