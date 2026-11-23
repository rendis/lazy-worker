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

1. Open the [latest release](https://github.com/rendis/lazy-worker/releases/latest). Choose the ZIP ending in `arm64.zip` for Apple silicon or `x86_64.zip` for Intel.
2. Unzip it, move **Lazy Worker** to **Applications**, and open it.
3. Click the wind-up key in the menu bar and choose **Open Panel**.

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
