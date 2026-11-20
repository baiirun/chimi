# SQLite, vendored for std/sqlite

- **Version.** SQLite 3.54.1, the amalgamation.
- **Source.** `sqlite-amalgamation-3540100.zip` from https://www.sqlite.org/2026/sqlite-amalgamation-3540100.zip.
- **Checksum.** SHA3-256 `92281c86e2957f9b9305890f9ac04204caa7b677cc868dc326d942689df28e30`, as https://www.sqlite.org/download.html gives it (sha256 of the zip: `0a9c8e34352dad46006aa82836d5383bff9dc6bc9ce5a60553c05aa30e6ad1f9`).
- **Licence.** Public domain (https://www.sqlite.org/copyright.html).
- **What is here.** `sqlite3.c` and `sqlite3.h`, unmodified. The shell and the extension header are left out. `MANIFEST.sha256` lists each file's checksum, and `tests/sqlite.rs` checks the tree against it.
- **Configuration.** The compile options are in `BINDING_PACKAGE_FLAGS` (`src/backend/runtime_archive_key.rs`), not edits to these files: thread-safe (serialized), no double-quoted string literals, no memory statistics, LIKE never matches a blob, no deprecated interfaces, no loadable extensions, no shared cache, and alloca for temporary buffers.
- **Updating.** Run `scripts/vendor-sqlite.sh <zip>` after changing the version and checksum at its top. Then update this file and run `cargo test --test sqlite`.
