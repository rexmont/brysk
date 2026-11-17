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
            rf-app
               │                    │
     gpui_kit/gpui-pre      rf_platform::<module>
               │                    │
      [windows] gpui-pre-windows    [windows] crates/rf-win
      [macos]   gpui-pre (apple)    [macos]   crates/rf-mac
```

* `crates/rf-platform` is a one-line facade: `pub use rf_win::*` on Windows,
  `pub use rf_mac::*` on macOS. The 1,657 `rf_win::` call sites in `rf-app` were renamed
  mechanically to `rf_platform::` (no behavior change).
* **Why not a dependency alias:** Cargo rejects the same dependency key pointing at two
  packages from two target tables ("different source paths depending on the build target"),
  and two crates with one lib name collide. Distinct package names behind a facade are the
  supported form.
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
* **Renderer:** keep the workspace `[patch.crates-io]` (patches are global); only the
  `gpui-pre-windows` dependency is Windows-gated. macOS uses `gpui-pre`'s Apple backend
  (verified: `gpui-kit` + `gpui-pre-macos` build on macOS 26 / Rust 1.100). A tiny
  `crates/gpui-windows-shim` exports `hybrid`, `dnd_hint`, `KEEP_ACTIVE_LOOK`,
  `SMOOTH_SCROLL` (and `hybrid::load_snapshot`) as no-ops.
* **Windows-only UI is gated, not stubbed:** `dock`, `shell` (shell host, desktop, switcher),
  `snip`, `bench`, Velopack `installer`, `properties_view`, `organizer`, `prop_chooser`,
  `picker` are `cfg(windows)` in `rf-app`. Stubbing the 24k-line dock platform layer was the
  wrong trade (review 1): those features are macOS non-goals. Everything Explorer-shaped —
  tabs, panes, columns, preview, settings — stays compiled.
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

Total after review: **20–28 weeks** for one developer (the first draft said 10–14; both
reviewers rejected that). 14–18 only if quick search/indexing and full video are cut.

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

## 6. Review corrections (both reviews incorporated)

Architecture (review 1):

* facade, not alias (above); `cargo check -p rf-app --bin rustyfiles` is the M0 gate, not
  `-p rustyfiles`.
* `rf-app` is not untouched: ~100 direct `windows::`/`std::os::windows` uses remain and are
  being replaced one by one (cursor/modifiers via `rf_platform::input`, `CommandExt` and
  `MetadataExt` behind `cfg`, a real `flock` settings lock).
* `rf-core` compiles on Unix but is not portable yet: drive letters, `C:\`, `$Recycle.Bin`
  and Windows attribute bits are in `Location`, history, navtree and 1,000+ tests.
* Only `hash`, `design`, `elf`, `git` (after a `CommandExt` shim) are reusable verbatim;
  `ftp`, `encoding`, `datefmt`, `diskusage`, `foldersize`, `search`, `organize` need ports.

macOS behavior (review 2), corrected in the module implementations:

* Trash: `trashItemAtURL:` gives no Finder "Put Back" record; Trash is per volume
  (`~/.Trash`, `/Volumes/*/.Trashes/<uid>`), network shares often have none.
* Copies: `clonefile` only same-APFS; use `copyfile(COPYFILE_ALL|COPYFILE_CLONE)` with
  fallback and a chunked path for pause/limit; `RENAME_SWAP` is for swapping, not replace.
* `getattrlistbulk` is not APFS-specific and fails on SMB/NFS: keep a `read_dir` correctness
  path, add the bulk path later with per-entry fallback.
* Search/index: FSEvents via `notify` (debounce in caller); persisted event ids need
  `FSEventStreamCreateRelativeToDevice`; network mounts need `PollWatcher`; Spotlight is an
  accelerator only.
* `preview_host` is rich preview (QuickLookUI/PDFKit/AVFoundation), not thumbnails;
  `QLThumbnailGenerator` is the thumbnail path, `NSWorkspace.icon(forFile:)` the icon path.
* Hotkeys: Carbon `RegisterEventHotKey` needs no Accessibility permission; event taps do.
* Pasteboard: multiple `NSPasteboardTypeFileURL` items (no promised files through gpui's DnD).
* TCC: Desktop/Documents/Downloads/removable/network prompts; Full Disk Access cannot be
  requested programmatically. **Decision: no App Sandbox**, Developer ID + hardened runtime +
  notarization.
* Packaging: DirectLook must be a nested `.app` with its own bundle id and be signed before
  the outer app; notarize with `notarytool`, universal binary.
* Case sensitivity is per volume; APFS is normalization-insensitive, HFS+ stores NFD; compare
  with `CFStringCompare`, not `to_lowercase`.
* Metadata: xattrs, resource forks, ACLs, FinderInfo, tags (`com.apple.metadata:_kMDItemUserTags`),
  quarantine all matter; `std::fs::copy` loses them.
* iCloud/File Provider dataless files: `SF_DATALESS`, logical vs allocated size, never
  auto-download for a thumbnail.
* Hide `.DS_Store`, `._*`, `Icon\r`, `.Trashes`, `.Spotlight-V100`; filter
  `MNT_DONTBROWSE` volumes.
* SMB: `statfs` lists mounted shares only; browsing servers needs NetFS/`NSNetServiceBrowser`.
* Process age: `proc_pidinfo(PROC_PIDTBSDINFO)`, not `NSProcessInfo`.

## 7. Status (branch `macos-port`)

Done:

* `rf-platform` facade and the mechanical rename; Cargo target wiring.
* `rf-mac` compiles: known folders, `/Volumes` + `statfs` drives, `read_dir` listing with
  symlink/hidden/cloud bits, recursive copy/move/rename/unique names (APFS `clonefile`),
  folder sizes, walk-based deep search, `~/.Trash` list/empty/restore, file info + xattrs +
  quarantine, NSPasteboard file clipboard, accent/appearance, localized folder labels,
  codepage decoding via iconv, DOS time conversion, `notify` directory watches (included from
  `rf-win`), single instance + launch hand-over over a Unix socket, open/reveal via
  `NSWorkspace`/`open`, a first `input` module (Quartz cursor/modifiers).
* `rf-archive`, `rf-player`, `rustdecoder` build on macOS (Windows code gated; mac fallbacks).
* Windows shell-replacement UI gated; typed stubs for the platform services still called
  (dock geometry, status bar, taskbar progress, shell namespaces, property system, preview
  host, PDF, DirectLook hooks).
* Plan reviewed by two expert agents; corrections above.
* **`rf-app` compiles, links and launches**: `cargo build -p rf-app --bin rustyfiles` on
  macOS 26 arm64; the process creates its 1280×800 window in ~290 ms (start-up trace:
  "first render at 259 ms", "window opened at 287 ms"), opens a folder given on the command
  line, writes `settings.toml`, and stays running.
* Native `NSOpenPanel`/`NSSavePanel` pickers (dispatched to the main queue), the Finder's icon
  for every file type (`NSWorkspace`, so apps and documents look like they do in Finder), a
  real Unix-socket single-instance/launch hand-over, the Trash backend, the global quick-search
  hotkey (Carbon `RegisterEventHotKey`, Alt+Space registers successfully), and
  `rf-platform::input` (Quartz cursor/modifiers) are in.
* `scripts/build-macos.sh` builds `target/rustyfiles.app` (Info.plist with the TCC usage
  texts, ad-hoc signature) with DirectLook as a **nested agent app** under
  `Contents/Helpers/DirectLook.app`: as a bare binary in `Contents/MacOS`, LaunchServices
  registered the helper as `com.rustyfiles.app`, and `open rustyfiles.app` then activated the
  invisible helper instead of starting the file manager.
* Launch timing (macOS 26 arm64, debug build): 250–400 ms direct binary to a visible window,
  ~460 ms through `open` (5 runs: 459/460/472/464/464 ms), including a launch while the
  DirectLook helper is already running. An earlier 7 s launch came from a stale bundle that
  still had the 5 s main-thread hotkey timeout.
* `cargo test -p rf-mac`: 32 pass; 6 fail — all Windows-fixture tests that came in with the
  included modules (`design` ZIPs, `git` path states, a watch timing test), not macOS code.

Next (visible gaps, in order):

* Screenshot-verified theme pass (the shell has no Screen Recording permission, so the UI has
  only been verified through the window server: owner `rustyfiles`, 1280×800, layer 0).
* Quick Look thumbnails are in (PDF pages, video frames, Office/HEIC through
  `QLThumbnailGenerator`); the list shows them wherever thumbnails are on.
* The PDF viewer renders through PDFKit; DirectLook video plays through the FilmCraft
  decoders (silent: CoreAudio output is the one missing piece).
* The menu-bar item exists (`NSStatusItem`: Open / New Window / Settings / Quit) and the
  name index (background walk + FSEvents) serves quick search; app menu bar and macOS
  Cmd shortcuts are installed alongside the Explorer Ctrl ones.
* Unix path cleaner fixed (`/Users/...` is no longer turned into `\Users\...`; old sessions
  written that way are read correctly) — the cause of "folder not found" on every tab.
* DirectLook: AVFoundation playback + PDFKit pages (the rustdecoder window route is Windows-only).
* NSStatusItem menu bar (background mode), Carbon hotkey for quick search, Finder selection hook,
  Carbon/AppKit context-menu verbs ("Open With", "Reveal in Finder", tags).
* `.app` bundling (with the nested `DirectLook.app`), signing/notarization, CI matrix.
* `rf-core` Unix path-model pass (drive letters, `C:\`, Recycle Bin assumptions) and the
  macOS copies of the shared tests.

* Is the "typed stub + dependency alias" approach right, versus `cfg`-gating `rf-app` modules?
* gpui-pre Apple vs upstream `gpui` (same 0.3.11 snapshot) — which is less risk?
* Should `rf-win` itself gain `#[cfg]`s (one crate, two backends) instead of a second crate?
* M1 scope: is native `NSMenu` context menu required, or is the existing gpui menu enough?
* DirectLook video: AVFoundation first (fast to ship) vs rustdecoder VideoToolbox first (parity)?
* Is keeping Windows keybindings (Ctrl+C etc.) on macOS acceptable as the default, with Cmd as
  an option?