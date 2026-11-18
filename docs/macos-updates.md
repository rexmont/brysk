# macOS updates

Brysk uses Sparkle 2.10.1's standard macOS updater. Scheduled checks show a quiet
**Update available** button at the right of the main title bar. Clicking it, About's
**View Update…**, or **Check for Updates…** opens Sparkle's native update window.
The user sees release notes, chooses **Install Update**, then **Install and Relaunch**.
Automatic checks can be turned off in About; downloading and installation require
the user's choice. Sparkle owns scheduling; Brysk does not poll for updates.

Before relaunch, Brysk waits for running file operations through their completion
notifications, saves the session, closes file-manager windows, and stops DirectLook,
the Finder LaunchAgent and the index. Cancelled or failed installations resume the
Finder helper; a successful relaunch registers the updated helper when opted in.
Sparkle verifies the signed archive and handles replacement and
relaunch. A bare Cargo binary cannot update itself. An app run from a DMG or an
unwritable location receives Sparkle's normal move/authorization guidance.

## Building and publishing

- `scripts/build-macos.sh` downloads a pinned, SHA-256-checked upstream Sparkle
  archive into `target/sparkle`, embeds the framework, and signs its nested code
  before signing Brysk. The framework is universal. Its complete licence and
  third-party notices are included in `packaging/macos/Sparkle-LICENSE` and the app.
- `CFBundleVersion` in `packaging/macos/Info.plist` is the monotonically increasing
  update build number. Increment it for every different published build, even
  when replacing an asset under the same release tag. The user-facing version comes
  from Cargo; Apple's numeric `CFBundleShortVersionString` stays numeric.
- The Sparkle Ed25519 signing key is in the login Keychain under account
  `com.rexmont.brysk`. Only its public key is committed in `SUPublicEDKey`.
  Back up the private key securely before moving machines; never put it in Git.
- `scripts/package-macos.sh` notarizes/staples the DMG, then signs the finished
  archive and generates `dist/appcast.xml`. Both the archive and feed have
  Ed25519 signatures. The package script does not upload anything.
- After uploading the DMG and checksum to the matching GitHub release, copy
  `dist/appcast.xml` to `packaging/macos/appcast.xml`, then commit and push it to
  `macos`. Publish assets first so a feed never advertises an unavailable download.
  The feed retains previous entries and replaces an entry with the same build or asset URL.
- Any feed edit requires re-signing with
  `target/sparkle/bin/sign_update --account com.rexmont.brysk packaging/macos/appcast.xml`.
  Validate with the same command plus `--verify`.

## QA

Test in a separate app with its own bundle identifier, config/instance namespace,
and signed loopback feed. Keep production's app and preferences untouched. Exercise
no update, available update, cancellation, signed download, installation/relaunch,
and operation completion before replacement. Do not use a fake version badge as
evidence that an installation works.

A renamed QA bundle must also use `<bundle identifier>.finder-agent` as its LaunchAgent
`Label` and its own identifier in `AssociatedBundleIdentifiers`. Both the app and agent
reject mismatched identities before accessing SMAppService, so a copied test app cannot
unregister the installed app's Finder helper.
