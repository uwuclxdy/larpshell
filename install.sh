#!/usr/bin/env bash

set -e

if [ ! -f "Cargo.toml" ] || ! grep -q "name = \"larpshell\"" Cargo.toml; then
    TEMP_DIR=$(mktemp -d)
    git clone https://github.com/uwuclxdy/larpshell "$TEMP_DIR/larpshell"
    cd "$TEMP_DIR/larpshell"
fi

cargo install larpshell --force --path .

rm -rf "$TEMP_DIR"

larpshell
