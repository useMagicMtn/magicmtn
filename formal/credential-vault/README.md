# Credential vault model

Run `python3 scripts/check-credential-vault.py --mutation-checks` with Java 11 or later. The runner downloads and verifies the pinned TLC release. The mutation checks require TLC to find counterexamples for deliberately weakened guards.

The model follows `PostgresKmsVault` in `core/src/credential_vault/mod.rs`. `Read` represents the lookup shared by reads and metadata. `Begin` models the preflight check for replacement or destruction, and `Finish` models the final conditional SQL update. Two workers can interleave those steps. The configuration covers one credential ID, all four scope fields, organization-owned and personal credentials, and four versions.

A successful lookup or write must match the live row's complete scope. A successful write must use the current version and advance it by exactly one. Destruction clears the row's envelope and retains the tombstone. The model assumes a unique database constraint on the credential ID and treats SQL statements as atomic.

The model checks the storage lifecycle. It treats the encrypted envelope as an opaque value and places each read at the point where PostgreSQL selects its snapshot. A read that selected a live row may finish decrypting after a concurrent destroy.
