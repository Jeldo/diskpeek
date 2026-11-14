# diskpeek

A terminal tool for macOS that shows which files and folders are using your disk space.

**Read-only.** It only reads file metadata and directory listings. There is no delete, move, or edit feature, and it creates no config or cache files. You decide what to remove by looking at the screen, then delete it yourself in Finder or the terminal.

## Install

### Prebuilt binary

Download the archive for your Mac from the [Releases](https://github.com/Jeldo/diskpeek/releases) page.

| Mac | Archive |
|---|---|
| Apple silicon | `diskpeek-aarch64-apple-darwin.tar.gz` |
| Intel | `diskpeek-x86_64-apple-darwin.tar.gz` |

```sh
tar xzf diskpeek-aarch64-apple-darwin.tar.gz
cd diskpeek-aarch64-apple-darwin
./diskpeek ~/Github
```

The binary is not code-signed. If you downloaded it with a browser and macOS refuses to open it, remove the quarantine flag:

```sh
xattr -d com.apple.quarantine ./diskpeek
```

Move `diskpeek` to a folder on your `PATH` (for example `/usr/local/bin`) to run it from anywhere.

### From source

Requires Rust 1.96 or later.

```sh
cargo install --path .
diskpeek ~/Github
```

To run without installing:

```sh
cargo run --release              # scan your home folder (~)
cargo run --release -- <path>    # scan the given folder
```

## Usage

```sh
diskpeek [path]    # defaults to ~
```

Every run scans from scratch and nothing is saved. Changes made while it is running are not picked up automatically; press `r` to scan again. While scanning, it shows the number of files read and their total size. Folders are read on four threads. A recently read tree of about 150,000 folders takes around a second, and the first read takes longer.

## Screens

| Screen | Key | Contents |
|---|---|---|
| Browse | start screen | Items in the current folder, sorted by size. Cleanup candidate folders carry a kind tag. |
| Cleanup candidates | `c` | All cleanup candidates found in the scanned tree, grouped by kind and sorted by size. |
| Large files | `f` | The 100 largest regular files that are not inside a cleanup candidate folder. |

## Keys

| Key | Action |
|---|---|
| `↑` `↓` or `k` `j` | Move selection |
| `Enter` or `→` | Browse: open the folder. Cleanup candidates and large files: jump to that location in Browse |
| `Esc`, `←` or `Backspace` | Browse: go to the parent folder. Cleanup candidates and large files: return to Browse |
| `c` | Open the cleanup candidates screen; press again to return to Browse |
| `f` | Open the large files screen; press again to return to Browse |
| `r` | Rescan from scratch. Keeps the current screen and selection; if the folder you were viewing no longer exists, moves to the nearest surviving parent |
| `q` or `Ctrl+C` | Quit |

Keys also work with the Korean (2-set) keyboard layout active.

## Cleanup candidates

A cleanup candidate means "data that is usually regenerated or can be downloaded again". **It is not a guarantee that deleting it is safe.** Check the contents before you delete anything.

Only clear-cut cases are flagged: a known fixed path, or a folder with a project marker file next to it. Names that may hold source code, such as `dist`, `out`, and `bin`, are never flagged.

| Kind | Matches |
|---|---|
| Cache | `~/Library/Caches`, `~/.cache`, `~/.npm`, `~/.yarn/cache`, `~/Library/pnpm/store`, `~/.cargo/registry`, `~/.cargo/git`, `~/.gradle/caches`, `~/.m2/repository`, `~/go/pkg/mod` |
| Build output | `node_modules` and `.next` (next to `package.json`), `target` (next to `Cargo.toml` or `pom.xml`), `build` (next to `build.gradle` or `build.gradle.kts`), `Pods` (next to `Podfile`), `.venv` and `venv` (containing `pyvenv.cfg`), `__pycache__`, `~/Library/Developer/Xcode/DerivedData` |
| Temporary data | `~/Library/Logs`, `~/.Trash` |

The inside of a cleanup candidate folder is not classified again, so the same space is never counted twice in the totals.

The kind tags and labels in the interface are displayed in Korean.

## Permissions

macOS blocks terminal apps from reading some folders. Folders that cannot be read are skipped and counted in the header of the Browse screen as `건너뜀 N개` ("N skipped"). The size of skipped folders is not included in the totals.

- The first time you access `~/Documents`, `~/Desktop`, or `~/Downloads`, macOS asks whether to allow your terminal app.
- `~/Library/Mail`, `~/.Trash`, and similar locations can only be read if your terminal app has Full Disk Access. Add it under **System Settings → Privacy & Security → Full Disk Access**.

## How sizes are calculated, and known limitations

- Sizes are actual disk usage, not file length, and match `du`. Units are decimal like Finder (1 GB = 1,000,000,000 bytes).
- Symbolic links are not followed. Files joined by hard links are counted once, under the path that sorts first by name.
- It does not cross into other volumes (external disks, network drives) from the starting path.
- APFS clones share space on disk but are counted separately, so totals can be larger than actual usage.
- Cloud folders whose files are not downloaded yet, such as iCloud, are not entered, because listing them can trigger a download. They show a size close to zero.
- File changes are not detected automatically. Press `r` to rescan for the latest state.

## Development

```sh
cargo test
```

`tests/read_only.rs` checks that the product code (`src/`) contains no calls that write, delete, or move files, or run external processes. To compare results against `du` on a real folder:

```sh
SM_REAL_DIR=$HOME/Github cargo test --release --test scan real_dir -- --ignored --nocapture
```

## License

MIT
