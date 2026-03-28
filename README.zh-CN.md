# codexresume

[English](README.md) | [简体中文](README.zh-CN.md)

## 为什么做这个

当前 `codex resume` 会受到 provider、source、cwd 等因素影响，
导致一部分 session 在 `codex resume` 里[看不到](https://dev.to/vild_da_f524590ed3ae13840/why-codex-history-disappears-after-switching-providers-and-how-i-fixed-it-f0j)。
这个工具通过直接读取本地 session 元数据来绕过问题，同时尽量保留接近原版 `codex resume` 的交互体验。

## 特性

- 默认显示来自所有 provider 的 session
- 可选 `--only-openai`，只显示内建 OpenAI provider
- 除包装器自身参数外，其余参数都会转发给 `codex resume`
- 不需要给 Codex 打补丁

当你在 TUI 里选中某个 session 并按回车后，它会执行：

```bash
codex resume <SESSION_ID> <所有转发参数>
```

## 安装

从 crates.io 安装：

```bash
cargo install codexresume
```

或者本地源码编译安装

```bash
git clone https://github.com/daquexian/codexresume
cd codexresume
cargo install --path .
```

## 使用方法

如果已经安装，可以直接执行：

```bash
codexresume --yolo
codexresume --only-openai --yolo
codexresume --codex-home ~/.codex --sqlite-home ~/.codex --yolo
codexresume -C /path/to/project --model gpt-5.4
codexresume --dangerously-bypass-approvals-and-sandbox
```

## 备注

- `--last`、`--all`、`--include-non-interactive` 会被拒绝，因为
  `codexresume` 始终使用自己的交互式 picker。
- `--only-openai` 是 `codexresume` 自己的参数，不会转发给 `codex resume`。
- `--codex-home` 和 `--sqlite-home` 也是 `codexresume` 自己的参数，
  只用于本地 session 发现。
- `codexresume` 会尽量贴近原版 `codex resume` 的体验，但它并没有直接复用
  Codex 内部 UI 组件，所以不会做到像素级一致。
- `codexresume` 不依赖 Codex 的 Rust crate，而是直接读取 `config.toml`、
  `session_index.jsonl` 和 `state_*.sqlite`。
- 如果 session index 里有线程名，`codexresume` 会尽量把名字补出来。
