# Miga's clay workshop

## Cast

- **Miga, the hero.** She is the tool. Start with her [six-pose model sheet](assets/miga-poses.webp). Her tapered terracotta body, low cap ridge, cream eyes, dark brows, mitten hands and two-loop brass key identify her in every shot. Keep the key visible on her back. She solves each problem with her hands.
- **The Inspector, the antagonist.** A corporate eyeball in a shirt and a burgundy tie. He watches, checks his clipboard and blocks. Start with his [six-pose model sheet](assets/inspector-poses.webp): watching, checking, blocking, fooled, outsmarted and grumpy. He always loses: the tool either fools him or goes around him.

## Episode layout

Each episode explains one tool in seven to ten seconds. It shows the problem without the tool, then the same moment with it. It carries no text or sound. The README and the app show the same clip at about 480 px wide and put their own caption below it.

| Beat | Activity | Caffeinate | AI Proxy | Reuse in the next episode |
| --- | --- | --- | --- | --- |
| Without | The chair is still turning. The camera pushes in on the chat while the idle ring runs out. The status turns amber and the inspector leans out from behind the monitor. | The chair is still turning and a job is running. The screen's eyelid droops twice, then closes. The job stops and the Mac snores. | Two Macs and the internet cloud with three AI services on it: a sparkle, a robot and a brain. The left Mac shows the AI sparkle and sends a packet. The Inspector rises behind it and blocks it with his palm. A second packet drops onto the pile and he ticks his clipboard. | Show who is missing, then one meter on a large prop. The consequence appears only when the meter fails. |
| With | The camera pulls back. Miga pops up and shoves the pointer once. The ring refills and the status turns green. The inspector smiles and nods, then Miga raises her thumb. | The camera pulls back. Miga pops up holding an espresso and lifts it to the Mac. The eyelid snaps open, the job runs to the end and Miga raises her thumb, cup in hand. | The camera pulls back. Miga pops up with a coiled network cable and throws it between the Macs. Three packets run along the cable, leave through the right Mac and each reaches the service whose mark it carries. That service lights up. The answers come back the same way, the left screen turns green and the Inspector crosses his arms. | Miga arrives and fixes the prop with one gesture that belongs to this tool. The meter recovers and the scene ends content. |

The idle ring stands for the chat's own timeout and compresses a 45-second idle interval. Chat presence depends on each platform. The job bar stands for any process Caffeinate waits for. The cloud stands for the internet. The sparkle, robot and brain stand for AI services in general, so the clip shows no brand.

## Set up

You need [Node.js](https://nodejs.org) 22.12 or newer and [FFmpeg](https://ffmpeg.org) on your `PATH`. The export script also uses `perl` and `awk`, which macOS and Linux include.

There is nothing to install in this directory. Each command downloads HyperFrames 0.9.72 the first time it runs, so everyone renders with the same version. The cast, the workbench and the props are in [assets](assets), each with the record of how it was generated.

## Edit or reuse

Run commands from this directory. Use `npm run dev` for the local timeline and `npm run check` to validate it. The [root timeline](index.html) plays the episodes one after another, because HyperFrames accepts one root file. Export one episode with `npm run export -- caffeinate`, using its ID in the root timeline. The [export script](export.sh) validates the timeline, renders the episode's span and writes every format:

| File | Use |
| --- | --- |
| `docs/images/<tool>.gif` | The tool's block in the main README. 480 px wide at 12 fps. |
| `src/assets/clips/<tool>-loop.mp4` | The tool's help inside the app. 960 × 540 and silent. |
| `src/assets/clips/<tool>-poster.jpg` | The last frame, for reduced motion. |
| `docs/design/clay-workshop/renders/<tool>.mp4` | The 1080p master. Git ignores it. |

Name every file after the tool's ID so the README and the app find a new tool's clip the same way.

- Each episode is one scene: [Activity](compositions/activity.html), [Caffeinate](compositions/caffeinate.html), [AI Proxy](compositions/proxy.html). A scene holds its props, its meter, its beats and its camera moves. Duplicate a scene for a new feature, give every ID the new prefix, then mount it in the root timeline after the last episode.
- [Puppet](assets/puppet.js) holds what every episode shares: the six sculpts, the [replacement eyelids](assets/miga-lids.webp), `swap`, and the actions `wait`, `arrive`, `shove`, `thumbsUp`, `emptyChair` and `dust`. `swap` changes the sculpt at the bottom of a squash. Move Miga as a whole between swaps. An episode adds its own sculpts as one row to the right of the sheet, numbered from 6, such as the [espresso pair](assets/miga-coffee.webp) and the [cable carry](assets/miga-proxy.webp). Generate them as edits of an existing sculpt so her body stays the same.
- The Inspector has two cutouts. In Activity, a [two-cell bust](assets/inspector.webp), watching and happy, leans out from behind the monitor. Elsewhere, use his [full-body sheet](assets/inspector-poses.webp) through a second `makeClayPuppet` and its `swap`, as in [AI Proxy](compositions/proxy.html). Clip his container at the edge he rises from. He appears only when the problem does.
- [Workbench styles](assets/workshop.css) and the workbench artwork anchor the series: [with the chat](assets/workshop.webp) and [with an empty screen](assets/workshop-blank.webp) and [with two Macs](assets/workshop-proxy.webp). The [chair](assets/chair.webp) marks the absent user. Draw each episode's meter on the screen in code and keep its styles in the scene. Carry forward the same puppet, lighting and scale.

The character, desk, pointer, chair, inspector, espresso, cable, AI cloud and packet sculpts were generated from the supplied clay storyboard using the integrated image tool. GSAP 3.15.0 is bundled with its license notice for local playback.
