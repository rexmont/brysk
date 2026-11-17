# Working with upstream

This repository is a fork of [gelenmedya/rustyfiles](https://github.com/gelenmedya/rustyfiles)
that ports the file manager to macOS. The Windows code stays in the tree as the reference
implementation; on macOS `rf-win` is a façade that re-exports `rf-mac`.

## Remotes

| Remote | URL | What it is |
| --- | --- | --- |
| `origin` | `https://github.com/rexmont/rustyfiles` | this fork (push here) |
| `upstream` | `https://github.com/gelenmedya/rustyfiles` | the original project (read only) |

## Branches

* `main` — a plain mirror of `upstream/main`. Never commit port work here.
* `macos` — the macOS port (the fork's default branch). All port commits live here.

## Bringing upstream work in

```sh
./scripts/sync-upstream.sh        # fetch, fast-forward main, merge main into macos
```

Then resolve what is left and run `cargo build && cargo test -p rf-mac`.

What a conflict normally means:

* **`rf-win/src/lib.rs`** — upstream added a Windows module. Add `#[cfg(windows)]` to its
  `pub mod` line. If `rf-app` starts calling it, add the same module path (a real
  implementation or a typed stub) to `rf-mac`.
* **`rf-app`** — upstream code calls a new `rf_win::…` function. Port it to `rf-mac` (or stub it
  if it is a Windows shell feature) and keep the cfg gate the same shape as the neighbours.
* **Windows-only subsystems** (`dock`, `shell`, `snip`, `tray`, `vfiles`, `dialog_reg`,
  `default_fm`, …) — if upstream adds calls in shared UI, gate the call with `#[cfg(windows)]`
  and give the macOS side a no-op/stub where needed.

Because `rf-app` keeps upstream's `rf_win::` naming and only 38 files differ from `main`,
most upstream commits merge cleanly. Our own files (`crates/rf-mac`, packaging, scripts) never
conflict.

## Selective sync

For a single fix, cherry-pick instead of merging:

```sh
git fetch upstream
git switch -c pick/<name> macos
git cherry-pick <sha>
```

Portable fixes (rf-core, rf-archive, rf-player, locales, settings) usually apply as they are;
Windows shell work is skipped unless it changes shared behaviour.

## Windows build

Not a target of this fork. `rf-win`'s Windows modules remain in the tree and are still written
against Win32; they are only compiled on Windows, and the fork does not keep the Windows build
green.
