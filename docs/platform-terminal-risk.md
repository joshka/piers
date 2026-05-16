# Platform And Terminal Risk

Terminal behavior looked like a UI concern in early Pi work, but the history
shows it becoming a reliability concern. Input protocols, Unicode width,
Windows shells, tmux, image rendering, full-screen programs, terminal restore,
and piped stdout all affected whether the harness could be used safely.

Piers should delay rich TUI work until the event core is stable. When a TUI is
added, it should be treated as a protocol implementation with fixtures.

## Risk Areas

Major platform and terminal risks:

- Windows shell selection and process cleanup
- CRLF file edits
- Unicode paths and non-UTF-8 edge cases
- CJK and emoji display width
- IME and dead-key input
- Kitty keyboard protocol
- tmux and screen feature negotiation
- terminal restore after panic or crash
- hyperlinks and escape-sequence support
- image rendering dimensions and offsets
- stdout behavior when piped
- full-screen interactive commands launched by tools

These risks cut across UI, tools, sessions, and file editing.

## Host Responsibilities

The host should own platform facts:

- operating system
- shell path and shell kind
- terminal type
- terminal feature flags
- path encoding behavior
- line-ending defaults
- process-group support
- signal and ctrl-c behavior

Guests should receive capabilities and diagnostics, not raw assumptions about
the platform.

## Conservative Defaults

Initial defaults should be boring:

- line-oriented REPL
- no inline images
- no advanced keyboard protocol
- no terminal hyperlinks unless detected safe
- no full-screen exec mode without approval
- plain text output in non-TTY mode
- explicit shell selection in diagnostics

This keeps the kernel focused on reload and replay instead of terminal edge
cases.

## TUI Contract

When a TUI arrives, it should consume the same host event stream as JSON mode,
RPC, and the line REPL.

The TUI should not own:

- session state
- tool state
- provider state
- reload state
- compaction state

It can own:

- layout
- focus
- scrollback presentation
- local input composition
- rendering caches

This keeps UI bugs from corrupting durable history.

## Test Fixtures

Terminal fixtures should include:

- narrow terminal widths
- wide characters at wrap boundaries
- combining characters
- CJK input and extraction
- dead-key composition
- paste handling
- resize during streaming output
- tmux and screen capability fallbacks
- Windows terminal input modes
- panic while raw mode is active

The goal is not perfect terminal support immediately. The goal is a testable
boundary so terminal improvements do not leak into the harness model.

## Tool Interaction

Exec tools can create terminal risk even before a TUI exists.

The host should detect or forbid:

- commands that request raw terminal mode
- pagers waiting for input
- editors launched accidentally
- commands that do not exit when stdout is piped
- child processes left behind after abort

Interactive mode can exist later, but it should be an explicit capability with
a clear recovery path.
