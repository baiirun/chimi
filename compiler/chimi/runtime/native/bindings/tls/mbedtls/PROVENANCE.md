# mbedTLS, vendored for std/tls

- **Version.** mbedTLS 4.1.0, an LTS release supported to March 2029. It bundles TF-PSA-Crypto 1.1.0 under `tf-psa-crypto/`.
- **Source.** The release tarball `mbedtls-4.1.0.tar.bz2` from https://github.com/Mbed-TLS/mbedtls/releases/tag/mbedtls-4.1.0.
- **Checksum.** sha256 `377a09cf8eb81b5fb2707045e5522d5489d3309fed5006c9874e60558fc81d10`, as the release notes give it.
- **Licence.** Apache-2.0 OR GPL-2.0-or-later. Chimi uses it under Apache-2.0. See `LICENSE` and `tf-psa-crypto/LICENSE`.
- **What is here.** The `.c` and `.h` files the default configuration compiles, at their upstream paths and unmodified. `MANIFEST.sha256` lists every file's checksum, and a test checks the tree against it.
- **Configuration.** The upstream defaults (`include/mbedtls/mbedtls_config.h` and `tf-psa-crypto/include/psa/crypto_config.h`). The include paths are set in `BINDING_PACKAGE_FLAGS` (`src/backend/runtime_archive_key.rs`), not by editing these files.
- **Updating.** Run `scripts/vendor-mbedtls.sh <tarball>` after changing the version and checksum at its top. Then update this file and run `cargo test --test tls`.
