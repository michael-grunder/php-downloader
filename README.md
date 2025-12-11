# php-downloader

`php-downloader` is a small Rust CLI that keeps a local cache of official PHP
release tarballs, extracts them into build trees, and upgrades those trees when
new patches become available. It was built to streamline extension development,
but it is handy anytime you need to pull, unpack, and rebuild PHP repeatedly.

## Features

- **Release-aware downloads** – Works against `php.net` and the museum archive,
  understands RC/beta/alpha suffixes, and automatically resolves a major/minor
  such as `8.3` into the newest patch before downloading.
- **Local registry** – Tarballs are cached under
  `~/.phpdownloader/tarballs` (override with `PHPDOWNLOADER_ROOT`) so repeat
  downloads are instant and usable offline.
- **Manifested extracts** – `extract` writes a manifest file alongside the
  tree, then runs optional hooks so configure/build scripts can run hands-free.
- **Incremental upgrades** – Point `upgrade` at an existing `php-X.Y.Z*`
  directory to pull the latest patch, run hooks, and optionally remove the old
  tree after backing up custom files.
- **Automation friendly output** – Pass `--json` to emit machine-readable
  metadata for `list`, `latest`, or `cached`, or stay with the colored CLI view.

## Installation

This is a standard Cargo project and requires a Rust toolchain (1.74+ is a safe
bet). Clone the repository and either build or install it locally:

```bash
git clone https://github.com/michael-grunder/php-downloader.git
cd php-downloader

# One-off build
cargo build --release

# Or install into ~/.cargo/bin
cargo install --path .
```

If you use a non-standard home directory for caches, set
`PHPDOWNLOADER_ROOT=/path/to/root` and all registry/manifests/hooks will live
under that directory instead of `~/.phpdownloader`.

## Usage

Global flags that apply to every subcommand:

| Flag | Description |
| --- | --- |
| `-e, --extension <bz2|gz|xz>` | Choose the compression type when downloading/extracting. |
| `-j, --json` | Render listings (list/latest/cached) as JSON instead of an aligned table. |
| `-f, --force` | Overwrite existing files when downloading a tarball. |
| `-n, --no-hooks` | Skip running hook scripts during `extract` or `upgrade`. |

```text
Usage: php-downloader [OPTIONS] <COMMAND>
```

### Command overview

#### `cached [VERSION]`
Show tarballs already present in the registry. Provide a partial version (for
example `8.2`) to filter the list.

```bash
php-downloader cached
php-downloader cached 8.3
```

#### `download <VERSION> [OUTPUT_PATH]`
Download a release into the registry or an explicit directory. `VERSION` can be
full (`8.3.6`) or partial (`8.3`), and the command resolves the latest patch
when `patch` is omitted.

```bash
php-downloader download 8.3              # => ~/.phpdownloader/tarballs/php-8.3.N.tar.bz2
php-downloader download 8.2.12 /tmp      # => /tmp/php-8.2.12.tar.bz2
```

#### `extract <VERSION> <OUTPUT_PATH> [OUTPUT_FILE]`
Ensures the requested tarball is present, extracts it into the target directory,
runs hooks (unless `--no-hooks`), and writes a manifest (`.phpdownloader-manifest`)
containing the tracked files for later upgrades.

```bash
php-downloader extract 8.3 ~/src
php-downloader extract 7.4 ~/build 7.4-debug-tree
```

#### `save-scripts <SRC_PATH> <DST_PATH>`
Copies files that are **not** part of the manifest from an extracted tree into a
backup directory. This is primarily used internally by `upgrade`, but can be
handy when preserving ad-hoc changes.

#### `latest [VERSION]`
Fetches the newest version for one or more major/minor branches (defaults to
`7.4, 8.0–8.3` if no version is supplied) and prints the download metadata.

#### `list [VERSION]`
Lists every downloadable tarball for the specified branch. Without a version,
`php-downloader` resolves the active version according to the data cached from
`https://www.php.net/releases/active/`.

#### `upgrade <PATH>`
Finds one or more `php-<version>` directories under `PATH`, downloads the
latest patch for each, extracts it beside the old tree, backs up custom files,
and (optionally) removes the replaced directory once the user confirms.

#### `version`
Prints the CLI version and exits.

### Hooks

Executable scripts placed under `~/.phpdownloader/hooks/` (or
`$PHPDOWNLOADER_ROOT/.phpdownloader/hooks/`) allow you to automate build steps.
Currently supported hook names are:

| Script | When it runs |
| --- | --- |
| `post-extract` | Immediately after untarring into the destination. |
| `configure` | After `post-extract`; intended for running `./configure` with your preferred flags. |
| `make` | Last step, perfect for `make -j$(nproc)` or running tests. |

Hooks receive the extracted directory path as both the working directory and the
sole argument. Combine this with `--no-hooks` when you want a clean extract
without executing any additional scripts.

### Example workflow

```bash
# See what's already cached
php-downloader cached

# Check the newest 8.3 release and grab it if needed
php-downloader latest 8.3
php-downloader download 8.3

# Extract into ~/src, run hooks, and capture the manifest
php-downloader extract 8.3 ~/src

# Later on, upgrade an existing build tree in-place
php-downloader upgrade ~/src/php-8.3.5
```

The CLI always prints the destination paths it touches, so it is easy to script
around. Combine `--json` with `jq` for automation, or stick to the aligned
terminal output when working interactively.
