# WebDAV ZIP Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add WebDAV ZIP synchronization for WorkBuddy sessions plus model/provider configuration, with smart merge, remote-over-local, and local-over-remote strategies.

**Architecture:** The backend owns sync packaging, ZIP encryption/decryption, WebDAV transport, safe import, and merge rules. The frontend exposes settings, connection test, upload/download/merge actions, and sync result summaries. ZIP packages are treated as sync snapshots; package contents are deterministic and verified with SHA-256, then encrypted before WebDAV upload because model/provider files contain API keys.

**Tech Stack:** Tauri 2, Rust 2021, React 18, TypeScript, Vite, WebDAV over HTTP(S), ZIP snapshot package.

## Global Constraints

- Do not sync `workbuddy.db`, `workbuddy.db-wal`, `workbuddy.db-shm`.
- Do not sync `.workbuddy/sessions/<pid>.json`, `.workbuddy/app/session`, `.workbuddy/app/sessions.json`, cache, credentials, or logs.
- Session history source is `.workbuddy/projects/<compressedCwd>/<sessionId>.jsonl`.
- Session metadata must be logically exported from the SQLite `sessions` table when available; MVP may export an empty metadata file if SQLite access is unavailable.
- Model config source is `.workbuddy/models.json`.
- Provider config source is `.workbuddy/model-providers.json`.
- ZIP imports must extract only inside a staging directory and reject absolute paths, drive-prefixed paths, and `..` path traversal.
- Sync strategies are exactly: `smartMerge`, `remoteOverwriteLocal`, `localOverwriteRemote`.
- Smart merge must not silently overwrite different API keys; it must keep local secret fields unless an overwrite strategy is explicitly selected.
- Remote overwrite local must create a local backup before writing session/model/provider files.
- Local overwrite remote must publish a new remote generation and keep previous remote generations.
- WebDAV settings must not be included inside sync ZIP packages.
- WebDAV remote must receive encrypted ZIP bytes (`workbuddy-sync.zip.enc`), not plaintext `workbuddy-sync.zip`.
- The encryption password/passphrase must be supplied by the user at sync time or stored outside the ZIP package.

---

## File Structure

- Modify: `src-tauri/Cargo.toml`
  - Add ZIP, hashing, and authenticated-encryption dependencies.
- Create: `src-tauri/src/sync.rs`
  - Sync data model, package build/read, safe ZIP extraction, merge rules, and tests.
- Create: `src-tauri/src/crypto.rs`
  - Password-based key derivation and authenticated encryption for ZIP bytes.
- Create: `src-tauri/src/webdav.rs`
  - WebDAV URL handling, PROPFIND/MKCOL/PUT/GET/HEAD helpers, and tests.
- Modify: `src-tauri/src/lib.rs`
  - Register sync commands and reuse existing WorkBuddy path helpers.
- Modify: `src/tauriRuntime.ts`
  - Browser-preview stubs for new sync commands.
- Modify: `src/main.tsx`
  - Add WebDAV sync settings and action UI.
- Create: `tests/syncUiText.test.mjs`
  - Verify UI exposes the required strategies and sensitive wording.

## Task 1: Backend sync package and merge rules

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Create: `src-tauri/src/sync.rs`
- Create: `src-tauri/src/crypto.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub enum SyncStrategy { SmartMerge, RemoteOverwriteLocal, LocalOverwriteRemote }`
  - `pub fn build_sync_package(workbuddy_dir: &Path, output_zip: &Path) -> Result<SyncPackageManifest, String>`
  - `pub fn encrypt_package(zip_path: &Path, encrypted_path: &Path, passphrase: &str) -> Result<(), String>`
  - `pub fn decrypt_package(encrypted_path: &Path, zip_path: &Path, passphrase: &str) -> Result<(), String>`
  - `pub fn inspect_sync_package(zip_path: &Path) -> Result<SyncPackageManifest, String>`
  - `pub fn apply_sync_package(workbuddy_dir: &Path, zip_path: &Path, strategy: SyncStrategy) -> Result<SyncApplyResult, String>`
  - `pub fn create_sync_backup(workbuddy_dir: &Path) -> Result<PathBuf, String>`

- [ ] **Step 1: Write failing Rust tests**

Add tests in `src-tauri/src/sync.rs` for:

```rust
#[test]
fn package_includes_sessions_models_providers_and_excludes_runtime_files() { /* create temp workbuddy dir, build package, inspect manifest */ }

#[test]
fn safe_extract_rejects_zip_slip_paths() { /* construct malicious zip entry and expect error */ }

#[test]
fn smart_merge_keeps_local_provider_api_key_when_remote_differs() { /* local provider and remote provider share id but differ api_key */ }

#[test]
fn remote_overwrite_creates_backup_before_writing_models_and_providers() { /* apply remote package and assert backup directory exists */ }

#[test]
fn encrypted_package_round_trip_restores_plain_zip_bytes() { /* encrypt package with passphrase, decrypt, assert same SHA-256 */ }
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml sync::
```

Expected: tests fail because `sync` module and functions do not exist.

- [ ] **Step 3: Implement minimal package model**

Implement:

