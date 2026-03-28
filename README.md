# codexresume

[English](README.md) | [简体中文](README.zh-CN.md)

## Why this exists

Current `codex resume` can be affected by provider, source, cwd, and other
factors, which can make some sessions [disappear from the history
picker](https://dev.to/vild_da_f524590ed3ae13840/why-codex-history-disappears-after-switching-providers-and-how-i-fixed-it-f0j).
This tool works around that by reading local session metadata directly, while
trying to keep an experience close to the stock `codex resume` flow.

## Features

- Shows sessions from all providers by default
- Optional `--only-openai` flag for the built-in OpenAI provider
- Forwards all non-wrapper arguments to `codex resume`
- Does not require patching Codex source

After you pick a session and press `Enter`, it `exec`s:

```bash
codex resume <SESSION_ID> <all forwarded args>
```

## Installation

Install from crates.io:

```bash
cargo install codexresume
```

Or clone and install from source:

```bash
git clone https://github.com/daquexian/codexresume
cd codexresume
cargo install --path .
```

## Usage

If installed, run it directly:

```bash
codexresume --yolo
codexresume --only-openai --yolo
codexresume --codex-home ~/.codex --sqlite-home ~/.codex --yolo
codexresume -C /path/to/project --model gpt-5.4
codexresume --dangerously-bypass-approvals-and-sandbox
```

## Notes

- `--last`, `--all`, and `--include-non-interactive` are rejected because
  `codexresume` always opens the interactive picker.
- `--only-openai` is a `codexresume` flag. It is consumed by the
  wrapper and is not forwarded to `codex resume`.
- `--codex-home` and `--sqlite-home` are `codexresume` flags. They are used
  only for local session discovery.
- `codexresume` tries to stay close to the stock `codex resume` experience,
  but it does not directly reuse Codex's internal UI components, so it is not
  pixel-identical.
- `codexresume` does not depend on Codex Rust crates. It reads `config.toml`,
  `session_index.jsonl`, and `state_*.sqlite` directly.
- If session names are available in the session index, `codexresume` will try
  to surface them.
