# Contributing

Thanks for helping with Lazy Worker. It is a toolbox: each tool removes one limit, and every tool shares the same panel, menu, and Quick start.

## Before you start

- **Report a bug.** [Open an issue](https://github.com/rendis/lazy-worker/issues) with your Mac's architecture (Apple silicon or Intel), your macOS version, the Lazy Worker version from **Settings**, and the steps that show the problem.
- **Propose a tool or a larger change.** Open an issue first. Describe the limit it removes and who meets it. Agree on the scope before you write code.
- **Fix something small.** A typo or a clear bug can go straight to a pull request.

## Set up

Follow [Run locally](docs/development.md#run-locally) in the development guide. [Project layout](docs/development.md#project-layout) shows where each part lives.

## Workflow

1. Fork the repository and branch from `main`. Name the branch after the change, for example `feat/clipboard-tool` or `fix/schedule-midnight`.
2. Read [AGENTS.md](AGENTS.md). It holds the project rules, and they apply to people as much as to agents.
3. Keep the change focused on one purpose. Put unrelated fixes in their own pull request.
4. Run the checks:

   ```sh
   npm run build
   cargo test --manifest-path src-tauri/Cargo.toml
   ```

   For changes to macOS integration, also build the app with the command in [Build for macOS](docs/development.md#build-for-macos) and test the packaged app.
5. Write commit messages in [Conventional Commits](https://www.conventionalcommits.org/) form: `feat:`, `fix:`, `docs:`, `refactor:`, `build:`, `ci:`, or `chore:`, then one line in the imperative. For example, `fix: keep the scroll bar from shifting the content`.
6. Open a pull request. Say what changed and why. Say how you tested it, with the architecture and macOS version. Add a screenshot for any visible change.

The [Check workflow](.github/workflows/check.yml) runs the same checks on every pull request. A pull request merges when the checks pass and a maintainer approves it.

## Add a tool

A new tool touches the same places every time. Use this list, and see [AGENTS.md](AGENTS.md) for the detail behind each step.

1. **Register it.** Add the module to `src/modules.ts` and mount the shared header, so its help, status, timer, and Quick start switch match the other tools.
2. **Name it in both languages.** Add its English and Spanish strings to `src/messages.ts`, and its menu entries to `src-tauri/src/tray.rs`.
3. **Keep the system behind an adapter.** Reach system APIs only from a platform adapter, with an unsupported-platform fallback. Shared scheduling and preferences must work without the adapter.
4. **Show times through `src/time.ts`.** A tool can stay on for weeks.
5. **Make its clip.** Build an episode in the [clay workshop](docs/design/clay-workshop/README.md), run `npm run export -- <tool>` there, and add the clip and poster to the tool's help in `src/modules.ts`.
6. **Document it.** Add a section to the [user guide](docs/user-guide.md). Copy the tool block in the [README](README.md), switch the clip to the other side, and link it to the new section. List what the tool can do under **What each tool can do** there, one line for each function. A change that adds a function to a tool adds its line too.
7. **Announce it.** Add the tool to the next release in `src/changelog.ts` with its `tool`, so What’s new links to it. Add its card to the tools step of the setup guide in `index.html` when a new user should meet it there.
8. **Record what you learned.** Add one line to [AGENTS.md](AGENTS.md) for each new project rule.

### Where a tool is registered

Step 1 touches these places. The TypeScript and Rust compilers report most of the ones you miss, because each tool is a key in a typed record or an arm in a `match`.

| File | What to add |
| --- | --- |
| `src/modules.ts` | The tool's ID in `ToolId` and its entry in `tools`: title, description, icon, and help. Add `section` when it sits under a broader sidebar entry, and `row` when its Tools row belongs to a group. |
| `index.html` | A `data-module-view="<id>"` block that holds a `.module-body`. The shared header mounts before that body. |
| `src/desktop.ts` | The tool in `defaults.quickStart`, in `previewStatuses`, and in the list `startPreviewTools` walks. Give each new native call a browser-preview result. |
| `src/main.ts` | The tool in the initial `statuses`, in `startFailures`, and in `unsupportedTools`. Import each new icon and add it to `renderIcons`. |
| `src-tauri/src/<tool>.rs` | The tool itself: its settings, its status, and its worker behind a platform adapter. |
| `src-tauri/src/main.rs` | The tool in `ToolId`, `Statuses`, and `Tools`, in `set_active` and `start_selected`, in the one-second clock, and in the stop on exit. Register each new command in `generate_handler!`. |
| `src-tauri/src/preferences.rs` | The tool's flag in `QuickStart`, and its settings in `Preferences` when it has any. |
| `src-tauri/src/tray.rs` | The tool's state, its start and stop labels in both languages, its menu item, and its action. |

Run `npm run dev` to work on the panel in a browser with simulated sessions, and `npm run tauri -- dev` for the native app.

## Guidelines

- Write code, comments, and documentation in English. Keep every interface string in English and Spanish.
- Prefer what the project, Tauri, and the installed packages already offer. A few lines of code never justify a new dependency.
- Write documentation in plain language: short sentences, one idea per sentence, the same word for the same thing.
- Never commit secrets or signing keys. Release settings live in `.release.local.json`, which git ignores.
- Only maintainers publish releases. The steps are in [Publish a macOS release](docs/development.md#publish-a-macos-release).

## License

By contributing, you agree that your work is released under the [MIT License](LICENSE).
