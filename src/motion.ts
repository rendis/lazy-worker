// Small changes of state in the remote-control views: a text that changes, and content that swaps with other content.
// They fade over a short time and apply at once when the user prefers reduced motion or the node is not on screen.

const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
const outTiming = { duration: 90, easing: "ease-in" };
const inTiming = { duration: 170, easing: "cubic-bezier(.22, .7, .36, 1)" };

/** A node that is not shown cannot animate: it is out of the document, or inside something that is hidden. */
function still(node: HTMLElement): boolean {
  return reduced.matches || !node.animate || !node.isConnected || node.closest("[hidden]") !== null;
}

/** Fades `node` out and then calls `done`. A timer also ends it, because a hidden window can pause animations. */
function fadeOut(node: HTMLElement, done: () => void): void {
  const animation = node.animate([{ opacity: 1 }, { opacity: 0 }], { ...outTiming, fill: "forwards" });
  let finished = false;
  const end = (): void => {
    if (finished) return;
    finished = true;
    done();
    animation.cancel();
  };
  void animation.finished.then(end, end);
  window.setTimeout(end, outTiming.duration + 60);
}

function fadeIn(node: HTMLElement): void {
  node.animate([{ opacity: 0 }, { opacity: 1 }], inTiming);
}

const nextText = new WeakMap<HTMLElement, string>();
const swapping = new WeakSet<HTMLElement>();

/** Sets the text of a node. A text that shows fades out and the new one fades in, so the words do not jump. */
export function fadeText(node: HTMLElement, text: string): void {
  if (!swapping.has(node) && node.textContent === text) return;
  nextText.set(node, text);
  if (swapping.has(node)) return;
  if (node.textContent === "" || still(node)) {
    nextText.delete(node);
    node.textContent = text;
    return;
  }
  swapping.add(node);
  fadeOut(node, () => {
    swapping.delete(node);
    const latest = nextText.get(node);
    nextText.delete(node);
    if (latest !== undefined) node.textContent = latest;
    node.animate([{ opacity: 0 }, { opacity: 1 }], inTiming);
  });
}

const hiddenTarget = new WeakMap<HTMLElement, boolean>();
const targetOf = (node: HTMLElement): boolean => hiddenTarget.get(node) ?? node.hidden;

/** Hides the nodes in `hide` and then shows the nodes in `show`, each with a fade. A node that is already so stays as it is. */
export function swap(show: readonly HTMLElement[], hide: readonly HTMLElement[]): void {
  const leaving = hide.filter(node => !targetOf(node));
  const entering = show.filter(node => targetOf(node));
  for (const node of leaving) hiddenTarget.set(node, true);
  for (const node of entering) hiddenTarget.set(node, false);
  const enter = (): void => {
    for (const node of entering) {
      if (hiddenTarget.get(node) !== false) continue;
      const wasHidden = node.hidden;
      node.hidden = false;
      if (wasHidden && !still(node)) fadeIn(node);
    }
  };
  const leavingNow = leaving.filter(node => !still(node));
  for (const node of leaving) if (still(node)) node.hidden = true;
  if (leavingNow.length === 0) {
    enter();
    return;
  }
  let remaining = leavingNow.length;
  for (const node of leavingNow) {
    fadeOut(node, () => {
      if (hiddenTarget.get(node) === true) node.hidden = true;
      remaining -= 1;
      if (remaining === 0) enter();
    });
  }
}

/** Shows the nodes in `visible` and hides the others of `all`, each with a fade. */
export function only(all: readonly HTMLElement[], visible: readonly HTMLElement[]): void {
  swap(visible, all.filter(node => !visible.includes(node)));
}

/** Shows or hides one node with a fade. */
export function setHidden(node: HTMLElement, hide: boolean): void {
  if (hide) swap([], [node]);
  else swap([node], []);
}

/** A node that enters after the first drawing of its list: it fades in and slides a little. */
export function enterNode(node: HTMLElement): void {
  if (still(node)) return;
  node.animate([{ opacity: 0, transform: "translateY(-4px)" }, { opacity: 1, transform: "none" }], inTiming);
}
