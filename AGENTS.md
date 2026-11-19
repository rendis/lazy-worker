# Lazy Worker working notes

- Write code, comments, and documentation in English. Keep English and Spanish webview strings together in `src/messages.ts`. Localize the native menu in `src-tauri/src/tray.rs`.
- Hide the macOS panel on close and keep workers running until explicitly stopped or the app quits.
- Acquire the macOS instance lock before app setup. Route repeat launches through the shared panel show path.
- Register launch at login through a per-user LaunchAgent that opens the installed app bundle. Run startup Quick start once after acquiring the instance lock.
- Keep the macOS menu icon legible in the system menu bar appearance and reserve space for its activity badge in both states.
- Hide the existing `NSStatusItem` during a presentation so its activity badge survives restoration.
- Keep the native panel fixed-size in the Tauri window configuration. Keep desktop scrolling inside `<main>` so the sidebar stays fixed.
- Crossfade the theme canvas layers and keep the Settings card geometry stable when its status text is translated.
- Read the full macOS preferred-language list in native code because WebKit may expose only its first entry.
- Register each module in `src/modules.ts` and mount its shared header so its icon, status, timer, and quick-start switch stay consistent with Tools. Keep the timer slot reserved when idle so it can fade without moving the layout.
- Derive Tools bulk actions from the runtime state of modules included in Quick start. Keep the action layout stable until a bulk operation finishes. Share one surface across single actions so only their icon and label crossfade. Keep row power available independently of the selection.
- Keep active power buttons visibly recessed in terracotta until their module stops.
- Derive each sidebar running dot from its module's runtime status, independent of the current page.
- Keep system input and power APIs inside the activity adapters. Shared scheduling and preferences must work with the unsupported-platform adapter.
- List Caffeinate process targets by PID and executable basename. Preserve the current sleep assertion if switching to a process fails.
- Check Accessibility in the same helper that posts Activity events and keep feedback outside the module layout.
- Anchor permission notices to their warning icon and keep them outside the scrolling content.
- Use one pending Accessibility status in Settings for Activity and hiding while sharing. Give each feature its own icon.
- Dismiss permission notices on outside clicks and keep general results in the shared bottom-right notification stack.
- Gate pointer movement on the system idle counter and skip it while any standard mouse button is held.
- Keep both the Tauri bundle and Swift helper deployment targets at macOS 12.0. Keep the frontend build target compatible with Monterey's WebKit.
- Use `npm run build` and `cargo test --manifest-path src-tauri/Cargo.toml` for changes to the shared app. Use the Intel bundle command in the README for changes to macOS integration.
- Verify the packaged helper and Accessibility permission on Monterey before reporting hardware compatibility. Verify presence in the target app separately from process status.

## macOS releases

- For signing, Accessibility after an update, or release automation, follow [issue #1](https://github.com/rendis/lazy-worker/issues/1). Describe test platforms by architecture and macOS version.
- Set `APPLE_SIGNING_IDENTITY` to the Developer ID Application identity for release builds. Verify the app and its Activity helper against that certificate in both architecture bundles.
- Require the Tauri updater to verify the signed version. Call `request_restart` after installing so `RunEvent::Exit` releases active tools.
- Local builds default to ad-hoc signing. If Accessibility fails after reinstall, compare the installed app's CDHash with the requirement in the macOS TCC log. Keep the app and Activity helper signing identities stable across releases. Keep private signing keys outside the repository.
- Archive the stapled app for the Tauri updater and bind its signature to the release version before publishing `latest.json`.
