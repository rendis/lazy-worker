type ProgressStep = {
  label: () => string;
  run: () => Promise<void>;
};

const stepDuration = 740;
const settleDuration = 120;
const fadeDuration = 180;

function wait(milliseconds: number): Promise<void> {
  return new Promise(resolve => window.setTimeout(resolve, milliseconds));
}

export function mountOperationProgress(region: HTMLElement) {
  const fill = region.querySelector<HTMLElement>("[data-progress-fill]")!;
  const label = region.querySelector<HTMLElement>("[data-progress-label]")!;
  const counter = region.querySelector<HTMLElement>("[data-progress-counter]")!;
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  let currentLabel: (() => string) | undefined;

  function renderLabel(): void {
    const text = currentLabel?.() ?? "";
    if (label.textContent !== text) label.textContent = text;
  }

  async function drain(from: number, to: number, duration: number): Promise<void> {
    const transform = `scaleX(${to})`;
    fill.style.transform = transform;
    const animation = !reducedMotion.matches && typeof fill.animate === "function"
      ? fill.animate([{ transform: `scaleX(${from})` }, { transform }], { duration })
      : undefined;
    await wait(duration);
    animation?.finish();
  }

  async function run(steps: readonly ProgressStep[]): Promise<void> {
    if (!steps.length) return;
    region.setAttribute("data-open", "");
    try {
      for (const [index, step] of steps.entries()) {
        currentLabel = step.label;
        renderLabel();
        counter.textContent = steps.length > 1 ? `${index + 1} / ${steps.length}` : "";
        const remaining = 1 - index / steps.length;
        const end = 1 - (index + 1) / steps.length;
        // Leave a small portion of each step visible until the real action settles.
        const pending = end + .1 / steps.length;
        const minimum = drain(remaining, pending, stepDuration);
        try {
          await step.run();
        } finally {
          await minimum;
          await drain(pending, end, settleDuration);
        }
      }
    } finally {
      region.removeAttribute("data-open");
      await wait(fadeDuration);
      currentLabel = undefined;
      renderLabel();
      counter.textContent = "";
    }
  }

  return { run, renderLabel };
}
