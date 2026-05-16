set dotenv-load := false

default:
  @just --list

build:
    cargo --locked build --all-targets --release

install:
    cargo --locked install --path crates/stacks-bench-agent

fmt:
    cargo +nightly --locked fmt --all

lint:
    RUST_LOG=warn cargo --locked clippy --all-targets -- -D warnings
    cargo check --locked --all-targets
    cargo +nightly --locked fmt --all -- --check

fix:
    RUST_LOG=warn cargo --locked clippy --fix --all-targets --allow-dirty
    cargo +nightly --locked fmt --all

test:
  cargo --locked nextest run --workspace --no-fail-fast --all-targets