# Proposal

## Why

A note must be editable on any client (native app, browser, mobile), sometimes concurrently or offline, without losing edits. The server can't merge content because it can only see ciphertext. Merging therefore has to happen on clients, using a CRDT that combines updates deterministically. Each note becomes a CRDT document whose updates and snapshots are encrypted before leaving the device.

## What Changes

- Represent each note as a CRDT document in `notebook-core` using Yrs, which shares a wire format with Yjs so it can interoperate with `y-codemirror.next` in the editor. The design records why Yrs was chosen over Loro.
- Document contents: the markdown body as shared text, plus note metadata (pinned/archived/trashed, timestamps) as a CRDT map.
- Produce incremental **updates** from local edits and apply remote updates, with convergence guaranteed regardless of order or duplication.
- Produce full-state **snapshots** for compaction and fast bootstrap. A snapshot plus later updates reproduces the current state.
- Encrypt every update and snapshot with the note's key, using the `vault-crypto` envelope, before it's handed to storage or sync.
- Expose a materialized view (current markdown body and metadata) for the tag parser, search, and export.

## Capabilities

### New Capabilities
- `note-documents`: Per-note CRDT document representation, update generation and application, convergence guarantees, snapshotting, and encryption of updates and snapshots.

### Modified Capabilities
<!-- None. -->

## Impact

- `notebook-core`: new `document` module. Depends on the `notes` and `vault-crypto` capabilities.
- `notes`: a crate-internal constructor so a document can rebuild a `Note` with its stored timestamps and flags. Note behavior is unchanged.
- Dependencies: `yrs` (0.28) and `dissimilar` (character diff for whole-body sets), both of which must build for `wasm32-unknown-unknown`.
- Out of scope: the plaintext editor exchange between the core and the open editor. `note-editor-component` adds it to this capability, and it never feeds storage or sync.
- Sets the data format persisted by local storage and transmitted by sync, so changing it later is costly. The design should treat it as a long-lived format.
