import { desktop, type UpdateStatus } from "./desktop";
import type { Message } from "./messages";
import { age, type AgeUnit } from "./time";

// A panel that gets focus checks again when the last answer is older than this.
const STALE_SECONDS = 15 * 60;
const ageMessages: Record<Exclude<AgeUnit, "seconds">, Message> = {
  minutes: "minutesAgo", hours: "hoursAgo", days: "daysAgo",
};

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
  const checkButton = document.querySelector<HTMLButtonElement>("#update-check")!;
  const checkStatus = document.getElementById("update-check-status")!;
  checkButton.hidden = !desktop.showUpdateCheck;

  let version: string | null = null;
  let deferred: string | null = null;
  let installing = false;
  let failed = false;
  let checked: number | null = null;
  let checking = false;
  let checkFailed = false;
  let refreshRevision = 0;

  function sinceCheck(): number {
    return checked === null ? 0 : Date.now() / 1000 - checked;
  }

  function renderCheck(): void {
    let status = "";
    if (checking) status = translate("updateChecking");
    else if (checkFailed) status = translate("updateCheckFailed");
    else if (checked !== null) {
      const { value, unit } = age(sinceCheck());
      const elapsed = unit === "seconds" ? translate("justNow") : translate(ageMessages[unit]).replace("{n}", String(value));
      const when = translate("updateChecked").replace("{age}", elapsed);
      status = version ? when : `${translate("upToDate")} · ${when}`;
    }
    if (checkStatus.textContent !== status) checkStatus.textContent = status;
    checkButton.setAttribute("aria-disabled", String(checking || installing));
  }

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
    renderCheck();
  }

  function take(status: UpdateStatus): void {
    if (status.checked !== checked) checkFailed = false;
    checked = status.checked;
    if (status.version === version) return;
    version = status.version;
    failed = false;
  }

  async function check(): Promise<void> {
    if (checking || installing) return;
    checking = true;
    checkFailed = false;
    render();
    ++refreshRevision;
    try {
      take(await desktop.checkForUpdate());
    } catch {
      checkFailed = true;
    }
    checking = false;
    render();
  }

  async function refresh(): Promise<void> {
    if (!desktop.macApp) return;
    const revision = ++refreshRevision;
    const status = await desktop.getUpdateStatus().catch(() => undefined);
    if (revision !== refreshRevision || status === undefined) return;
    take(status);
    render();
    if (sinceCheck() > STALE_SECONDS) void check();
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
  checkButton.addEventListener("click", () => { void check(); });
  setInterval(renderCheck, 30_000);
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
