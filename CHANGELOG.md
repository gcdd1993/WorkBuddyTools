# Changelog

All notable changes to WorkBuddy Tools are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Added automatic signed application update checks, a header update indicator, download progress, and in-app installation with restart.
- Added Tauri updater artifacts and signing configuration to the GitHub release workflow.
- Added a GitHub Release fallback for versions published before updater metadata was available.

### Fixed

- Smart merge now maps projects outside the remote default workspace from the remote workspace drive to the local workspace drive while preserving their directory structure.

## [0.2.3] - 2026-07-14

### Added

- Added local session management backed by the `sessions` table in `workbuddy.db`.
- Added session search, recycle-bin deletion, and editing for session names and working directories.
- Added WebDAV ZIP sync with smart merge, remote overwrite, and local overwrite strategies.
- Added optional encrypted sync packages and local backups before remote overwrite.
- Added English and Simplified Chinese README files with feature screenshots.

### Changed

- Session sync now compares remote and local `defaultWorkspacePath` values and rewrites `sessions.cwd` and `workspaces.path` when workspace roots differ.
- Session model labels now use the value stored in `sessions.model`.
- Application settings and WebDAV sync controls are available from the desktop UI.

## [0.2.2] - 2026-07-10

### Changed

- Refreshed the desktop UI and verified the release build.
- Improved provider workflow state handling and TypeScript module resolution.

## [0.2.1] - 2026-07-10

### Changed

- Updated the release workflow to publish tagged releases without a manual draft step.

## [0.2.0] - 2026-07-10

### Added

- Added Windows NSIS installer bundles to release builds.

## [0.1.0] - 2026-07-10

### Added

- Initial tagged release of the WorkBuddy model configuration desktop application.
- Added automated dependency updates and the initial build and release workflow.

[Unreleased]: https://github.com/gcdd1993/WorkBuddyTools/compare/v0.2.3...HEAD
[0.2.3]: https://github.com/gcdd1993/WorkBuddyTools/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/gcdd1993/WorkBuddyTools/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/gcdd1993/WorkBuddyTools/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/gcdd1993/WorkBuddyTools/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/gcdd1993/WorkBuddyTools/releases/tag/v0.1.0
