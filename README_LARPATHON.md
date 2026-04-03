# `larpshell` - LARP using the terminal

We, yes WE as in all of US, the skids of israel, just love using AI, so much so that we simply must spread the slop everywhere, even into the terminals *insert emdash* as we lack a working brain to learn all those shell commands 😵‍💫. That's why `larpshell` is here to save the day as it does exactly that!! (and it also works with root access (i think), how wonderful is that!)

<details>
<summary><strong>Description for normies</strong></summary>

With `larpshell`, you can type what you want to do in your natural language (if you dont remember the command or too lazy to google), and let an LLM translate that into a shell command, then **run it without the hastle of ctrl+c and ctrl+v** :D. For example, you can say "show me the disk usage" and `larpshell` will show you `df -h`). Don't worry, it will ask for your confirmation before executing the command.

</details>

>[!IMPORTANT]
> Regardless if you're the biggest skiddie ever, random larper or just a lazy bum; **ALWAYS review the generated commands before running them**. larpshell tries to make this as easy as possible with an option to manually edit commands before execution.

Oh yea, one more thing, this was entirely Claude's work, I didn't write a single line of code, don't you even worry 😂✌️

### Preview

[![asciicast](https://asciinema.org/a/z2Q3GNeVJubnNx0M.svg)](https://asciinema.org/a/z2Q3GNeVJubnNx0M)

## Installation

### Requirements

1. [Rust](https://www.rust-lang.org/tools/install)

---

from crates.io **(recommended)**:
```bash
cargo install larpshell
```

from source, latest commit:
```bash
curl -sSL https://raw.githubusercontent.com/uwuclxdy/larpshell/main/install.sh | sh
```

## Setup

### Configure AI provider

```bash
larpshell api
```

Select provider and enter credentials. Config is stored in `~/.config/larpshell/config.toml`.

## Supported Providers

- **Gemini [FREE]** - free access to the group (gemini) fleshlight at [aistudio.google.com/apikey](https://aistudio.google.com/apikey)
- **OpenRouter [FREE]** - access to free models with `openrouter/auto`
- **Ollama** - local models
- **OpenAI-Compatible APIs** - chatgpt or compatible APIs (LMStudio, Groq, etc.)

> You can get free OpenRouter model access at https://openrouter.ai/models?q=free

## Usage

```
larpshell show disk usage
$ df -h
Run this?
[Y/Enter] to execute, [E] to explain, [Arrow Up] to edit, [N] to cancel
```

Interactive mode:
```
larpshell
larpshell> show disk usage
$ df -h
Run this?
[Y/Enter] to execute, [E] to explain, [Arrow Up] to edit, [N] to cancel
```

Explain command:
```
larpshell explain df -h
$ df -h
✅ Displays free disk space of mounted filesystems in a human readable format.
Run this?
[Y/Enter] to execute, [Arrow Up] to edit, [N] to cancel
```

Edit generated commands before running:
```
$ df -h --total▉
[Enter] to confirm, [Ctrl+C] to quit
```

**subcommands:**
- `--help` - show help
- `api` - configure API provider
- `uninstall` - remove larpshell
- `prompt` - show/edit the prompt templates
- `explain` - explain a command

## How it works

1. translates natural language to shell commands using AI
2. asks for confirmation
3. command runs in parent shell and appears in history

## Credits

my favoritest company of them all: Anthropic!! thank you for the love of my life ⸺ Claude ❤️❤️❤️❤️❤️❤️mmmmmwagh<3