- deterministic package layout under `workbuddy-sync/`
- manifest with schema version, generation timestamp, file list, SHA-256
- encrypted upload artifact name `workbuddy-sync.zip.enc`
- session JSONL discovery under `projects/**/*.jsonl`
- model/provider inclusion
- runtime file exclusions
- safe extraction path validation

- [ ] **Step 4: Implement merge strategy core**

Implement:

- `SmartMerge`: merge providers by `id`, preserve local `api_key` if remote differs, merge models by `id`, merge session files by hash/prefix or conflict copy.
- `RemoteOverwriteLocal`: create backup, replace synced files from package, upsert config files.
- `LocalOverwriteRemote`: same package build path; transport task uses this for upload.

- [ ] **Step 5: Verify GREEN**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml sync::
```

Expected: all new sync tests pass.

## Task 2: WebDAV transport and Tauri commands

**Files:**
- Create: `src-tauri/src/webdav.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes Task 1 package functions.
- Produces commands:
  - `webdav_test_connection(settings: WebDavSyncSettings) -> Result<(), String>`
  - `webdav_upload_sync(settings: WebDavSyncSettings, strategy: SyncStrategy) -> Result<WebDavSyncResult, String>`
  - `webdav_download_sync(settings: WebDavSyncSettings, strategy: SyncStrategy) -> Result<WebDavSyncResult, String>`
  - `webdav_fetch_remote_info(settings: WebDavSyncSettings) -> Result<Option<WebDavRemoteInfo>, String>`

- [ ] **Step 1: Write failing WebDAV unit tests**

Add tests for:

```rust
#[test]
fn remote_generation_path_uses_versioned_zip_layout() { /* assert /WorkBuddySync/v1/generations/<id>/workbuddy-sync.zip */ }

#[test]
fn webdav_settings_validation_rejects_missing_url_or_credentials() { /* empty base_url / username / password */ }

#[test]
fn manifest_public_does_not_include_api_keys_or_local_paths() { /* serialize public manifest and assert no sk- or C:\\Users */ }
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml webdav::
```

Expected: tests fail because `webdav` module and commands do not exist.

- [ ] **Step 3: Implement WebDAV helpers**

Implement PROPFIND, MKCOL, PUT, GET, HEAD using `reqwest`, matching cc-switch compatibility:

- `PROPFIND Depth=0` for connection check.
- `MKCOL` per path segment.
- Treat existing-directory `405`/`409` as success only after PROPFIND confirms existence.
- Upload snapshot ZIP first, then publish `latest.json`.

- [ ] **Step 4: Register commands**

Expose commands in `tauri::generate_handler![]`.

- [ ] **Step 5: Verify GREEN**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml webdav::
```

Expected: all WebDAV unit tests pass.

## Task 3: Frontend UI and browser-preview runtime

**Files:**
- Modify: `src/tauriRuntime.ts`
- Modify: `src/main.tsx`
- Create: `tests/syncUiText.test.mjs`

**Interfaces:**
- Consumes Task 2 Tauri commands.
- Produces UI with strategy selector and actions:
  - Test connection
  - Upload local to remote
  - Download remote to local
  - Smart merge

- [ ] **Step 1: Write failing frontend tests**

Add `tests/syncUiText.test.mjs` that reads `src/main.tsx` and asserts the UI contains:

```js
["智能合并", "远端覆盖本机", "本机覆盖远端", "WebDAV", "ZIP", "API Key"]
```

Add/extend runtime test to assert browser preview blocks new mutating commands and returns neutral preview for remote info.

- [ ] **Step 2: Run tests to verify RED**

Run:

```powershell
npm run test:runtime
node --test tests/syncUiText.test.mjs
```

Expected: tests fail because commands/UI text do not exist.

- [ ] **Step 3: Implement UI**

Add state and controls in `src/main.tsx`:

- WebDAV URL, username, password, remote root.
- Strategy select with the exact three options.
- Warning that model/provider configs include API Key and sync ZIP must be protected.
- Buttons wired to Tauri commands through `invokeCommand`.
- Result and error display.

- [ ] **Step 4: Update browser-preview command handling**

In `src/tauriRuntime.ts`, return neutral data for `webdav_fetch_remote_info` and reject mutating commands outside Tauri.

- [ ] **Step 5: Verify GREEN**

Run:

```powershell
npm run test:runtime
node --test tests/syncUiText.test.mjs
npm run typecheck
```

Expected: all pass.

## Task 4: End-to-end verification and docs

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes Tasks 1-3.
- Produces user-facing sync section and final verification.

- [ ] **Step 1: Document sync scope**

Add README section covering:

- what gets zipped
- what is excluded
- the three strategies
- warning about API Key sensitivity
- backup behavior for remote-over-local

- [ ] **Step 2: Run full verification**

Run:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm run test:layout
npm run test:provider-workflow
npm run test:runtime
npm run test:theme
node --test tests/syncUiText.test.mjs
npm run typecheck
npm run build
```

Expected: all commands exit 0.

- [ ] **Step 3: Final review**

Request code review for the branch diff and fix Critical/Important findings.

## Self-Review

- Spec coverage: ZIP package, sessions, model config, providers, strategy selector, and excluded runtime files are covered.
- Placeholder scan: no `TBD` or `TODO` remains in this plan.
- Type consistency: `SyncStrategy` names match command and UI strategy values.
