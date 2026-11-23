import type { ActivityAccessibility } from "./desktop";
import type { Message } from "./messages";
import { tools } from "./modules";

// The first-run setup. Each control drives the control of the same setting in Settings,
// so a choice made here is saved through the same path.

export function mountOnboarding({ translate, accessibility, openAccessibility, onClose }: {
  translate: (message: Message) => string;
  accessibility: () => ActivityAccessibility | undefined;
  openAccessibility: () => void;
  onClose: () => void;
}) {
  const root = document.getElementById("onboarding")!;
  const workspace = root.closest<HTMLElement>(".workspace")!;
  const steps = [...root.querySelectorAll<HTMLElement>("[data-step]")];
  const back = root.querySelector<HTMLButtonElement>("#onboarding-back")!;
  const next = root.querySelector<HTMLButtonElement>("#onboarding-next")!;
  const nextLabel = next.querySelector("span")!;
  const skip = root.querySelector<HTMLButtonElement>("#onboarding-skip")!;
  const progress = root.querySelector<HTMLElement>("#onboarding-progress")!;
  const mirrors = [...root.querySelectorAll<HTMLInputElement>("[data-mirror]")];
  const covered = () => [...workspace.children].filter(child => child !== root && !child.classList.contains("notification-stack"));
  const permission = root.querySelector<HTMLButtonElement>("#onboarding-permission")!;
  const permissionState = root.querySelector<HTMLElement>("#onboarding-permission-state")!;
  const guide = root.querySelector<HTMLElement>("#onboarding-permission-guide")!;
  const clips = [...root.querySelectorAll<HTMLVideoElement>("[data-tool-clip]")];
  let index = 0;
  let watch: number | undefined;

  for (const clip of clips) {
    const media = tools.find(tool => tool.id === clip.dataset.toolClip)?.help.clip;
    if (!media) continue;
    clip.src = media.video;
    clip.poster = media.poster;
  }

  function missingPermission(): boolean {
    const state = accessibility();
    return state === "required" || state === "unavailable";
  }

  function setGuide(open: boolean): void {
    guide.toggleAttribute("data-open", open);
    guide.inert = !open;
  }

  /** Follows what the app learns while the setup is open, such as a granted permission. */
  function sync(): void {
    const state = accessibility();
    if (!missingPermission()) setGuide(false);
    permissionState.textContent = translate(state === "granted" ? "accessibilityGranted"
      : state === "unsupported" ? "onboardingPermissionPreview" : "onboardingPermissionMissing");
    permissionState.toggleAttribute("data-granted", state === "granted");
    permission.disabled = state === "granted" || state === "unsupported" || state === undefined;
    for (const mirror of mirrors) {
      if (mirror.dataset.mirrorWhen === "leave") continue;
      const setting = target(mirror);
      mirror.disabled = setting.disabled;
      if (mirror.checked !== setting.checked) mirror.checked = setting.checked;
    }
  }

  progress.replaceChildren(...steps.map(() => document.createElement("span")));

  function target(mirror: HTMLInputElement): HTMLInputElement {
    return document.getElementById(mirror.dataset.mirror!) as HTMLInputElement;
  }

  function push(mirror: HTMLInputElement): void {
    const setting = target(mirror);
    if (setting.checked === mirror.checked) return;
    setting.checked = mirror.checked;
    setting.dispatchEvent(new Event("change"));
  }

  /** A deferred control changes the system, so it applies when the user leaves its step. */
  function leave(step: HTMLElement): void {
    for (const mirror of mirrors) {
      if (mirror.dataset.mirrorWhen === "leave" && step.contains(mirror)) push(mirror);
    }
  }

  function render(): void {
    const last = index === steps.length - 1;
    nextLabel.textContent = translate(last ? "onboardingFinish" : "continue");
    back.style.visibility = index === 0 ? "hidden" : "";
    skip.style.visibility = last ? "hidden" : "";
    progress.setAttribute("aria-label",
      translate("onboardingProgress").replace("{n}", String(index + 1)).replace("{total}", String(steps.length)));
    [...progress.children].forEach((dot, position) => dot.toggleAttribute("data-current", position === index));
    for (const clip of clips) {
      const label = tools.find(tool => tool.id === clip.dataset.toolClip)?.help.clip?.label;
      if (label) clip.setAttribute("aria-label", translate(label));
    }
    sync();
  }

  function show(position: number): void {
    root.dataset.direction = position < index ? "back" : "forward";
    index = position;
    root.dataset.step = steps[index].dataset.step;
    steps.forEach((step, at) => { step.hidden = at !== index; });
    render();
    for (const clip of clips) {
      if (steps[index].contains(clip)) void clip.play().catch(() => {});
      else clip.pause();
    }
    steps[index].querySelector<HTMLElement>("h1")?.focus({ preventScroll: true });
  }

  function close(): void {
    steps.forEach(leave);
    root.hidden = true;
    delete workspace.dataset.onboarding;
    covered().forEach(element => { (element as HTMLElement).inert = false; });
    window.clearInterval(watch);
    clips.forEach(clip => clip.pause());
    onClose();
  }

  for (const mirror of mirrors) {
    if (mirror.dataset.mirrorWhen === "leave") continue;
    mirror.addEventListener("change", () => {
      // Without the permission the option cannot turn on. The guide says why and shows what to do.
      if (mirror.dataset.mirror === "hide-while-sharing" && mirror.checked && missingPermission()) {
        mirror.checked = false;
        setGuide(true);
        return;
      }
      push(mirror);
    });
  }
  back.addEventListener("click", () => show(index - 1));
  next.addEventListener("click", () => {
    leave(steps[index]);
    if (index === steps.length - 1) close();
    else show(index + 1);
  });
  skip.addEventListener("click", close);
  permission.addEventListener("click", () => {
    setGuide(true);
    openAccessibility();
  });

  return {
    render,
    open(): void {
      // A deferred control starts on. The others show the saved setting.
      for (const mirror of mirrors) mirror.checked = mirror.dataset.mirrorWhen === "leave" || target(mirror).checked;
      watch = window.setInterval(sync, 500);
      workspace.dataset.onboarding = "";
      covered().forEach(element => { (element as HTMLElement).inert = true; });
      root.hidden = false;
      show(0);
    },
  };
}
