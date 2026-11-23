# Development

This guide covers running, building, and releasing Lazy Worker. To propose a change, start with [Contributing](../CONTRIBUTING.md).

## Project layout

| Path | Holds |
| --- | --- |
| `src/` | The panel: TypeScript and CSS, no framework. `modules.ts` registers each tool, `messages.ts` holds the English and Spanish strings, and `desktop.ts` is the bridge to the native app. |
| `src-tauri/src/` | The native app in Rust: the menu bar, the panel window, preferences, updates, and one module per tool. |
| `src-tauri/native/` | The Swift helper. It posts Activity's pointer events and detects screen sharing. |
| `docs/` | The user guide, this guide, the README images, and the [clay workshop](design/clay-workshop/README.md) that makes the tool clips. |
| `scripts/` | The release command and its update verifier. |
| [`AGENTS.md`](../AGENTS.md) | The project rules. They apply to every change, by people or by agents. |

## Run locally

Install Node.js 22.12 or newer, Rust stable, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform. macOS needs the Xcode command-line tools, including Swift.

```sh
npm ci
npm run tauri -- dev
```

Use `npm run dev` to open the interface in a browser. Browser mode simulates module sessions and saves preferences in browser storage. The installed desktop app runs the native helper and schedule.

Run the checks with `npm run build` and `cargo test --manifest-path src-tauri/Cargo.toml`. The [Check workflow](../.github/workflows/check.yml) runs the same checks on macOS for every pull request.

A development launch may require enabling the terminal or the helper under Accessibility.

## Build for macOS

The app bundle and Swift helper target macOS 12.0 (Monterey) and later on Intel and Apple silicon.

```sh
rustup target add x86_64-apple-darwin
npm run tauri -- build --target x86_64-apple-darwin --bundles app
```

Use `aarch64-apple-darwin` to build for Apple silicon. Find the app under `src-tauri/target/<target>/release/bundle/macos/`.

Local macOS builds use an ad-hoc signature for testing. Distribution requires its own signing and notarization setup.

macOS may keep an Accessibility entry enabled after an ad-hoc update even when it no longer matches the installed app. If Activity reports a permission error while Lazy Worker is enabled, select its entry in Accessibility and click **−**. Click **+**, choose `/Applications/Lazy Worker.app`, and enable it again. Use a stable signing identity for builds that need their permission to survive updates. Test the packaged build on the actual Intel Mac before claiming Monterey support for a release.

### Publish a macOS release

One command publishes a release from this Mac:

```sh
npm run release -- 0.5.0 --summary "One line about the release"
```

Run it on a clean `main`. It does these steps in order and stops at the first one that fails:

1. Checks the branch, the tools, the signing identity, both Rust targets, and that the updater key matches the last release.
2. Runs the build and the tests.
3. Sets the version in the five files that hold it and commits.
4. Builds Apple silicon and Intel with Developer ID, and verifies each app's version, architecture, signature, hardened runtime, and macOS 12.0 target.
5. Notarizes and staples each app.
6. Packages each one as a notarized disk image and a ZIP for manual installation. The disk image opens on a window with Miga, the app, and the Applications folder; its picture and layout are in `scripts/disk-image`. It also packages each one as a signed `.app.tar.gz` for the updater, with `latest.json` and `SHA256SUMS.txt`.
7. Rehearses the update: installs the last published release in a scratch folder and updates it with the real Tauri updater.
8. Tags, pushes, and creates a draft release on GitHub.

Review the draft and publish it. Installed copies see the update once the release is published.

If the command stops after step 3, fix the cause and run it again with the same version. It continues from its version commit, which is not on GitHub until the last step.

Use `npm run release -- --dry-run` to rehearse the current version. A dry run does steps 1, 2 and 4 to 7, and changes nothing in the repository or on GitHub.

The command needs [uv](https://docs.astral.sh/uv/) on your `PATH`. Its `uvx` runs `dmgbuild`, the tool that builds the disk image, at the version the command pins.

The command reads local names from `.release.local.json`, which git ignores. Copy `scripts/release.local.example.json` and fill in the Developer ID identity, the `notarytool` keychain profile, and the keychain item that holds the updater key as JSON with `privateKey` and `publicKey`. Set `rustHome` when the release uses its own Rust toolchain. Keep the Tauri private key outside the repository and use the same key for every release.

## Add Windows support

Build the project on Windows using the [Windows development prerequisites](https://v2.tauri.app/start/prerequisites/#windows). The shared UI, scheduling rules, and preferences already have a platform boundary. Activity and Caffeinate display an unavailable state on Windows until Windows adapters are added. Proxy detects and opens its clients only on macOS, in `src-tauri/src/proxy/apps.rs`.

Implement the Windows worker alongside `src-tauri/src/activity/macos.rs` and select it by target in `src-tauri/src/activity/mod.rs`. A worker starts a session, reports whether it is running, and releases its resources when dropped. Keep the schedule and preferences outside that adapter.
