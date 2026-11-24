# User guide

- [Get started](#get-started)
- [Menu bar](#menu-bar)
- [Quick start](#quick-start)
- [Activity](#activity)
- [Caffeinate](#caffeinate)
- [AI](#ai): [Proxy](#proxy)
- [Devices](#devices)
- [Security](#security)
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

Enable **Quick start** from a module header or its card in Tools. Both views share the same saved selection. **Start selected** starts the selected tools that are off, and **Stop selected** stops the selected tools that are running. Tools shows both actions when the selection has a mix of states. Use a row’s power button to start or stop that tool directly. Tools shows each tool as a card. Pin a card to keep it at the front. The AI Proxy Server card shows the server; open it to reach the AI tools. Running sessions keep their original start time. Each module shows a session timer in its header. Quick start selections persist when the app closes or the Mac restarts.

In **Settings**, turn on **Open Lazy Worker at login** to open the installed app when you sign in to macOS, including after a restart. Turn on **Run Quick start when the app opens** to start the selected tools with their saved settings each time Lazy Worker starts. Both options default to off and can be used independently.

## Activity

<img src="images/activity.gif" width="560" alt="Without Activity, nobody touches the Mac and the chat status turns amber. With Activity, Miga pushes the pointer and the status turns green again.">

Activity needs the [Accessibility permission](#accessibility).

- In **Duration**, choose 15 minutes, 30 minutes, one hour, or until stopped. The power button starts or stops a session immediately. The header counts down for a fixed-duration session and shows elapsed time for an open-ended or scheduled session.
- In **Daily schedule**, set a start and end time, then enable or update the schedule. A schedule can span midnight and runs while Lazy Worker is open. Turn it off to disarm it. Stopping a scheduled session with the power button skips the rest of that window.

The macOS helper holds an idle-sleep assertion while enabled. It measures the system idle time and moves the pointer briefly only after 45 seconds without input, at most once per 45 seconds. Resumed input postpones the next movement. The helper reads an elapsed time counter and records no keystrokes. The activity indicator describes the local session. Presence in a third-party app depends on what that app observes.

## Caffeinate

<img src="images/caffeinate.gif" width="560" alt="Without Caffeinate, the Mac falls asleep in the middle of a download. With Caffeinate, Miga offers it an espresso, it wakes up, and the download finishes.">

Caffeinate keeps the Mac awake with the macOS `caffeinate` utility.

- **Keep the Mac awake** has two choices. **On charger** keeps the Mac awake while the charger is connected, and lets it sleep as usual on battery. **Always** keeps it awake on battery too. A new installation starts with **On charger**.
- **Keep the screen on** stops the screen from turning off. With it, the Mac stays awake on battery too.
- **Advanced** holds two rare options and the command that runs, with a button that copies it. **Keep the disk awake** is for an external hard disk that pauses when idle. **Turn the screen on at start** wakes the screen when Caffeinate starts; without a timer its effect lasts five seconds.

Choose to run until stopped, for a duration, or while a running process stays open. Choose a process from the list or enter its PID. Stopping Caffeinate releases the sleep assertions. The target process can finish independently. Valid changes save automatically after a short pause and apply to a running session. Quick start uses the saved settings. Saved command mode migrates to Until stopped and drops the old executable and arguments.

**Stay awake with the lid closed** decides what the Mac does when you close its lid with no screen connected. The sleep options above do not cover that case.

- **Never** leaves macOS as it is: the Mac sleeps.
- **On charger** keeps the Mac awake while the charger is connected. On battery, the Mac sleeps.
- **Always** keeps the Mac awake on battery too. A closed Mac inside a bag gets hot, so the row shows a warning.

The choice applies only while Caffeinate is on. When Caffeinate stops or Lazy Worker quits, the Mac sleeps as usual again. The first time you choose **On charger** or **Always**, macOS asks for an administrator password. Lazy Worker never sees it. The password installs one rule, `/etc/sudoers.d/lazy-worker-lid`, that lets your user run `pmset -a disablesleep` without a password. Delete that file to take the permission back.

If your Mac already stays awake with the lid closed, because of a setting you made before, the row shows **Always** and Lazy Worker leaves that setting alone. Choose **Never** or **On charger** to put Lazy Worker in charge of it.

## AI

### Proxy

<img src="images/proxy.gif" width="560" alt="Without Proxy, the Inspector blocks every request this Mac sends to its AI services. With Proxy, Miga throws a network cable to the other Mac, the requests reach the AI services through it, and the answers come back.">

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

## Devices

Devices lets you control one device from another. It is not a tool, so it has no power button. Use it to start a tool, open an application, or run a saved action on a device that is not in front of you. Both devices need Lazy Worker. They talk over SSH, the secure connection that macOS already has. Every device decides what the other one may do.

Open **Devices** in the sidebar. **This device** comes first. The devices you control from here follow it. One device is open at a time. Select a device to open it.

Each open device has tabs on one line. **This device** has **Access** (remote access, pairing, and **Cut all access**), **Who controls it**, **Actions**, and **Log**. A device that you control has **Tools**, **Applications**, **Actions**, and **Connect**. A small badge after the name of a tab shows its state, for example **1/3** tools on, the number of applications that are open, the number of actions, or a green dot when a way into the device is on. Every tab has the same height. A tab with more rows than fit scrolls inside. **Applications** lists your pinned applications first, then the ones you opened last from here, and a count that opens the rest. **Actions** has a search field. **Connect** lists the ways into the device: a terminal, the screen, and files. Use the arrow keys, **Home**, and **End** to move between tabs. Each device remembers its last tab.

Each device you control shows if it is online. Lazy Worker checks every few seconds while Devices is open, and when you come back to the window. Select **Check this device now** (the circular arrow) to check at once and read its state again.

### Turn on remote access

Do this on each device that others will control.

1. Under **This device**, select **Turn on remote access**.
2. Enter your administrator password when macOS asks. Lazy Worker never sees it.

This turns on Remote Login for your user and limits it to keys. Nobody can log in with a password. The row then shows the real state, for example **Only paired keys can log in**.

If macOS does not let Lazy Worker turn it on, a guide opens. Follow its steps: select **Open Sharing settings**, turn on **Remote Login**, and make sure your user is allowed. Lazy Worker checks again by itself when you close the window.

If the row says that passwords can still log in, select **Allow passwords again** only when you want that. Lazy Worker removes only its own setting file and asks you to confirm first.

### Pair a device

Pairing links two devices with a code that works once. Start on the device you want to control.

1. On that device, open **Devices** and select **Open pairing**.
2. Choose what the new device may do. **Control** starts on. **Applications** and **Actions** start off. Shell is never part of pairing.
3. Select **Open for 10 minutes**. The device shows a 6-digit code, the user name to enter, and the address to reach it.
4. On the other device, open **Devices** and select **Add device**.
5. Choose the device in the list. If it is not there, select **Enter an address**. Use this for a device on a VPN such as Tailscale, for example `studio.local` or `100.64.0.5`. Add `:port` if the port is not 22.
6. On the same network, a device with an open pairing window shows **Pairing open**, and its user name fills in by itself. Over a VPN or with an address you typed, enter the user name. Then enter the code and select **Pair**.

The code is not sent over the network. Three wrong codes close the window, and a code works once. The device shows when the pairing is done. Close the pairing window with **Close pairing** if you change your mind.

### What a device may do

Each device that can control this one has five switches under **Who can control it**. Change them at any time.

The pairing window and the tip of each switch say the same in one sentence:

- **Control**: turn tools on and off and change their settings here. It can also see the names of open programs, and update and restart Lazy Worker.
- **Applications**: open any application installed here, including Terminal. It cannot type into it.
- **Actions**: run the commands you saved here and see what they print. It cannot see or change the commands.
- **Files** lets it open the folders you choose, see and copy files, and, if you allow it, add, replace, and delete them. You turn it on only at this device, for a time. Pairing never turns it on, and a remote window cannot turn it on.
- **Shell** lets it open a full terminal on this device. You turn it on only at this device. Pairing never turns it on, and a remote window cannot turn it on.

Some settings stay with the person at the device, even with **Control**. Examples are starting the proxy server, its address lists, and turning on Quick start at launch. The remote window shows these controls as off and says to do them on that device.

### Use the remote window

Select **Open interface** next to a device. A window of Lazy Worker opens for that device. A banner at the top names it.

- Every action in the window runs on that device, and that device checks it.
- The banner turns amber when the link is down. The window connects again by itself.
- If the two devices run different versions, the banner offers **Update** for that device.
- The language and the theme stay yours. They always come from the device in front of you.
- A control that the device does not allow is off, with the reason. Either the device did not allow it, or only the person at that device can do it.

In the **Tools** tab of a device you control, each tool shows its state. You can turn a tool on or off there.

### Applications

With **Applications** allowed, a device shows its installed applications in its **Applications** tab. Each shows whether it is open. Pin up to three that you use often. Search the rest. Select **Open** to open an application on that device. A device opens only applications from its own list.

### Actions and schedules

An action is a named shell command that you save on a device. Each device keeps its own actions. You create, change, and delete an action only at that device. The command runs as your user, with your permissions. Add only commands you trust.

- In the **Actions** tab of **This device**, select **Add action**. Give it a name, a command, and a time limit. The action stops by itself when the time passes.
- Next to the command, **Test in my terminal** runs the text you typed in your own terminal, the same way the action runs, before you save it. **Copy command** copies the line that does it, to paste in any terminal. Both work only at the device that owns the action.
- A device with **Actions** allowed can run your actions and stop them. It can also follow the output. It cannot read the command.
- Select **Schedule** next to an action to schedule it: once, daily, or weekly. The action runs on the device that owns it, while Lazy Worker is open there. If that time passes while the app is closed, Lazy Worker skips it and does not run late.
- Turn on **Flag it if it fails** to see a failed run in the row until a run works.
- Two actions run at the same time at most.
- A schedule belongs to the one who set it. A device can add a schedule to an action that has none, and it can change or clear its own. It cannot change one that you or another device set. You can change any schedule here.
- When you remove a device, turn off its **Actions**, or cut all access, its schedules go and the runs it started stop. A schedule of a device that lost **Actions** never runs, and the log says it was removed.

### Full shell

A full shell lets another device run any command on this device, as your user. Only you can allow it, only at this device.

1. In the **Who controls it** tab, select **Shell** on the row of a device.
2. Read the warning. Choose **1 hour**, **2 hours**, **Custom** (1 minute to 24 hours), or **Until I turn it off**.
3. Select **Allow shell**.

The other device then shows **Terminal** in its **Connect** tab, with buttons to open it in Terminal or copy the command.

The warning says that, while the shell is on, the other device can leave itself permanent access, for example by adding a key. Turning the shell off does not undo that. Lazy Worker watches `~/.ssh/authorized_keys` while a shell is open. It tells you when that file changes. Check it then.

The shell turns off by itself when the time ends. Select **Turn off shell** to end it sooner. This also ends open sessions.

### Files

Files lets another device open folders on this device and move files in both directions. Everything goes through the same SSH connection as the rest of Devices. It adds no network service and no sharing in System Settings. Only you can allow it, only at this device.

1. In the **Who controls it** tab, select **Files** on the row of a device.
2. Under **Where**, choose **Only these folders** or **Whole disk**. For folders, use **Add folder…** and pick each folder. **~/Lazy Worker** is offered first. Lazy Worker creates it the first time you share it. You can share up to 16 folders.
3. Under **Permission**, choose **Read only** or **Read and write**.
4. Under **Duration**, choose **1 hour**, **2 hours**, **Custom** (1 minute to 24 hours), or **Until I turn it off**.
5. Select **Share files**.

**Whole disk** shows a warning: the device will be able to see, copy, and change all your files, including personal documents, mail, and app data. Choose it only for a device you trust. Lazy Worker's own data and your SSH keys are never shared, with either choice.

The chip then says **Files · until 17:00**. Select it to stop sharing. Lazy Worker asks first, in the row. Stopping, or the end of the time, takes effect on the next request and ends the transfers in progress. An upload that did not finish leaves no partial file.

On the other device, the **Connect** tab shows a **Files** row, for example **2 shared folders · read and write**. Select **Open the files of that device** to open the explorer:

- The shared folders are on the left. The path is above the list. Select **Name** to sort. Double-click a folder to open it. Backspace goes up.
- **Send files here** picks files on your device and sends them to the open folder. You can also drag files from Finder onto the window. A file that has the same name is replaced.
- **New folder** makes a folder in the open one.
- **Save a copy on this device** saves the selected files. One file asks where to save it, starting in Downloads. Several ask for a folder, and a name that is taken gets a number. Folders cannot be saved yet.
- **Delete** asks in the toolbar before it deletes. A folder is deleted with everything in it. A folder with more than 10,000 items is refused.
- **Refresh** reads the folder again.
- The bar at the bottom shows the transfer in progress, with **Stop this transfer**. Several transfers wait in line.

With **Read only**, the buttons that change files are off, and their tip says why. Files larger than 4 GB are refused. You cannot drag files out of the window to Finder yet. Use **Save a copy on this device**.

### Screen sharing

Screen sharing uses the Screen Sharing that macOS already has. Lazy Worker adds no permission of its own for it. macOS asks for the password of the account on the other device and decides who gets in. Any device with **Control** sees if it is on, but only the device itself can turn it on.

To share a screen, do this on the device that is shared:

1. Under **This device**, in **Access**, select **Turn on screen sharing**.
2. Enter your administrator password when macOS asks. Lazy Worker never sees it.

The row then says **Open to your network, protected by this account's password**. If macOS does not let Lazy Worker turn it on, a guide opens with the steps: select **Open Sharing settings**, turn on **Screen Sharing**, and make sure your user is allowed. macOS may ask for your password there. The guide has no picture yet.

To open the screen of a device you control, open its **Connect** tab and select **Open screen** on the **Screen** row. The button works only while the row says that screen sharing is on. Screen Sharing opens, and macOS asks for the password of the account.

### The log

Open the **Log** tab of **This device** to see what other devices asked of this one. It scrolls inside the tab. Each line shows the time, the device, the request in words, and the result. It shows requests, changes, and refusals. It does not show what was inside a request. It does not list a read that worked.

The log keeps security events apart from the rest: pairing, removing a device, shell and files turned on or off, shell sessions, changes to the keys file, files received, sent, and deleted (the path inside the shared folder and the size, never the content), and runs of actions. Each group keeps its last 1000 lines, so a device that sends many refused requests cannot push a security event out. A refusal that repeats within one minute is one line with a count, for example **×7**.

### Remove a device

- To stop a device from controlling this one, select **Remove access** next to it. The next request from that device is refused, also on a connection that is open.
- **Cut all access** removes every device at once and closes any open pairing window. Lazy Worker asks you to confirm first. It does not turn off Remote Login. The **Remote access** row then says that Remote Login is still on, with keys only. If you do not need it, turn it off in Sharing settings.
- To stop controlling a device, select **Forget this device** on the controller. This removes it only from that device. Your key stays on the other device until you remove the access there too. Lazy Worker reminds you.

### Troubleshooting

- **Remote access is off.** The other device cannot connect. Turn on remote access on the device you want to control. The row says what is wrong.
- **“<name> no longer accepts this device.”** The other device removed your access, or its pairing data changed. Pair the two devices again.
- **“That device has a different identity than before.”** Stop. Lazy Worker saved the identity of the device when you paired. A different one can mean another device is pretending. Check the address. Check that the device was not reset. Do not continue until you are sure. Then forget the device and pair again.
- **macOS asks to find devices on the local network.** Allow it. Lazy Worker needs it to list the devices on this network. Without it, use **Enter an address**.
- **Cannot reach the device.** Check that the device is on and awake. Check that your firewall and your VPN let port 22 through. Lazy Worker accepts connections only from private networks, link-local addresses, and the Tailscale range.
- **Pairing is not open on that device.** Select **Open pairing** there, and try again within 10 minutes.

## Security

Devices lets other devices act on yours, so it is built to be safe by default.

### What is safe by default

- Nothing is reachable until you turn on remote access.
- Lazy Worker sets SSH to accept keys only.
- A new device gets **Control** only. **Applications** and **Actions** start off. **Files** and **Shell** are never on at pairing.
- Lazy Worker checks each request again, on the device that receives it, against the permissions it has at that moment. Removing a device works at once.
- Files are shared only in the folders you choose, for a time. Each path is resolved on this device and must stay inside its shared folder. A link that leaves it, a device file, a socket, and a pipe are refused. Lazy Worker's data and your SSH keys are never shared, also with **Whole disk**.
- A device never sends the text of its saved commands to another device.
- Names and text from another device show as plain text. They cannot run anything.
- The pairing code is not sent. Three wrong codes close the window.
- While a pairing window is open, and only then, the device announces its user name and device id on your local network, so the other device can fill in the user name. The announcement never holds the code. The 6-digit code that SPAKE2 checks is still needed to pair, so a forged announcement cannot pair a device. At most it fills in a wrong user name.

### What is up to you

- Commands you save as actions, and commands you write for Proxy tools, run as your user. Save only what you trust.
- Allowing a **Shell** gives the other device the same power as you. Allow it only for a time you choose, and only to a device you trust.
- **Files** with **Read and write** lets the other device change and delete files in the shared folders. **Whole disk** shares everything this account can open. macOS still decides what Lazy Worker may read, so some folders, such as Mail, can ask for permission or stay closed.
- **Allow full disk access for remote users** in Sharing is not needed and is safer off. If it is on, a shell can read all files, including Mail and Messages.

### Risks that remain

- A program that runs as your user on the same device can act as you. It can also use the same local socket that remote requests use.
- The SSH server of macOS stays reachable on your networks while remote access is on. Only keys can log in.
- If someone takes an unlocked device, that device keeps its permissions on your other devices. Remove it there, and use **Cut all access** if needed.
- The pairing key in the app is public on purpose. It opens only the pairing step, and only while a pairing window is open. The 6-digit code is the secret.
- The code check uses the SPAKE2 method. The library that does it has not had an independent audit.
- A shell can leave permanent access. Lazy Worker warns you, and it cannot undo it.

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

Lazy Worker checks GitHub Releases at launch and every hour. It also checks when you open the panel and the last check is older than 15 minutes. After a check that fails, for example with no network, it tries again in five minutes. **Settings** shows the time of the last check beside the version, and **Check for updates** checks at once. After an update, **What’s new** shows the changes once. **What’s new** in Settings lists the changes of the last versions. A new version appears in the menu and in the panel. Choose **Later** to keep working and find the update again in Settings. **Restart to update** looks for a newer version once more, so one restart always reaches the latest. It downloads and verifies the package, then replaces and restarts Lazy Worker. The restart stops the tools that are on, and the new version starts them again with their saved settings. Their sessions start from zero. A Proxy server drops its open connections for the restart, and the clients you opened stay open.
