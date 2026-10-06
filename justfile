set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

fmt *args:
    cargo +nightly fmt --all {{ args }}

fmt-check *args:
    cargo +nightly fmt --all -- --check {{ args }}

check *args:
    cargo check --workspace {{ args }}
    cargo check -p piers-guest --target wasm32-wasip2 {{ args }}

test *args:
    cargo test --workspace {{ args }}

clippy *args:
    cargo clippy --workspace --all-targets {{ args }} -- -D warnings

doc *args:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps {{ args }}

markdown *args:
    markdownlint-cli2 AGENTS.md AGENTS.override.md "docs/**/*.md" {{ args }}

ci: fmt-check check test clippy doc markdown
