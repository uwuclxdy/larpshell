# larpshell

Use terminal with natural language.

[![Crates.io](https://img.shields.io/crates/v/larpshell)](https://crates.io/crates/larpshell)
[![Release](https://github.com/uwuclxdy/larpshell/actions/workflows/release.yml/badge.svg)](https://github.com/uwuclxdy/larpshell/actions/workflows/release.yml)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)

---

`larpshell` translates natural language into shell commands using AI, then executes them in your terminal (not without asking for confirmation of course). You can also edit the commands before running, or ask for an explanation of what they do. Both prompt templates for command generation and explanation are customizable.

## Usage Demo

[![asciicast](https://asciinema.org/a/z2Q3GNeVJubnNx0M.svg)](https://asciinema.org/a/z2Q3GNeVJubnNx0M)

>[!IMPORTANT]
> **Always review generated commands before running them!** larpshell makes this easy with editing and explanation features, but you own what executes on your machine.

## Requirements

[Rust Language](https://www.rust-lang.org/tools/install)

## Installation

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

## Setup

Configure your AI provider:

```bash
larpshell api
```

Pick a provider and enter credentials. Config is saved in `~/.config/larpshell/config.toml`.

### Supported providers

| *Provider* | *About* |
|----------|-------|
| **Gemini** | Free API key at [aistudio.google.com/apikey](https://aistudio.google.com/apikey) |
| **OpenRouter** | Free models available with `openrouter/auto` (default, [free models list](https://openrouter.ai/models?q=free)) |
| **Ollama** | API key optional |
| **OpenAI-compatible** | any compatible API (custom base URL support) |

## Usage

### Single command

```
$ larpshell show disk usage
> df -h
Run this?
[Y/Enter] to execute, [E] to explain, [Arrow Up] to edit, [N] to cancel
```

### Interactive mode

```
$ larpshell
larpshell> show disk usage
> df -h
Run this?
[Y/Enter] to execute, [E] to explain, [Arrow Up] to edit, [N] to cancel
```

### Explain a command

```
$ larpshell explain df -h
> df -h
✅ Displays free disk space of mounted filesystems in a human readable format.
Run this?
[Y/Enter] to execute, [Arrow Up] to edit, [N] to cancel
```

### Edit before running

Press Arrow Up at the confirmation prompt. Full cursor control with left/right, home/end, backspace/delete.

```
> df -h --total▉
[Enter] to confirm, [Ctrl+C] to quit
```

### Agent mode

Let the LLM gather context before producing a command. It can call built-in tools (`read_file`, `list_files`, `search_files`, `run_command`) and any [MCP](https://modelcontextprotocol.io) servers you configure. Every tool call asks for confirmation before it runs.

Toggle it on:

```bash
larpshell agent on    # or `/agent` in the REPL
```

Then ask as usual:

```
$ larpshell what's the largest file in this repo
  tool Allow listing files in .?
  result (14 lines)
  tool Allow running du -ah --max-depth=2?
  result (28 lines)
> du -ah . | sort -rh | head -n 1
Run this?
[Y/Enter] to execute, [E] to explain, [Arrow Up] to edit, [N] to cancel
```

MCP servers go in `~/.config/larpshell/mcp.json` using the standard `mcpServers` format:

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

### Subcommands

| Command | Description |
|---------|-------------|
| `larpshell api` | Configure AI provider |
| `larpshell agent [on\|off]` | Toggle agent mode (tools + MCP) |
| `larpshell explain <cmd>` | Explain a command with safety rating |
| `larpshell prompt [system\|explain] [show\|edit]` | View or edit prompt templates |
| `larpshell uninstall` | Remove larpshell |
| `larpshell --help` | Show help |

## How it works

1. You describe what you want in plain language
2. An AI provider translates your request into a shell command
3. You review the command, edit it if needed, or ask for an explanation
4. Press Enter to run it in your shell

## License

[GPL-3.0](LICENSE)
