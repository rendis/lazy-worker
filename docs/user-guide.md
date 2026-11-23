# User guide

- [Get started](#get-started)
- [Menu bar](#menu-bar)
- [Quick start](#quick-start)
- [Activity](#activity)
- [Caffeinate](#caffeinate)
- [AI](#ai): [Proxy](#proxy)
- [Hide while sharing](#hide-while-sharing)
- [Accessibility](#accessibility)
- [Updates](#updates)

## Get started

The first time you open Lazy Worker, a setup guide asks for your language and your look. It offers to open Lazy Worker when you sign in, asks for the [Accessibility permission](#accessibility), and shows Activity and Caffeinate so you can mark them for Quick start. Each choice is saved at once, and **Settings** keeps all of them. **Skip setup** closes the guide, and you can grant the permission later. Opening at login starts on, and the guide applies it when you leave that step or skip.

After the guide, Lazy Worker opens in **Tools**. Select **?** next to a module’s title to see what the tool does, with a short clip. Select the clip to enlarge it.

In **Settings**, choose Light or Dark and English or Spanish, or leave either setting on System. For language, System uses your first supported preferred language and falls back to English.

## Menu bar

Use the wind-up key in the menu bar to open the panel, start the selected tools, or start and stop individual tools. A terracotta dot appears beside the key while at least one tool is running. Menu actions use saved settings. Close the panel to leave Lazy Worker running in the menu bar. Opening Lazy Worker again brings back the existing panel. Choose **Quit** from the menu to exit and release its active sessions.

## Quick start

Enable **Quick start** from a module header or its row in Tools. Both views share the same saved selection. **Start selected** starts the selected tools that are off, and **Stop selected** stops the selected tools that are running. Tools shows both actions when the selection has a mix of states. Use a row’s power button to start or stop that tool directly. Running sessions keep their original start time. Each module shows a session timer in its header. Quick start selections persist when the app closes or the Mac restarts.

In **Settings**, turn on **Open Lazy Worker at login** to open the installed app when you sign in to macOS, including after a restart. Turn on **Run Quick start when the app opens** to start the selected tools with their saved settings each time Lazy Worker starts. Both options default to off and can be used independently.

## Activity

Activity needs the [Accessibility permission](#accessibility).

- In **Duration**, choose 15 minutes, 30 minutes, one hour, or until stopped. The power button starts or stops a session immediately. The header counts down for a fixed-duration session and shows elapsed time for an open-ended or scheduled session.
- In **Daily schedule**, set a start and end time, then enable or update the schedule. A schedule can span midnight and runs while Lazy Worker is open. Turn it off to disarm it. Stopping a scheduled session with the power button skips the rest of that window.

The macOS helper holds an idle-sleep assertion while enabled. It measures the system idle time and moves the pointer briefly only after 45 seconds without input, at most once per 45 seconds. Resumed input postpones the next movement. The helper reads an elapsed time counter and records no keystrokes. The activity indicator describes the local session. Presence in a third-party app depends on what that app observes.

## Caffeinate

Caffeinate uses the macOS system utility to hold sleep assertions for the display, system, or disk and to declare the user active. System sleep assertions apply on AC power. The user-active assertion defaults to five seconds without a timer.

Choose to run until stopped, for a duration, or while a running process stays open. Choose a process from the list or enter its PID. Stopping Caffeinate releases the sleep assertions. The target process can finish independently. Valid changes save automatically after a short pause and apply to a running session. Quick start uses the saved settings. Saved command mode migrates to Until stopped and drops the old executable and arguments.

## AI

### Proxy

Use Proxy when one computer cannot reach an AI service and another computer on the same network can. The second computer shares its connection, and the AI tool on the first one sends its traffic through it.

On the computer that shares its connection:

1. Open **AI** and choose **Server**.
2. Turn on the power button. The header shows the session timer.
3. Copy an address under **Address**. The `.local` name keeps working when the IP address changes.

On the computer that uses the connection:

1. Open **AI** and choose **Client**.
2. Enter the server address with its port, for example `studio.local:8899`. **Available** confirms that the server answers.
3. Select **Open** next to a tool. **Open** stays off until the server address is set, and its tooltip says why.

- **Access** limits the server to the IP addresses you add. The field accepts digits and dots only. A row shows three addresses, and **+N more** opens the full list. With an empty list, the server accepts computers on your local network only. The server never connects to services on its own computer.
- **Connected** lists the computers that used the server, their open connections, and the time since their last activity. Select **+** next to a computer to add it to **Access**.
- **Requests** shows each request as it arrives: its time, the computer, the target, and the result. Select its button to open the list in a window over the view. The server keeps the last 200 in memory and clears them when it turns off.
- Claude and Codex have one row each. Choose **App** for the desktop app or **Code** for the command-line tool. A tool that is not installed shows **Not detected** and stays off.
- The copy button next to **Open** copies the command, so you can run it in the terminal you prefer.
- A command-line tool needs a terminal for its interface, so it opens in Terminal, in your home folder. The desktop app opens directly.
- Select the arrow in a row to see its command. **Edit** changes it, and `{server}` stands for the server address. **Restore** brings back the built-in command. Switching between App and Code also restores it.
- **Other command** adds a tool of your own. Enter a name and the command that starts it. The command runs as you write it. Write `{server}` where the server address goes. **Delete** asks before it removes the command.
- The Quick start switch works on the server and on each client. **Start selected** starts a marked server and opens each marked client in the form you chose, App or Code. **Stop selected** stops the server and leaves the clients open.

## Hide while sharing

**Settings → Hide Lazy Worker while sharing** defaults to on once [Accessibility](#accessibility) is granted. Without permission, the switch stays off and shows a warning when selected. Choose **Open Accessibility settings** in the warning to grant access.

Lazy Worker hides the menu bar icon and panel when it detects screen sharing, including window sharing. It restores the icon when the share ends or the picker is canceled. The panel returns if it was open. Running tools continue during the share. Detection depends on the app and macOS version, and some apps only expose a signal after sharing starts.

## Accessibility

Activity and Hide while sharing need the Accessibility permission. Check it under **Settings → Accessibility**.

1. Select **Open Accessibility** there or in an Activity error. It opens the macOS privacy pane. On Monterey, the pane is under **System Preferences → Security & Privacy → Privacy → Accessibility**.
2. Enable Lazy Worker in the system list.
3. Return to the panel and start Activity again.

If Activity still reports a permission error while Lazy Worker is enabled, select its entry in the list and click **−**. Click **+**, choose `/Applications/Lazy Worker.app`, and enable it again.

## Updates

Lazy Worker checks GitHub Releases at launch and every hour. It also checks when you open the panel and the last check is older than 15 minutes. After a check that fails, for example with no network, it tries again in five minutes. **Settings** shows the time of the last check beside the version, and **Check for updates** checks at once. After an update, **What’s new** shows the changes once. **What’s new** in Settings lists the changes of the last versions. A new version appears in the menu and in the panel. Choose **Later** to keep working and find the update again in Settings. **Restart to update** downloads and verifies the package, then replaces and restarts Lazy Worker. The restart stops the tools that are on, and the new version starts them again with their saved settings. Their sessions start from zero. A Proxy server drops its open connections for the restart, and the clients you opened stay open.
