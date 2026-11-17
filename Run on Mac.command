#!/bin/bash
# Microslop Flight Simulator on a Mac: builds the game the first time
# (several minutes), then starts it. Double-click this file, or run it
# from Terminal.
cd "$(dirname "$0")"
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust is not installed yet."
  echo "Install it from https://rustup.rs (one command in Terminal),"
  echo "then open this file again."
  read -r -p "Press Return to close."
  exit 1
fi
cargo run --release
status=$?
if [ $status -ne 0 ]; then
  echo
  echo "The game stopped with an error (see above)."
  read -r -p "Press Return to close."
fi
