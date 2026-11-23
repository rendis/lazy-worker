# Clay workshop

- Read the series layout and mascot reference in [README.md](README.md) before editing an episode.
- Keep each episode's props and feature-specific animation in its scene. Share Miga's poses and the workbench across episodes.
- Keep episodes silent and free of text. Put captions in the README and in the app strings.
- Change Miga's sculpt only through `swap`, and move her as a whole between swaps. Cue each reaction after the visible result.
- Run the pinned HyperFrames check and inspect every frame around a sculpt change, the hand contact and the final status change in the encoded MP4.
- Keep puppet sheets at their intrinsic dimensions. Select one 512 px cell with the SVG viewBox.
- Give each tool its own gesture that fits its prop. Miga shoves the pointer in Activity only.
- Keep the cast: Miga is the hero and the tool, the Inspector is the antagonist who watches and blocks. He appears only with the problem and always loses.
- Show the problem without the tool before the fix. Give the problem a visible meter so the fix has a visible effect.
- Set opening states with `gsap.set` outside the timeline so frame 0 renders them.
