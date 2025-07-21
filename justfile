init-db:
    #!/bin/bash
    mkdir -p .ddrive
    touch .ddrive/metadata.sqlite3
    for file in migrations/*.sql; do
        echo "Running migration: $file"
        sqlite3 .ddrive/metadata.sqlite3 < "$file"
    done
    cargo sqlx prepare

fmt:
    cargo fmt --all

lint: fmt test
    cargo clippy

test:
    cargo test

install:
    SQLX_OFFLINE=true cargo install --path .

run *args:
    cargo run -- {{args}}
