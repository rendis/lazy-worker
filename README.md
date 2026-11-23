<p align="center"><img src="docs/images/hero.jpg" alt="Miga, a clay wind-up helper, stands on a workbench between a pointer, an espresso, an open padlock, and a toolbox."></p>

<h1 align="center">Lazy Worker</h1>

<p align="center"><strong>A toolbox for your Mac menu bar.</strong><br>
Small tools that keep locks and limits out of your way.</p>

<p align="center">
  <a href="https://github.com/rendis/lazy-worker/releases/latest"><img src="https://img.shields.io/badge/Download_for_macOS-c9603f?style=for-the-badge&logo=apple&logoColor=white" height="44" alt="Download for macOS"></a>
</p>

<p align="center"><sub>Free and open source · macOS 12 or later · Apple silicon and Intel</sub></p>

<p align="center">
  <a href="#tools">Tools</a> ·
  <a href="#what-each-tool-can-do">What each tool can do</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#features">Features</a> ·
  <a href="#install">Install</a> ·
  <a href="#privacy">Privacy</a> ·
  <a href="#contributing">Contributing</a> ·
  <a href="docs/user-guide.md">User guide</a>
</p>

<br>

## Why

Your Mac decides things for you. It falls asleep in the middle of a long job. Your chat marks you as away while you read. Your network blocks the AI service you need.

Lazy Worker gives you one small tool for each of these limits, and one place to run them all.

<br>

## Tools

Each tool does one job. Miga, the wind-up helper, shows the desk without the tool, then with it.

<br>

<!-- Tool block. Copy it for each new tool and alternate align="left" and align="right". -->
<img align="left" width="480" src="docs/images/activity.gif" alt="Without Activity, nobody touches the Mac and the chat status turns amber. With Activity, Miga pushes the pointer and the status turns green again.">

### Activity

Moves the pointer briefly when your Mac is idle. Run it on a timer or a daily schedule.

