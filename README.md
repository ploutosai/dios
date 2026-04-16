# Dios

Very experimental editor made exclusively by the AI. 
Built with miniquad, [blitz](https://github.com/DioxusLabs/blitz) and [dioxus](https://github.com/DioxusLabs/dioxus).
Editor text is represented as Dixous/HTML div rows styled with CSS and rendered into a native miniquad surface. No browser, no javascript, just pure rust all the way through!

This started as a little benchmark for blitz/dioxus, but somehow it became my personal daily driver rust IDE.

## Features

### projectile-style navigation

Fuzzy search within a project and projectile-like buffer<->project association. 

<img width="500" alt="proj" src="https://github.com/user-attachments/assets/9516f340-6ad3-479b-a478-599352adfaf9" />

### LSP navigation/autocomplete

<img width="800" height="464" alt="lsp" src="https://github.com/user-attachments/assets/baf469e5-c1e0-448e-a10a-583eb75e623e" />

### Emacs-style tab button

<img width="500" alt="tab" src="https://github.com/user-attachments/assets/9dd9183d-f3a3-49e0-8b85-1e41d1061cd1" />

### Emacs-style undo tree

<img width="500" alt="undotree" src="https://github.com/user-attachments/assets/e40ca675-9b17-4fab-81d3-b1e94fcfe2f8" />

### ripgrep/compilation errors buffer

<img width="500" alt="compilation" src="https://github.com/user-attachments/assets/74ccfb5a-e787-4d05-9260-a78bb49152d8" />

### Web build

LSP/compilation/ripgrep are disabled on wasm, but everything else works just fine: 

<img width="500" alt="web" src="https://github.com/user-attachments/assets/e815c41d-06c2-446f-a011-3ef9fd1d2293" />

It is a normal miniquad web build:

```
cargo build --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/dios.wasm web/dios.wasm
cd web && basic-http-server .
```

## Hotkeys

| Hotkey | Action |
| --- | --- |
| `Tab` | Align current Rust line, or all selected Rust lines |
| `C-a` | Select all |
| `C-c` | Copy selection |
| `C-x` | Cut selection |
| `C-v` | Paste |
| `C-z` | Undo |
| `C-S-z` / `C-y` | Redo |
| `C-g` | Cancel prefix/transient state and clear minibuffer |
| `C-f` | Start forward incremental search, twice to cycle results |
| `C-r` | Start backward incremental search, twice to cycle results |
| `C-S-f` | Ripgrep |
| `C-Space` | LSP completion |
| `C-k C-f` | Find/open/create file (native) |
| `C-k b` | Switch buffer |
| `C-k u` | Undo tree visualizer |
| `C-k r` | Revert current buffer from disk |
| `C-k f` | Run `rustfmt` on current file |
| `C-k c` | Run `cargo check` for active project |
| `C-k s` | Save current buffer (native) |
| `C-k k` | Kill/close current buffer |
| `C-k ←` | Navigate back through LSP/rg result history |
| `C-k 1` | Close right pane |
| `C-k o` | Toggle focus between editor and right pane |

## Configuration

Everything is hardcoded. Colorscheme, hotkeys, panels layout. AI prompt of a `dios` fork is a way to change how the editor behaves. Welcome to the new era of software!

## Profiling

Editor is only fast enough to be practical for me personally. On my machine it loads significantly faster than both emacs and vscode, and feels more responsive. However, AI gods do introduce perfomance regressions, and to monitor what's going on `dios` always starts [puffin](https://github.com/EmbarkStudios/puffin) server.

To connect:

```
puffin_viewer --url 127.0.0.1:8585
```

<img width="500" height="306" alt="profiling2" src="https://github.com/user-attachments/assets/7bece458-edfe-4cb4-b067-37fae2962189" />

*Cargo.lock for reasonably big file test. 4ms per frame is nothing to be overly proud of, but considering zero idle CPU/GPU usage - frames only happen on keystrokes - it is fast enough for now!*

