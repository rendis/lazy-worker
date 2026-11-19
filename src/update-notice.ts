import { desktop } from "./desktop";
import type { Message } from "./messages";

export function mountUpdateNotice({ translate, setNoticeOpen, beforeRestart, moveFocus }: {
  translate: (message: Message) => string;
  setNoticeOpen: (notice: HTMLElement, open: boolean) => void;
  beforeRestart: () => Promise<void>;
  moveFocus: () => void;
}) {
  const notice = document.getElementById("update-notice")!;
  const settings = document.getElementById("settings-update")!;
  const description = document.getElementById("update-description")!;
  const settingsDescription = document.getElementById("settings-update-description")!;
  const announcement = document.getElementById("update-announcement")!;
  const restart = document.querySelector<HTMLButtonElement>("#update-restart")!;
  const settingsRestart = document.querySelector<HTMLButtonElement>("#settings-update-restart")!;
  const later = document.querySelector<HTMLButtonElement>("#update-later")!;

  let version: string | null = null;
  let deferred: string | null = null;
  let installing = false;
  let failed = false;
  let refreshRevision = 0;

  function render(): void {
    const open = version !== null && version !== deferred;
    const status = installing ? translate("updateInstalling") : failed ? translate("updateFailed")
      : version ? translate("updateAvailable").replace("{version}", version) : "";
    if ((!open && notice.contains(document.activeElement)) || (!version && settings.contains(document.activeElement))) {
      moveFocus();
    }
    setNoticeOpen(notice, open);
    settings.hidden = version === null;
    for (const node of [description, settingsDescription]) node.textContent = status;
    for (const button of [restart, settingsRestart, later]) {
      button.setAttribute("aria-disabled", String(installing));
    }
    const spoken = open || installing || failed ? status : "";
    if (announcement.textContent !== spoken) announcement.textContent = spoken;
  }

  async function refresh(): Promise<void> {
    if (!desktop.macApp) return;
    const revision = ++refreshRevision;
    const available = await desktop.getAvailableUpdate().catch(() => undefined);
    if (revision !== refreshRevision || available === undefined || available === version) return;
    version = available;
    failed = false;
    render();
  }

  async function apply(): Promise<void> {
    if (installing || version === null) return;
    const chosen = version;
    installing = true;
    failed = false;
    render();
    try {
      await beforeRestart();
      await desktop.restartToUpdate(chosen);
    } catch (error) {
      installing = false;
      if (error === "updateChanged") await refresh();
      else failed = true;
      render();
    }
  }

  restart.addEventListener("click", apply);
  settingsRestart.addEventListener("click", apply);
  later.addEventListener("click", () => {
    if (installing) return;
    deferred = version;
    failed = false;
    render();
  });

  return {
    render,
    refresh,
    async start() {
      await desktop.onAvailableUpdate(() => { void refresh(); }).catch(() => {});
      await refresh();
    },
  };
}