[Activity in the user guide →](docs/user-guide.md#activity)

<br clear="all">
<br>

<img align="right" width="480" src="docs/images/caffeinate.gif" alt="Without Caffeinate, the Mac falls asleep in the middle of a download. With Caffeinate, Miga offers it an espresso, it wakes up, and the download finishes.">

### Caffeinate

Keeps your Mac awake for a set time, while a process runs, or until you stop it.

[Caffeinate in the user guide →](docs/user-guide.md#caffeinate)

<br clear="all">
<br>

<img align="left" width="480" src="docs/images/proxy.gif" alt="Without Proxy, the Inspector blocks every request this Mac sends to its AI services. With Proxy, Miga throws a network cable to the other Mac, the requests reach the AI services through it, and the answers come back.">

### AI Proxy

Sends your AI tools' traffic through another computer on your network, when this one can't reach the service.

[Proxy in the user guide →](docs/user-guide.md#proxy)

<br clear="all">
<br>

## What each tool can do

This is the full list for each tool. The [user guide](docs/user-guide.md) shows how to use each item.

<!-- Capability list. Add one for each new tool, and one line for each new function. -->

**Activity**

- Moves the pointer briefly after 45 seconds without input, and at most once every 45 seconds. It waits while you use the Mac.
- Keeps the Mac from idle sleep while it runs.
- Runs for 15 minutes, 30 minutes, one hour, or until you stop it.
- Runs on a daily schedule with a start and an end time. A schedule can span midnight.
- Shows the time left or the time elapsed in its header.
- Needs the Accessibility permission. It records no keystrokes.

**Caffeinate**

- Prevents sleep with the macOS `caffeinate` utility. You choose what stays awake: the display, the system when idle, the system on AC power, or the disk.
- Can declare the user active.
- Runs until you stop it, for a set duration, or while a process you choose stays open. Choose the process from a list or enter its PID.
- Saves each change automatically and applies it to a running session.

**AI Proxy**

- Works in one of two roles. A **Server** shares its connection. A **Client** opens AI tools through a server.
- Server: runs an HTTP and HTTPS forward proxy inside the app, on a port you choose. It needs no other software.
- Server: shows its addresses to copy, as an IP address and as a `.local` name.
- Server: accepts only computers on your local network, or only the IP addresses you list under **Access**. It never connects to services on its own computer.
- Server: lists the computers that use it, with their open connections and the time since their last activity.
- Server: shows each request as it arrives. It keeps the last 200 in memory and clears them when it stops.
- Client: checks that the server answers at the address you enter.
- Client: opens Claude or Codex through the server, as the desktop app or as the command-line tool. It detects which ones are installed.
- Client: shows the command behind each tool. You can copy it, edit it, or restore it.
- Client: runs commands of your own for other tools, each with a name and an icon. `{server}` stands for the server address.

**Every tool**

- Starts and stops from the panel or from the menu bar, and keeps running with the panel closed.
- Joins Quick start, which starts your selected tools together, by hand or when Lazy Worker opens.
- Shows a timer for its session and a short help clip.

<br>

## How it works

<table>
  <tr>
    <td width="33%" valign="top">
      <h3>1 · Open</h3>
      <p>Click the wind-up key in your menu bar. The panel lists every tool.</p>
    </td>
    <td width="33%" valign="top">
      <h3>2 · Start</h3>
      <p>Each tool has its own power button. Quick start runs your selected tools together.</p>
    </td>
    <td width="33%" valign="top">
      <h3>3 · Close</h3>
      <p>Close the panel. Tools keep running until you stop them or quit Lazy Worker.</p>
    </td>
  </tr>
</table>

<br>

## Features

Every tool gets the same support from the toolbox.

<table>
  <tr>
    <td width="33%" valign="top"><strong>Menu bar control</strong><br>Start and stop tools from the wind-up key. The panel can stay closed.</td>
    <td width="33%" valign="top"><strong>Quick start</strong><br>Select your usual tools once. Start them together with one click.</td>
    <td width="33%" valign="top"><strong>Starts with your Mac</strong><br>Open Lazy Worker at login and let it run Quick start for you.</td>
  </tr>
  <tr>
    <td valign="top"><strong>Hidden while sharing</strong><br>The icon and the panel disappear when you share your screen.</td>
    <td valign="top"><strong>Your look and language</strong><br>Light or dark. English or Spanish. Or follow your system.</td>
    <td valign="top"><strong>Updates in place</strong><br>A new version appears in the menu. Restart to install it.</td>
  </tr>
</table>

<br>

## Install

You need no technical knowledge. Follow these four steps.

1. **Find out which Mac you have.** Click the Apple logo in the top left corner of the screen and choose **About This Mac**.
   - If you see **Chip** and a name that starts with **Apple**, such as Apple M1, you have **Apple silicon**.
   - If you see **Processor** and a name with **Intel**, you have **Intel**.
2. **Download the installer for your Mac.**
   - [Download for Apple silicon](https://github.com/rendis/lazy-worker/releases/latest/download/Lazy-Worker-apple-silicon.dmg)
   - [Download for Intel](https://github.com/rendis/lazy-worker/releases/latest/download/Lazy-Worker-intel.dmg)
3. **Install it.** Open the file you downloaded. A window shows Lazy Worker and the Applications folder. Drag **Lazy Worker** onto **Applications**.
4. **Open it.** Open **Lazy Worker** from Applications. A short setup guide asks for your language and your look, and shows each tool. After that, the wind-up key in the menu bar opens the panel.

The wrong installer does no harm. macOS tells you that it cannot open the app, and you download the other one.

### Let an AI agent install it

If you use an AI agent that can run commands on your Mac, such as Claude Code or Codex, give it this prompt:

```text
Install Lazy Worker on this Mac.

1. Run `uname -m`. `arm64` means Apple silicon. `x86_64` means Intel.
2. Download the installer for that Mac to a temporary folder:
   - Apple silicon: https://github.com/rendis/lazy-worker/releases/latest/download/Lazy-Worker-apple-silicon.dmg
   - Intel: https://github.com/rendis/lazy-worker/releases/latest/download/Lazy-Worker-intel.dmg
3. Run `spctl -a -t open --context context:primary-signature -vv <file>`. Go on only if the answer says "Notarized Developer ID".
4. Mount the file with `hdiutil attach`, copy "Lazy Worker.app" to /Applications with `ditto`, and unmount it with `hdiutil detach`.
5. Open the app with `open -a "Lazy Worker"`.

Tell me when the setup guide is on screen. Do not grant any permission for me.
```

Lazy Worker runs on macOS 12 or later. Activity and Hide while sharing need the Accessibility permission. The [user guide](docs/user-guide.md#accessibility) shows how to grant it.

<br>

## Privacy

- **No account.** Your settings stay on your Mac.
- **No keystrokes.** Activity reads how long your Mac has been idle. It records nothing you type.
- **Your traffic, your network.** Lazy Worker contacts GitHub Releases to check for updates. Proxy carries traffic only while you run it. By default, it serves only computers on your local network.

<br>

## Contributing

Bug reports, ideas for new tools, and pull requests are welcome. [Contributing](CONTRIBUTING.md) explains the workflow, the checks, and how to add a tool.

<br>

## Learn more

- [User guide](docs/user-guide.md) — Every tool, Quick start, permissions, and updates.
- [Development guide](docs/development.md) — Project layout, local runs, builds, and releases.
- [Contributing](CONTRIBUTING.md) — Workflow, guidelines, and the checklist for a new tool.
- [Issues](https://github.com/rendis/lazy-worker/issues) — Report a problem or ask for a tool.

<br>

<p align="center"><sub>Released under the <a href="LICENSE">MIT License</a>.</sub></p>
