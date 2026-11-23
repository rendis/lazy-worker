import type { Message } from "./messages";

// The first-run setup. Each control drives the control of the same setting in Settings,
// so a choice made here is saved through the same path.

export function mountOnboarding({ translate }: { translate: (message: Message) => string }) {
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
  let index = 0;

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
  }

  function show(position: number): void {
    root.dataset.direction = position < index ? "back" : "forward";
    index = position;
    steps.forEach((step, at) => { step.hidden = at !== index; });
    render();
    steps[index].querySelector<HTMLElement>("h1")?.focus({ preventScroll: true });
  }

  function close(): void {
    steps.forEach(leave);
    root.hidden = true;
    delete workspace.dataset.onboarding;
    covered().forEach(element => { (element as HTMLElement).inert = false; });
  }

  for (const mirror of mirrors) {
    if (mirror.dataset.mirrorWhen !== "leave") mirror.addEventListener("change", () => push(mirror));
  }
  back.addEventListener("click", () => show(index - 1));
  next.addEventListener("click", () => {
    leave(steps[index]);
    if (index === steps.length - 1) close();
    else show(index + 1);
  });
  skip.addEventListener("click", close);

  return {
    render,
    open(): void {
      // A deferred control starts on. The others show the saved setting.
      for (const mirror of mirrors) mirror.checked = mirror.dataset.mirrorWhen === "leave" || target(mirror).checked;
      workspace.dataset.onboarding = "";
      covered().forEach(element => { (element as HTMLElement).inert = true; });
      root.hidden = false;
      show(0);
    },
  };
}
