# Vendored yrs

- **Source:** `yrs` 0.28.0 from crates.io (<https://github.com/y-crdt/y-crdt>).
- **License:** MIT. Authors: Kevin Jahns, Bartosz Sypytkowski.
- **Wiring:** patched in through `[patch.crates-io]` in the root `Cargo.toml`, and excluded from the workspace.
- **Removed:** `benches/` and the dev-dependencies, which notebook doesn't build. `Cargo.toml` was rewritten by hand to match.

Notebook applies updates from other devices, and only key holders can produce one that decrypts. A crafted or corrupt update reached panics and memory corruption in Yrs 0.28. `notebook-core`'s structural check (`crates/notebook-core/src/document/validate.rs`) rejects most of those shapes. These patches fix the integration bugs a stateless check can't rule out.

Every change is marked `NOTEBOOK PATCH` in the source.

## `src/update.rs`

### `Update::integrate`: out-of-order blocks
- **Bug:** a block whose clock is below the client's local clock was integrated unconditionally. `BlockStore::push` then overwrote whatever block held that clock. When that was a live item rather than a Skip, its neighbours kept pointers to the freed item: a use-after-free, seen as SIGSEGV in release and as text silently vanishing.
- **Trigger:** one client in two block sections with clocks going backwards.
- **Fix:** the block is now integrated only if it fits inside one integrated Skip (`BlockStore::fits_in_skip`). Otherwise it is dropped before anything links to it.

### `Update::integrate`: offset overflow
- **Bug:** the clock offset was computed in `i32`, so it overflowed for clocks at or above 2^31.
- **Fix:** it is now computed in `i64`, and the local clock advance saturates.

### `BlockSet::exclude`: empty block list
- **Bug:** it unwrapped `front()`/`back()`, which panicked when decoding had left a client with an empty block list (zero-length items are dropped).
- **Fix:** such a client is now skipped.

## `src/transaction.rs`

### `TransactionMut::apply_update`: held updates stuck after a skip fills
- **Bug:** held (pending) updates were retried only when a missing client's clock advanced.
  - Yrs integrates a block that arrives ahead of its client's clock early, if its own dependencies exist, and leaves a Skip for the gap.
  - The update that later fills that Skip satisfies the held dependency without moving the clock. So the held update was never retried, and replicas that saw the same updates in different orders disagreed.
- **Trigger:** ordinary out-of-order delivery of legitimate updates. Pinned by the `held_update_integrates_when_a_skip_fills` and `shuffled_delivery_converges` tests.
- **Fix:** the held update is now also retried whenever the incoming update brought new blocks. This can't loop: the pending update is taken out before it's re-applied.

## `src/block.rs`

### `Block::splice`: neighbour links on unintegrated items
- **Bug:** splitting an unintegrated update item (in `BlockSet::exclude`, `merge`, or `merge_updates`) went through `ItemPtr::splice`, which links the two halves as neighbours.
  - When one half was then dropped (the known half in `exclude`, or a slice in merging), the other kept a dangling `left`/`right`.
  - Integration then read it: a heap-use-after-free under AddressSanitizer.
- **Trigger:** a legitimate update arriving after a crafted GC-only update for the same client.
- **Found by:** coverage-guided fuzzing (since removed); pinned by the `legitimate_update_over_a_foreign_skip` test.
- **Fix:** both links are now cleared after the split. Unintegrated items carry no neighbour links; integration repairs them from their origins.

## `src/block_store.rs`

### `BlockStore::fits_in_skip`
New helper for the guard above.

### `BlockStore::push`
Asserts that a block replacing a Skip fits inside it. Any path that bypasses the guard now panics instead of corrupting memory.

## Upstream

All of the above should be reported to y-crdt. When upstream fixes these, drop this directory and the `[patch.crates-io]` entry, then re-run the `notebook-core` tests before relying on the release.
