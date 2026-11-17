# Porting rustyfiles to macOS — plan

Status: draft for review (M0). Goal: run the *same* rustyfiles interface — the Windows Explorer
look: tabs, breadcrumbs, details/grid/columns, Mica-ish chrome, DirectLook, archives — on macOS,
with macOS/Unix behavior underneath (POSIX paths, APFS volumes, Trash, Quick Look, FSEvents,
LaunchServices, native dialogs and menus).

Non-goals for the first release: replacing Finder as the system file manager, a Dock/taskbar
replacement, Mica/HyperGlass parity, Windows shell extensions, Velopack updates, code signing
policy decisions beyond ad-hoc/Developer ID.

---

## 1. The good news, measured on the tree (0.8.8, 201ca7b)

| Part | Size | Verdict |
| --- | --- | --- |
| `rf-core` (model, listing merge, sort, locations) | 18k lines, 48 files | builds on Unix; only 3 `cfg(windows)` references |
| `rf-archive` (ZIP/7z/RAR/tar/ISO, own rarchive) | 5.7k lines | portable; one `windows` use in `winrar.rs` |
| `rf-app` UI | 106k lines, 217 files | gpui (GPU-drawn) — the Explorer theme is 100% custom-drawn, OS-agnostic |
| `rf-win` platform layer | 118k lines, 348 files | 78 of 101 top-level modules touch Win32/WinRT; this is the port |
| `rf-player` / `rustdecoder` | 1.7k / 31k lines | player is tiny; decoder: AV1/H.264 GPU path is D3D11 → VideoToolbox later, CPU path portable |
| `rf-dialog` / `rf-shellguard` | 1.3k / 11k lines | Windows-only by definition → excluded on macOS |

Two structural facts make the port much cheaper than a rewrite:

1. **The UI never calls the OS directly at a low level.** `rf-app` goes through `rf_win::<module>`
   (1 656 call sites, 558 distinct symbols) and through `gpui_kit::*` (a re-export of
   `gpui-pre` 0.3.11). Both are crate boundaries we can substitute.
2. **`gpui-pre` 0.3.11 is cross-platform.** The pinned `gpui-pre-windows` fork only adds the
   Windows renderer/`hybrid` API; the crate family already ships `gpui-pre-apple` (AppKit/Metal).
   `rf-app` touches `gpui_windows` directly in ~20 places (`hybrid`, `dnd_hint`,
   `KEEP_ACTIVE_LOOK`, `SMOOTH_SCROLL`) — all shimmable.

## 2. Architecture: substitute the platform layer, do not fork the app

```
            rf-app (unchanged source)
               │                  │
     gpui_kit/gpui-pre     rf_win::*  (name kept)
               │                  │
      [windows] gpui-pre-windows   [windows] crates/rf-win  (untouched)
      [macos]   gpui-pre (apple)   [macos]   crates/rf-mac  (package = "rf-mac", lib name rf_win)
```

* `crates/rf-mac/Cargo.toml` sets `[lib] name = "rf_win"`; `rf-app` depends on
  `rf-win = { package = "rf-mac", path = "../rf-mac" }` under `cfg(target_os = "macos")`.
  Every `rf_win::…` call site keeps resolving; no mass edit of `rf-app`.
* **Reuse by path, not by copy.** Portable `rf-win` modules are included verbatim:
  `#[path = "../../rf-win/src/git.rs"] pub mod git;` (git, ftp, hash, encoding, design, elf, pe,
  datefmt, openrules, tree_watch, diskusage, foldersize, …), with small `cfg` shims where they
  touch the registry/times. Anything Windows-specific gets a macOS implementation or a typed stub.
* **Typed stubs, not `cfg`-deleted modules.** Windows-only subsystems (`dock`, `shellhost`,
  `snip`, `glass`, `switcher`, `desktop`, `tray`, `toasts`, `jumplists`, `weather`, `sysstatus`,
  `appvolume`, `tasklist`, `winkey`, …) keep their *public types and signatures* in `rf-mac`,
  with bodies that no-op/return empty. This keeps all 106k lines of `rf-app` compiling
  untouched, so the Explorer UI, tabs, columns, preview pane, settings pages and command palette
  all come up on day one; features are switched on one module at a time.
* **Renderer:** on macOS use `gpui-pre` from crates.io (drop the `[patch.crates-io]` Windows
  fork); add a tiny `crates/gpui-windows-shim` (lib name `gpui_windows`) exporting the four
  symbols `rf-app` uses, as no-ops. Window chrome (`TitleBar`, `open_window`) already comes from
  gpui-kit.
