# idea-git-tool

A desktop Git client with IDEA-style staging, built with Tauri 2 (Rust + git2)
and React 19 + TypeScript.

## Features

- **Status & staging** — working-tree / index diff with per-file and per-directory
  checkboxes for stage, unstage, and discard.
- **Merge & conflicts** — three-way merge, conflict resolution (accept ours /
  theirs, or edit inline), and abort-merge.
- **History** — commit log with an expandable per-commit file tree and inline diffs.
- **Branches & remotes** — create / delete / checkout branches, plus push / fetch /
  pull.
- **Inline diff** — IDEA-style split diff viewer using UTF-16-safe char-level diff
  ranges, and a text editor for editing conflict files in place.

## Development

Tauri 2 + Vite + React 19 + TypeScript.

```bash
npm install
npm run tauri dev   # run the desktop app
npm run build       # type-check and build the frontend
```

## Layout

- `src/` — React frontend; `App.tsx` is the main view.
- `src-tauri/` — Rust backend; Tauri commands wrapping git2 (see `src-tauri/src/lib.rs`).

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
