# cargo-version

I made this because I like the functionality of `npm version` and wanted the same functionality available for Rust (Cargo) projects.

## Installation

Install a current stable Rust toolchain with Cargo, then install from GitHub:

```sh
cargo install --git https://github.com/icdevin/cargo-version.git
```

Or install from a local checkout:

```sh
cargo install --path .
```

Ensure Cargo's binary directory (usually `~/.cargo/bin`) is on your `PATH`.
Git is required to create commits and tags.

## Usage

Run `cargo-version` directly from the directory containing the package's `Cargo.toml`:

```sh
# Increment the patch version for a bug-fix release.
cargo-version patch

# Set an exact version for a specific release.
cargo-version 2.0.0

# Show all options.
cargo-version --help
```

The command updates `package.version`, refreshes an existing package or workspace
`Cargo.lock` with `cargo update --workspace`, and prints the new version as `vX.Y.Z`.
Inside a Git worktree, it also commits the changed Cargo files with message `X.Y.Z`
and creates an unsigned tag named `vX.Y.Z`. Tracked files must be clean before the
command runs; untracked files do not block it. It does not push or publish releases.

Use the executable name `cargo-version`, not `cargo version`, which is Cargo's
built-in version command.

### Version arguments

Each example below starts from `1.2.3`. Prerelease examples use `--preid rc`.

| Argument | Result |
| --- | --- |
| `major` | `2.0.0` |
| `minor` | `1.3.0` |
| `patch` | `1.2.4` |
| `premajor` | `2.0.0-rc.0` |
| `preminor` | `1.3.0-rc.0` |
| `prepatch` | `1.2.4-rc.0` |
| `prerelease` | `1.2.4-rc.0` |
| Exact SemVer, such as `2.4.6` | `2.4.6` |

On an existing prerelease, `prerelease` increments the final numeric component
(for example, `1.2.4-rc.0` becomes `1.2.4-rc.1`). Without `--preid`, a new
prerelease starts at `0`. Changing `--preid` starts the new identifier at `.0`.
Use `patch` on a prerelease to remove the prerelease suffix without incrementing
the patch number. All bump arguments clear build metadata.

### Options

```sh
# Update Cargo files without a Git commit or tag, including in a dirty worktree.
cargo-version patch --no-git-tag-version

# Use a custom commit message; %s becomes the new version.
cargo-version minor --message "Release %s"

# Create a signed tag using the configured Git signing key.
cargo-version patch --sign-git-tag

# Start a release candidate, then increment it on the next run.
cargo-version preminor --preid rc
cargo-version prerelease --preid rc
```

`-m` is shorthand for `--message`. Outside a Git worktree, the command updates
Cargo files without creating a commit or tag.

### Workspace support

Run the command from the workspace member's directory. The member must have an
explicit `package.version` string. Virtual workspace roots and inherited versions
(`version.workspace = true`) are not supported. Only the selected package's
manifest version is changed; dependency version requirements are not updated.
