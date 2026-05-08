# Contributing to Connected

Thanks for helping improve Connected. This guide describes the expected development workflow, code quality standards, commit message format, and pull request process.

## Before You Start

- Check any existing issues and pull requests.
- Open an issue for major UI, protocol, security, or architecture changes.
- Keep changes focused. Small pull requests are easier to review and safer to merge.

## Development Setup

Clone the repository:
```bash
git clone https://github.com/paterkleomenis/connected.git
cd connected
```

And run the setup script:
```bash
./scripts/setup-dev.sh
```

Alternatively, install/make sure you have the required tools:

- Rust stable with the repository toolchain (we recommend using Rustup)
- `just`
- `pre-commit`

Useful platform tools:

- Android: Android Studio, Android SDK, and `cargo-ndk`
- iOS: macOS, Xcode, and `xcodegen`

List available development tasks with:

```bash
just
```

## Common Commands

```bash
# Format Rust and TOML files
just fmt

# Run formatting, Clippy, typos, and TOML checks
just lint

# Type-check the Rust workspace
just check

# Run the Rust test suite
just test

# Run security and license checks
just audit

# Run the repository CI helper
just ci

# Run the desktop app
just run-desktop
```

For platform-specific work, use the matching `just` recipes when available.

## Code Quality Standards

- Prefer small, direct changes over broad rewrites where possible.
- Follow the existing project structure: shared Rust logic belongs in `core/`, desktop code in `desktop/`, Android code in `android/`, iOS code in `ios/`, and UniFFI bindings in `ffi/`. Updates in `core` usually require updates in `ffi` as well.
- Keep protocol, encryption, discovery, and permission changes easy to review. Document behavior changes in the pull request.
- Run `just ci` before committing, and make sure it passes. Let us know if you're having trouble at any point.
- Add or update tests for bug fixes and behavior changes where practical.
- Do not commit build artifacts, generated local files, secrets, signing keys, or personal configuration. Our .gitignore rules should cover most of those, but double check to be certain.

## Commit Messages

You may opt to use Conventional Commits:

```text
type(scope): short description
```

Examples:

```text
feat(desktop): add transfer retry action
fix(core): handle discovery timeout cleanup
docs(readme): clarify Android setup
```

Allowed types are:

- `feat`
- `fix`
- `docs`
- `style`
- `refactor`
- `test`
- `chore`
- `ci`
- `build`
- `perf`
- `revert`

Irrespective of your choice to use conventional commits or not, make sure to follow the guidelines below:

- Use the imperative mood: `fix crash`, not `fixed crash`.
- Keep the subject concise.
- Add a body when the reason for the change is not obvious, and also if you believe it is needed, or want to provide additional information on a change you're making.
- Reference related issues when applicable, such as `Fixes #123`.

## Pull Request Process

Before opening a pull request:

1. Create a feature branch from the current default branch.
2. Keep the branch focused on one fix or feature, where possible.
3. Run `just ci`.
4. Run platform-specific builds or checks for affected platforms.
5. Update documentation when behavior, setup, permissions, or supported platforms change.

In the pull request description, include:

- A short summary of the change.
- Why the change is needed.
- Tests or checks you ran.
- Screenshots or recordings for UI changes.
- Any known limitations or follow-up work.

Review expectations:

- Address requested changes with follow-up commits unless a maintainer asks for a rebase or squash.
- Keep discussion technical and constructive.
- Do not force-push over review history unless needed to resolve conflicts or requested by a maintainer as a clean up before merge.

## Reporting Bugs

When reporting a bug, include:

- Operating system and version.
- App version or commit SHA.
- Device model for mobile issues.
- Steps to reproduce.
- Expected and actual behavior.
- Logs, screenshots, or recordings when useful, and if possible.

Security-sensitive issues should not be posted publicly. Contact the maintainers privately before sharing exploit details.

## Licensing

Connected is licensed under either the MIT license or the Apache License, Version 2.0. By contributing, you agree that your contributions are licensed under the same terms.
