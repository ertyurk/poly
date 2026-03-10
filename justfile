set shell := ["zsh", "-cu"]

_default:
    @just --list

default: _default

run-trader *args:
    cargo run --bin polymarket-bot -- {{args}}

run-trader-paper *args:
    cargo run --bin polymarket-bot -- --paper-trade {{args}}

run-dashboard db="data/bot.db" port="3030" host="127.0.0.1":
    cargo run --bin dashboard -- --db {{db}} --host {{host}} --port {{port}}

run-dashboard-paper port="3031" host="127.0.0.1":
    cargo run --bin dashboard -- --db data/bot.db.paper --host {{host}} --port {{port}}

test:
    cargo test

build:
    cargo build

fmt:
    cargo fmt

clippy:
    cargo clippy --all-targets --all-features