* **Keep the look.** `theme.rs`, `workspace.rs`, `folder_view.rs`, icons and layout stay
  byte-identical. Mica/HyperGlass become: solid/vibrancy under-window (NSVisualEffectView via
  gpui window background) with the same colors; Segoe UI falls back to the system UI font with a
  metrics table so row heights do not jump.

## 3. `rf-mac` module map

Legend: **REUSE** = include `rf-win` source; **PORT** = write a macOS implementation;
**STUB** = typed no-op for M1, real implementation later; **DROP** = Windows-only, not compiled.

| Area | Modules | Plan |
| --- | --- | --- |
| Paths & places | `known`, `places`, `drives`, `volume`, `libraries`, `network` | PORT: `$HOME`, `~/Desktop|Documents|Downloads|Movies|Music|Pictures|Applications`, `/Volumes` (local/network/external via `getmntinfo`), iCloud Drive, SMB via `statfs` |
| Listing | `listing`, `watch`, `tree_watch`, `journal`, `usn`, `nameindex`, `indexer`, `search`, `wsearch` | PORT: `getattrlistbulk` fast enumeration (APFS), FSEvents (`notify` 8 already supports macOS) instead of USN/ReadDirectoryChangesW; name index = walk + FSEvents delta; Spotlight (`NSMetadataQuery`) optional accelerator |
| File ops | `fileops`, `fastcopy`, `fastdelete`, `foldersize`, `removal`, `engine_guard`, `pane_picture` | PORT: parallel copy/move engine (same journal), `clonefile()` for APFS copies, `renameatx_np(RENAME_SWAP)` for atomic replace; progress/cancel/verify kept; eject via `diskutil`/`DADiskUnmount` |
| Delete / trash | `recycle`, `recycle_settings`, `fileactions` | PORT: `NSFileManager trashItemAtURL:` (put-back supported by Finder), no Recycle-Bin UI (view maps to `~/.Trash`), purge via settings |
| Icons / thumbs | `thumbs`, `image_cache`, `preview_host`, `look`, `pane_picture`, `rotate` | PORT: `NSWorkspace iconForFile:` (cached), `QLThumbnailGenerator` for documents/video/PDF, image decode in Rust as today; `pdf.rs` via PDFKit later |
| Menus / shell | `ctxmenu`, `shell`, `shellfolder`, `default_fm`, `openrules`, `runfind`, `runcmd`, `activate`, `share`, `dialogs`, `dialog_reg` | PORT subset: native `NSMenu` context menus (Open/Open With/Reveal in Finder/Compress/Copy Path/Get Info/Tags), `NSWorkspace openURL` for open/reveal, `NSOpenPanel`/`NSSavePanel` for pickers; `shell:` namespaces → `Location::Shell` mapped to `/Volumes`, Trash, Applications; default-FM registration DROP |
| Clipboard / DnD | `clipboard`, `dnd`, `shortcut`, `vfiles` | PORT: `NSPasteboard` file URLs + promised files, drag source/target via gpui; Finder interop via `NSFilenamesPboardType`/`public.file-url`; aliases read via `NSURL` bookmarks |
| System | `accent`, `osver`, `inputlang`, `sysparams`, `encoding`, `localized`, `apps`, `apps_direct`, `apppkg`, `bench`, `tasklist`, `hotkey`, `single_instance`, `handover`, `window`, `process_age_ms` | PORT: `NSAppearance` dark mode, system accent (`NSColor.controlAccentColor`), locales (same 49 JSON), `/Applications` scan + `LSCopyApplicationURLsForURL`, `RegisterEventHotKey` (Carbon) with Accessibility prompt, Unix-domain socket for single instance/handover, `NSProcessInfo` age |
| Archives / git / ftp / design | `rf-archive`, `git`, `ftp`, `design`, `elf`, `pe`, `hash`, `fileinfo` | REUSE as-is (last two behind `cfg` shims for registry/Exif host info) |
| Windows shell replacement | `dock`, `shellhost`, `shell_window`, `switcher`, `desktop`, `tray`, `trayhost`, `tray_wire`, `snip`, `glass`, `toasts`, `jumplist`, `jumplists`, `winkey`, `taskbar_*`, `sta`, `appvolume`, `sysstatus`, `weather`, `security`, `signature`, `cert`, `engine_guard`, `ownfolder`, `dialog_reg`, `recycle_settings`, `vfiles` shell bits | STUB (types kept). Later: status-bar item, Dock menu, Quick Look-style snip (ScreenCaptureKit), no taskbar/dock replacement |

`rf-app` keeps every module; only these are `cfg`-gated at the crate level because they need the
Windows DLL/installer or a Windows-only dependency: `rf-dialog` (Open/Save dialog DLL),
`rf-shellguard` (taskbar watchdog), Velopack `installer`/`update_splash` (macOS uses a `.app`
bundle + Sparkle-style updater later), `bench` (Win32 timing harness).

