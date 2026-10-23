# Backup file format

Notebook writes encrypted backups of the whole notebook into a folder the user
chooses, usually one that Dropbox, iCloud Drive, OneDrive, or Syncthing
uploads. Each backup is one self-contained file that restores on any computer
with the recovery key. The code is `notebook_core::backup`.

## File names

```
notebook-backup-YYYYMMDD-HHMMSS.nbbackup
```

The stamp is the creation time in UTC. If that name is taken, ` 2`, ` 3`, and
so on go before the extension. Only names matching
`^notebook-backup-\d{8}-\d{6}( \d+)?\.nbbackup$` are treated as backups for
retention, so a renamed backup or a sync client's "conflicted copy" is never
deleted. Names and headers aren't authenticated, so retention never deletes
the backup it just wrote, and it ignores files stamped more than a day in the
future, which would otherwise sort as newest and push out real backups.

A backup is first written as a hidden `.notebook-backup-<random>.partial` file
in the same folder, flushed to disk, and then hard-linked to its final name,
which never replaces an existing file (on drives without hard links, such as
FAT and exFAT, it is renamed into a name that was free). Anything under a
backup name is complete. A sync client may upload the hidden partial file
too; it is the same ciphertext and is deleted at the next backup. A partial file left by a crash is deleted at the next
backup.

## Layout

All integers are big-endian.

```
offset  size  field
0       8     magic "NBBACKUP"
8       1     format version (1)
9       16    vault ID (binary ULID)
25      2     recovery entry length r (u16)
27      r     recovery entry
27+r    …     payload envelope (rest of the file)
```

The first `27 + r` bytes are the **header**. Today `r` is 91.

### Recovery entry

```
offset  size  field
0       1     entry version (1)
1       16    vault ID (same as the header's)
17      74    envelope: the 32-byte vault key, encrypted
```

The envelope key is HKDF-SHA256 with the recovery key's 16 random bytes as
input key material, the vault ID's 16 bytes as salt, and the info string
`notebook/v1/kek/recovery`, expanded to 32 bytes. The envelope kind is 33, the
same wrap a key bundle's recovery entry uses.

### Envelopes

Every ciphertext uses the vault-wide envelope:

```
offset  size  field
0       1     envelope version (1)
1       1     algorithm (1 = XChaCha20-Poly1305)
2       24    random nonce
26      n+16  ciphertext and Poly1305 tag
```

The associated data is `"notebook/v1/aad" || version || algorithm ||
vault ID || kind || note ID`, with an all-zero note ID for vault-level kinds.
A ciphertext only opens with the same vault, kind, and note.

### Payload

The payload is one envelope under the **vault key**, kind 48, bound to the
vault ID. Its plaintext is:

```
size  field
32    SHA-256 of the header bytes (offsets 0 to 27+r)
8     created_at (Unix milliseconds, i64)
4     note count
then, for each note:
16    note ID (binary ULID)
2     wrapped key length k (u16)
k     wrapped note key (envelope under the vault key, kind 16, bound to the note)
4     snapshot length s (u32)
s     encrypted snapshot (envelope under the note key, kind 3, bound to the note)
```

Nothing may follow the last note, and note IDs are unique. Each snapshot
holds the note's full CRDT state: a format byte (1) followed by a Yrs/Yjs v1
full-state update. The document layout is described in
`notebook_core::document`.

## What is visible without the key

- the format version
- the vault ID
- the recovery entry, which opens only with the recovery key
- the file's size and modification time

Note IDs, the number of notes, their sizes, titles, tags, and content are
all inside the payload.

## Integrity

- The payload's tag authenticates the vault ID and kind.
- The header hash inside the payload makes the cleartext header
  tamper-evident too, so changing any byte of the file, truncating it, or
  appending to it makes it fail to open.
- A header whose vault ID doesn't match its recovery entry is damaged.
- An unknown magic means "not a Notebook backup". A version above 1 means
  "needs a newer version of Notebook", which is distinct from damage.

## Restoring

1. Read the header. If its vault ID is the vault open on this device, use the
   device's vault key. Otherwise, parse the recovery key the user enters (a
   checksum failure means it was mistyped), and open the recovery entry with
   it. A decryption failure here means the key belongs to another notebook.
2. Open the payload with the vault key, check the header hash, and parse it.
3. Verify every note: unwrap its key with the vault key, load its snapshot
   into an empty document, and check it yields a note. Stop on any failure.
   Nothing on the device has changed yet.
4. Build a fresh store and import every note as its wrapped key plus base
   snapshot. Store the recovery entry beside it.
5. Swap the fresh store in for the device's store, and save the vault key to
   the OS keychain.

Without the app, steps 1 to 3 are enough to read a backup: derive the
recovery wrapping key, unwrap the vault key, open the payload, unwrap each
note key, and decrypt each snapshot to get its Yjs update.

## Versioning

This is format version 1. A change to the layout bumps the version byte;
older apps then report that a newer version is needed instead of misreading
the file.
