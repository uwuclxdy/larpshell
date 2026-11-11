# larpshell - use terminal with natural language

<img src="media/larplarplarpsahur.png" alt="larpshell" align="right" width="260" />

Why learn shell commands or lose time googling and alt-tabbing?

[![Crates.io](https://shields.uwuclxdy.dev/crates/v/larpshell)](https://crates.io/crates/larpshell)
[![Release](https://github.com/uwuclxdy/larpshell/actions/workflows/release.yml/badge.svg)](https://github.com/uwuclxdy/larpshell/actions/workflows/release.yml)
[![License: GPL v3](https://shields.uwuclxdy.dev/badge/License-GPLv3-blue.svg)](LICENSE)

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
| **Anthropic** | Claude models via a paid API key (default `claude-sonnet-4-6`) |
| **OpenRouter** | Free models available with `openrouter/auto` (default, [free models list](https://openrouter.ai/models?q=free)) |
| **Ollama** | API key optional |
| **LM Studio** | local models at `http://localhost:1234` (default, no key) |
| **OpenAI-compatible** | any compatible API (custom base URL support) |

Each `[[providers]]` profile takes an optional `timeout_secs` that caps each request in seconds (minimum 1; `timeout_secs = 0` rejects at load; a request may legitimately spend minutes before answering, so leave it out for no cap).

## Usage

### One-shot

```text
$ larpshell show disk usage
> df -h
Run this?
↵ execute   e explain   ↑ edit   n cancel   r retry
```

### Interactive REPL

```text
$ larpshell
larpshell> show disk usage
> df -h
Run this?
↵ execute   e explain   ↑ edit   n cancel   r retry
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
```

### Edit before running

Press Arrow Up at the confirmation prompt to resend the generated command into the input editor.

### Try again

Press `r` at the confirmation prompt to resend the same request to the provider; the fresh command re-enters the same prompt.

## Agent mode

Agent mode lets the model gather context before it returns a final command.

Built-in tools:

- `read_file`
- `list_files`
- `search_files`
- `run_command`

Every tool call asks for confirmation before it runs. A failed `run_command` feeds the model its exit status plus stdout and stderr (each stream capped at 2 KiB with a truncation marker), and a zero-exit command that wrote to stderr feeds that stderr too, so the model can correct its own attempt.

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
↵ execute   e explain   ↑ edit   n cancel   r retry
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
- `/context [all|single-prompt]`
- `/explain <command>`
- `/history [on|off]`
- `/provider [<name>]`
- `/prompt [system|explain|agent|agent-safe] [show|edit|reset]`
- `/verbose [on|off]`
- `/help`
- `/quit`
- `/uninstall`

Typing `/` shows a live hint list below the prompt. Arrow Up/Down or Tab/Shift+Tab cycle through the matches (commands, then their arguments). The current match is marked in the list and shown as a dimmed suggestion after the cursor. Enter runs it.

In the REPL, `! <command>` runs a shell command directly.

## Prompt files and history

`larpshell` stores prompt templates in `~/.config/larpshell/`:

- `sys-prompt.md`
- `explain-prompt.md`
- `agent-prompt.md`
- `agent-safe-prompt.md`

Use the CLI or slash commands to show, edit, or reset them.

The default system template accepts `{git_context}`, `{project_type}` and `{dir_summary}` beside `{cwd}`, `{home}`, `{user}`, `{shell}`, `{os}` and `{request}`: the git block renders empty outside a repository, project type detects node/python/rust/go markers nearest the working directory, and the directory summary lists the working directory capped at 30 entries.

Prompt history is on by default. Toggle it with `larpshell history on|off` or `/history on|off`.

Conversation context is on by default in the REPL: each request carries the session's last 8 earlier requests and their commands (declined and failed ones marked as not executed, long ones truncated), so a followup like `i meant the tmp filesystem` resolves in context. `/context single-prompt` sends each request alone; `/context all` turns it back on.

## Shell integration

On the first interactive run without a subcommand, `larpshell` tries to install shell integration automatically.

Current support:

- bash function in `~/.bashrc`
- zsh function in `~/.zshrc`
- fish function in `~/.config/fish/functions/larpshell.fish`
- bash, zsh, and fish completions

If setup changes your shell files, restart your shell or source the updated config.

The test suite builds the binary and runs it against mock provider servers. It does not need real provider credentials.

## License

[GPL-3.0](LICENSE)