## 4. Milestones

**M0 — scaffold (1–2 days).** `crates/rf-mac` with `[lib] name = "rf_win"`; workspace + macOS
conditional dep; `gpui-windows-shim`; drop the Windows `[patch]` on macOS; `cargo check -p
rustyfiles` on macOS gets past dependency resolution. CI job on `macos-14` (`cargo check`).

**M1 — read-only Explorer (1–2 wks).** Main window, tabs, sidebar, breadcrumbs, details/grid/
columns, folder tree, sorting, settings load/save, home/volumes, icons + thumbnails, file
properties. Acceptance: browse `$HOME`, `/Applications`, an SMB share and a 100k-file folder
without blocking the UI; screenshot matches the Windows chrome.

**M2 — file ops (2–3 wks).** Copy/move/rename/delete-to-Trash, conflict dialog, undo/redo,
bulk rename, drag & drop between rustyfiles and Finder, progress/status center, folder sizes.
Acceptance: the copybench scenarios pass on APFS and exFAT; undo restores.

**M3 — preview & DirectLook (2 wks).** `directlook` as a second binary in a `.app` bundle;
images/text/code/archives via existing Rust paths; PDF via PDFKit; audio/video via AVFoundation
(initial) and rustdecoder CPU/VideoToolbox later. Space-to-preview, arrows to move.

**M4 — search & indexing (1–2 wks).** Name index + FSEvents, quick search, Spotlight fallback,
`*.ext` deep search bounded and cancellable.

**M5 — macOS integration polish (2–3 wks).** Native menu bar (File/Edit/View/Go/Window/Help),
Cmd-vs-Ctrl keymap (keep Windows shortcuts as a preset), Services/Share, Tags ↔ Finder tags,
default folder handler (optional), accent color, RTL, locale switching, Accessibility labels,
permissions (Full Disk Access prompt when a folder is protected).

**M6 — packaging (1 wk).** `cargo-bundle`/`create-dmg`, universal (arm64+x86_64), Developer ID
signing + notarization, app icon, first-run onboarding, crash log path, update check (manual
download link first).

Total: ~10–14 weeks for one developer who knows the codebase; the risky items are M1's
`getattrlistbulk`/gpui-apple and M3's video.

## 5. Risks and mitigations

1. **gpui-pre Apple backend maturity** (the project only ever shipped Windows). Verify on M0
   with a hello-window; fallback is upstream Zed `gpui` at the same snapshot, or patching
   `gpui-pre-apple` locally (Metal renderer exists upstream).
2. **`gpui_windows` APIs** (`hybrid`, `KEEP_ACTIVE_LOOK`, `SMOOTH_SCROLL`, `dnd_hint`) — shim,
   plus a "Renderer" badge hidden on macOS.
3. **`getattrlistbulk` unsafe code** — isolate in `rf-mac/src/sys/attrlist.rs`, fuzz with tests
   against `read_dir`, fall back to `std::fs` when it errors (network volumes).
4. **AppKit main-thread rules** — all `NS*` calls behind a `MainThreadMarker` helper; rf-mac
   functions may block on other threads only for pure-Rust work; icon/thumbnail APIs dispatch to
   the main thread and are cached.
5. **Trash on non-APFS/network volumes** — `trashItemAtURL` fails; fall back to `~/.Trash` move
   with a `.DS_Store`-style sidecar, or offer permanent delete (same as Finder).
6. **TCC/privacy prompts** (Desktop/Documents/Downloads, Full Disk Access, Apple Events to
   Finder) — first-run explainer, graceful `EPERM` → "Open System Settings" link.
7. **Case-insensitive but case-preserving FS** — existing name-conflict logic assumes Windows
   rules; audit `rf-core` comparisons against HFS+/APFS semantics; add tests.
8. **Text metrics/fonts** — Segoe UI absent; bundle a metric-compatible fallback or tune
   gpui-kit font stack so the Explorer chrome does not reflow.

## 6. Decisions for review

* Is the "typed stub + dependency alias" approach right, versus `cfg`-gating `rf-app` modules?
* gpui-pre Apple vs upstream `gpui` (same 0.3.11 snapshot) — which is less risk?
* Should `rf-win` itself gain `#[cfg]`s (one crate, two backends) instead of a second crate?
* M1 scope: is native `NSMenu` context menu required, or is the existing gpui menu enough?
* DirectLook video: AVFoundation first (fast to ship) vs rustdecoder VideoToolbox first (parity)?
* Is keeping Windows keybindings (Ctrl+C etc.) on macOS acceptable as the default, with Cmd as
  an option?