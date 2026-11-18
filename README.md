<p align="center">
  <img src="docs/readme/brysk-banner.png" alt="Brysk — a macOS file manager that stays responsive. Built with Rust." width="100%">
</p>

<p align="center">
  <a href="https://ko-fi.com/J3J3WWZKO" target="_blank" rel="noopener noreferrer">
    <img height="36" src="https://storage.ko-fi.com/cdn/kofi6.png?v=6" border="0" alt="Support Brysk on Ko-fi">
  </a>
</p>

# Brysk

Website: [brysk.app](https://brysk.app)

Brysk is a **macOS-only file manager**, written in Rust, with an Explorer-inspired interface
and a native macOS core. Based on [rustyfiles](https://github.com/gelenmedya/rustyfiles) by
Özer Güzel, it has been **extensively reworked and extended for macOS**: a native filesystem
and file-operation layer, macOS system integration, a rebuilt preview pipeline, and substantial
performance and interface changes.

> **Version: 0.1.1.** Requires macOS 13 or later. Releases are universal (Apple silicon + Intel)
> Developer ID builds, notarised and stapled as DMGs.

## Why Brysk

- Tabs and **split panes**, details / grid / column views, grouping, a shelf and a preview pane.
- A fast **name index** with live FSEvents updates, application search and Chrome/Edge bookmarks.
- **DirectLook** previews with Space: images, video with sound, PDF, text/code, archives and more.
- A macOS-native **copy/move engine**: progress, pause/resume, verification, speed limit,
  Finder-style `name copy` names and non-destructive keep-both clashes.
- Finder tags, Trash with its own journal and undo, archives opened like folders, Git status,
  folder sizes and disk usage.
- Native menu bar, Dock/background reopen, native panels and Share sheet, Cmd shortcuts alongside
  the Explorer keys, and 49 interface languages.

## Finder vs Brysk

**Everyday browsing**

| | Finder | Brysk |
|---|---|---|
| Tabs | Yes | Yes |
| Side-by-side panes | No | Yes — split the tab |
| Views | Icons, list, columns, gallery | Details, grid, columns |
| Quick preview | Quick Look | DirectLook (Space) |
| Search | Spotlight, including file contents | Fast name index + live updates; apps and browser bookmarks; no content index |
| Batch rename | Yes | Yes — patterns, find/replace, regex |
| Network shares | SMB/AFP discovery | Mounted volumes only |

**Files and power features**

| | Finder | Brysk |
|---|---|---|
| Copy / move | System engine, conflict prompt | Own engine: queue, pause, verify, speed limit; name clashes keep both |
| Trash | All apps, Put Back | Own journal + undo; other apps' Trash items need Full Disk Access and are not listed |
| Archives | Creates and extracts ZIP | Opens ZIP/7z/tar like folders and extracts them |
| Finder tags | Yes | Read and written back to Finder |
| Git | — | Status column, branch/sync, init/clone |
| Folder size / disk usage | Get Info | On-demand size column and disk-usage view |
| Open in Terminal / editor | Services | Context menu + status bar; editor auto-detected |
| Shortcuts | macOS | macOS Cmd **and** Explorer Ctrl keys |

Current builds include Sparkle for in-app updates: check in Settings → About,
or use the title-bar **Update available** button when a newer build is found.
See [macOS updates](docs/macos-updates.md).

## Performance

Latest recorded measurements on macOS 26 arm64. Application memory was measured in a release build
with the search index enabled; the other measurements used debug builds.

- **Folder opening:** longest measured UI-thread stall of **34–39 ms**.
- **Search index:** **480k names** use **39 MB** of memory and a **22 MB** disk cache.
- **Application memory:** approximately **208 MB RSS / 111 MB physical footprint**, with the index enabled.
- **Thumbnails:** **40 MB** physical footprint for a 96-image grid; approximately **198 MB** after a 300-preview walk.
- **Git:** **47 ms** for `git status` in a large repository.
- **Undo/redo:** approximately **4 MB** additional memory for a 50-operation history.
- **Idle CPU:** effectively zero.

## Install

Download the latest DMG from [Releases](https://github.com/rexmont/brysk/releases), open it and
drag **Brysk** to Applications. The build is signed and notarised; no Gatekeeper bypass is needed.

Onboarding offers Finder Dock-click replacement with its checkbox selected by default. Continue
applies the choice; it can also be changed in Settings → General. A small
macOS-managed background helper opens Brysk even after **Quit (⌘Q)**. It needs Accessibility
access and intercepts plain left-clicks only; right-clicks and modified clicks keep their usual
Finder behavior. Turning the option off unregisters the helper. Native filesystem notifications
restore Finder clicks if Brysk is moved or removed. There is no polling, system patch or SIP
change. After granting Accessibility access, return to Brysk to activate the integration.

The optional menu-bar icon is off by default and can be enabled in Settings → General.
Settings → Advanced controls whether Brysk keeps running after its windows close; this is
independent of the icon and Finder helper. **Quit (⌘Q)** exits Brysk.

## Build from source

```sh
git clone https://github.com/rexmont/brysk.git
cd brysk
./scripts/build-macos.sh          # → target/Brysk Debug.app
open "target/Brysk Debug.app"
```

Debug builds use a separate app identity, settings folder and DirectLook instance. They leave
Finder Dock integration, the global HyperSearch shortcut and release updates to the release app.

For a release DMG (universal, Developer ID, notarised):

```sh
UNIVERSAL=1 \
BRYSK_DEVELOPER_ID='Developer ID Application: …' \
BRYSK_NOTARY_PROFILE='<notarytool keychain profile>' \
./scripts/package-macos.sh        # → dist/Brysk-<version>.dmg
```

Port notes: [`docs/port-macos.md`](docs/port-macos.md). Upstream sync:
[`docs/upstream.md`](docs/upstream.md).

## Known limits

- Trash lists items Brysk deleted; Finder/other-app items require Full Disk Access and are not
  enumerated. The empty state explains this.
- Name clashes keep both instead of asking replace/skip; nothing is overwritten.
- Get Info is read-only (no permission or tag editing yet).
- Finder Services/system-wide default-file-manager registration are not implemented; network locations must be mounted first.
- Speed test and Folder organizer are Windows-only features and are not part of the macOS build.

## Credits

Brysk is based on [rustyfiles](https://github.com/gelenmedya/rustyfiles) by
[Özer Güzel](https://github.com/gelenmedya). Brysk retains the original MIT licence and copyright notices.

The Finder Dock-click integration is a Rust port of
[finder-replace](https://github.com/mishamyrt/finder-replace) by Mikhail Khrustik, also MIT
licensed. Its copyright and full permission notice are reproduced in [NOTICE](NOTICE).

The full MIT licence, including the upstream copyright notice, is in [LICENSE](LICENSE).
Project and third-party attribution is in [NOTICE](NOTICE); both files ship inside `Brysk.app`.
