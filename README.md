# Lazy Worker

Lazy Worker is a desktop toolbox with Activity and Caffeinate. The app opens in Tools. In Settings, choose Light or Dark and English or Spanish, or leave either setting on System. System uses the first supported preferred language, with English as the fallback.

## macOS menu bar

Use the wind-up key in the menu bar to open the panel, start the selected tools, or start and stop individual tools. A terracotta dot appears beside the key while at least one tool is running. Menu actions use saved settings. Close the panel to leave Lazy Worker running in the menu bar. Opening Lazy Worker again brings back the existing panel. Choose **Quit** from the menu to exit and release its active sessions.

**Settings → Hide Lazy Worker while sharing** defaults to on once Accessibility is granted. Without permission, the switch stays off and shows a warning when selected. Choose **Open Accessibility settings** in the warning to grant access. Lazy Worker hides the menu bar icon and panel when it detects screen sharing, including window sharing. It restores the icon when the share ends or the picker is canceled. The panel returns if it was open. Running tools continue during the share. Detection depends on the app and macOS version, and some apps only expose a signal after sharing starts.

## Quick start

Enable **Quick start** from a module header or its row in Tools. Both views share the same saved selection. **Start selected** starts the selected tools that are off, and **Stop selected** stops the selected tools that are running. Tools shows both actions when the selection has a mix of states. Use a row’s power button to start or stop that tool directly. Running sessions keep their original start time. Each module shows a session timer in its header. Quick start selections persist when the app closes or the Mac restarts.

## Activity

Open **How it works** in Activity to see what happens while a session is active.

- In **Duration**, choose 15 minutes, 30 minutes, one hour, or until stopped. The power button starts or stops a session immediately. The header counts down for a fixed-duration session and shows elapsed time for an open-ended or scheduled session.
- In **Daily schedule**, set a start and end time, then enable or update the schedule. A schedule can span midnight and runs while Lazy Worker is open. Turn it off to disarm it. Stopping a scheduled session with the power button skips the rest of that window.

The macOS helper holds an idle-sleep assertion while enabled. It measures the system idle time and moves the pointer briefly only after 45 seconds without input, at most once per 45 seconds. Resumed input postpones the next movement. The helper reads an elapsed time counter and records no keystrokes. Check Accessibility under **Settings → Accessibility**. Use **Open Accessibility** there or in an Activity error to open the macOS privacy pane. Enable Lazy Worker in the system list, return to the panel, and start Activity again. On Monterey, the pane is under **System Preferences → Security & Privacy → Privacy → Accessibility**. A development launch may require enabling the terminal or the helper. The activity indicator describes the local session. Presence in a third-party app depends on what that app observes.

## Caffeinate

Caffeinate uses the macOS system utility to hold sleep assertions for the display, system, or disk and to declare the user active. System sleep assertions apply on AC power. The user-active assertion defaults to five seconds without a timer.

Choose to run until stopped, for a duration, or while a running process stays open. Choose a process from the list or enter its PID. Stopping Caffeinate releases the sleep assertions. The target process can finish independently. Valid changes save automatically after a short pause and apply to a running session. Quick start uses the saved settings. Saved command mode migrates to Until stopped and drops the old executable and arguments.

## Develop

Install Node.js 22.12 or newer, Rust stable, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform. macOS needs the Xcode command-line tools, including Swift.

```sh
npm ci
npm run tauri -- dev
```

Use `npm run dev` to open the interface in a browser. Browser mode simulates module sessions and saves preferences in browser storage. The installed desktop app runs the native helper and schedule.

Run the checks with `npm run build` and `cargo test --manifest-path src-tauri/Cargo.toml`.

## Build for macOS

The app bundle and Swift helper target macOS 12.0 (Monterey) and later on Intel and Apple silicon.

```sh
rustup target add x86_64-apple-darwin
npm run tauri -- build --target x86_64-apple-darwin --bundles app
```

Use `aarch64-apple-darwin` to build for Apple silicon. Find the app under `src-tauri/target/<target>/release/bundle/macos/`.

Local macOS builds use an ad-hoc signature for testing. Distribution requires its own signing and notarization setup.

macOS may keep an Accessibility entry enabled after an ad-hoc update even when it no longer matches the installed app. If Activity reports a permission error while Lazy Worker is enabled, select its entry in Accessibility and click **−**. Click **+**, choose `/Applications/Lazy Worker.app`, and enable it again. Use a stable signing identity for builds that need their permission to survive updates. Test the packaged build on the actual Intel Mac before claiming Monterey support for a release.

### macOS updates

Lazy Worker checks GitHub Releases at launch and every four hours. A new version appears in the menu and in the panel. Choose **Later** to keep working and find the update again in Settings. **Restart to update** downloads and verifies the package, then replaces and restarts Lazy Worker. Restarting ends active sessions.

The public key and GitHub Releases endpoint are in `src-tauri/tauri.macos.conf.json`. After notarizing and stapling both app bundles, package each as a ZIP for manual installation and as an `.app.tar.gz` for the updater. Sign each updater archive with `tauri signer sign --app-version <version>`. Keep the Tauri private key outside the repository and use the same key for every update. Publish `latest.json` in the same release with `darwin-aarch64` and `darwin-x86_64` entries pointing to the corresponding archives and containing their `.sig` contents. See the [Tauri updater guide](https://v2.tauri.app/plugin/updater/).

## Add Windows support

Build the project on Windows using the [Windows development prerequisites](https://v2.tauri.app/start/prerequisites/#windows). The shared UI, scheduling rules, and preferences already have a platform boundary. The Activity and Caffeinate controls display an unavailable state on Windows until Windows adapters are added.

Implement the Windows worker alongside `src-tauri/src/activity/macos.rs` and select it by target in `src-tauri/src/activity/mod.rs`. A worker starts a session, reports whether it is running, and releases its resources when dropped. Keep the schedule and preferences outside that adapter.

## License

Lazy Worker is available under the [MIT License](LICENSE).
