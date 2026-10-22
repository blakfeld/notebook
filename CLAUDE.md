# Notebook

## Definition of done

A change is not done, and must not be committed as finished, merged, or
archived, until it passes all of these:

1. Code review (the `code-reviewer` agent or `/code-review`), with its
   findings fixed or explicitly deferred by the user.
2. `cargo fmt --all --check`
3. `cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo clippy -p notebook-core --target wasm32-unknown-unknown --features transport-wasm -- -D warnings`

These match CI. Run them locally after the last edit, not before it; any
fix made after review means running fmt and clippy again.

On macOS, native builds need
`SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk`.
