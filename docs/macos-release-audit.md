# macOS release audit follow-up

Reviewed the two supplied audits against `a74d503` on 2026-11-18. Their snapshots precede several package fixes already in that commit. This pass treats broken file operations and silent damage as release blockers; Finder feature parity is a separate backlog.

## Fixed in this pass

- File rows in details, icons and columns now start GPUI file drags. The same path payload reaches folder/tab/sidebar drop targets and is promoted to native macOS file URLs outside the window. Details and icons were exercised in the running app.
- App-row drops open the dropped documents with that application; they do not transfer documents into the bundle. Option forces a copy and Command forces a move on macOS.
- ZIP, encrypted ZIP, tar and 7z preserve Unix file permissions and symlinks. Extraction restores relative symlinks after regular files and rejects links escaping the destination and existing symlink parents. Package fixtures include an executable, an empty directory and chained framework links.
- New directory/package transfers have a whole-item undo record. Existing merged folders retain per-file records, protecting pre-existing contents. Move undo restores the exact original name without replacing another item. New directories retain their metadata.
- Case-only rename uses an exclusive temporary rename. Watcher stat calls return the filename's actual disk spelling, preventing duplicate rows after a case change. Inline/sidebar rename share platform filename validation.
- Return renames; Command+Down opens; Command+Option+V moves clipboard items into the current folder. Duplicate, a Go to Folder prompt, Finder's Go shortcuts, Eject, Hide/Hide Others/Show All and window Minimize/Zoom are connected. Menus/settings/hints were updated, including Get Info and Command+1/2.
- Volume enumeration no longer drops additional disks merely because they share a filesystem type. Mount-change polling runs off the UI thread. Looking up the mount of one path no longer probes every mounted network share. Eject is available in the volume menu and with Command+E.
- macOS search debounces recursive name searches while typing; Home searches the user's home directory. Dotfiles are no longer incorrectly marked SYSTEM in search/tag results.
- Packages are picked as single items, sorted with files, and retain their suffix during bulk rename. Common document/installer package suffixes were added. Directory symlinks navigate inside Brysk. Finder icons for folders, links and special document types are cached per path. iCloud uses its actual macOS folder.
- Direct Look uses native ImageIO decoding on dedicated workers. A sharp 2048-pixel representation arrives before the original (still capped at the existing 8192-pixel GPU limit). Cancelled navigation requests are skipped without discarding requests for live pinned windows; originals have their own worker. Probing and preparing macOS previews run off the UI thread. Window resizing after asynchronous preparation and the native toolbar-hover check replace macOS stubs.

## Already fixed at the starting commit

Packages launch as applications/documents and offer Show Package Contents. App copies keep both bundles instead of merging them, including suffix-preserving copy names. Package search/index traversal and app icons in QuickSearch were corrected. Applications is pinned; new tabs inherit the active location. Command-click, bare Backspace, basic Go shortcuts, volume labels, the status-bar free-space figure and the macOS sidebar's Show More Options gating were already addressed.

## Deferred

These are real gaps, but should not be grouped with silent file damage:

- Spotlight content search and a This Mac/Home/current-folder scope selector. Current search is explicitly recursive **name** search.
- Native Finder aliases, Show Original, Recents, AirDrop and a populated Open With application list.
- NFC/NFD matching, a complete UTType-based package classification and bundle version/identifier cards. The suffix list remains a heuristic.
- Finder-compatible AppleDouble/xattr/resource-fork storage and restoration in archives. This pass preserves executable bits and filesystem links; ZIP is not yet a full Finder metadata round trip. Tar hard links and unsupported link types remain skipped.
- Remaining Trash/menu/palette wording, global Empty Trash, full standard Help/Window menus, Gallery, arbitrary-depth columns, path bar/view options and folder spring loading.

Do not advertise the deferred features as supported. If full Finder metadata preservation or native alias support is a release promise, those remain release requirements.

## Validation and remaining checks

- Both application binaries build on macOS.
- macOS backend: 82 tests passed, one manual timing benchmark ignored by default.
- Archive unit suite: 66 passed, one ignored; three additional Unix package/security integration tests passed.
- Core suite: 390 passed and 57 failed. The identical 57 failures reproduce at the untouched starting commit; they concern existing Windows-path/platform assumptions. No new core failures were introduced.
- The app unit-test target has 13 macOS compile errors in existing Windows-only test code. The identical errors reproduce at the starting commit. UI smoke tests and a standalone shortcut assertion check cover the touched app behavior instead.
- UI checks: details/icon file drag into a folder, directory-symlink navigation, Return/case-only rename without duplicate rows, Command+Down, Duplicate, Go to Folder, live name search, and asynchronous Direct Look image/window presentation.
- Manual 24 MP JPEG debug benchmark: old Rust decode about 1.05–1.49 s; native sharp representation about 187 ms; native original about 537–602 ms. These measure decoding, not a guarantee for cold process startup, cloud downloads or GPU upload.
- Still requires manual hardware/platform checks: outbound drops to Finder/Dock/Terminal, physical toolbar hover (UI automation injects virtual pointer events), USB/DMG/network eject, cross-volume undo, and multi-display/Retina placement. No user volumes were ejected during testing.
