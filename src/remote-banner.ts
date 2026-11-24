import { desktop, type DeviceKind } from "./desktop";
import type { Message } from "./messages";
import { fadeText, setHidden, swap } from "./motion";
import { PROTOCOL, type Session } from "./remote";
import { formatClock } from "./time";

type UpdateState = "idle" | "working" | "restarting" | "none" | "failed";

/** The banner of a window that controls another device: who it is, whether the link works, and a version that differs. */
export function mountRemoteBanner(host: { translate(key: Message): string; language(): string }) {
  const banner = document.getElementById("remote-banner")!;
  const text = document.getElementById("remote-banner-text")!;
  const link = document.getElementById("remote-banner-link")!;
  const shell = document.getElementById("remote-banner-shell")!;
  const shellText = document.getElementById("remote-banner-shell-text")!;
  const updateLine = document.getElementById("remote-banner-update")!;
  const updateText = document.getElementById("remote-banner-update-text")!;
  const updateButton = document.getElementById("remote-update") as HTMLButtonElement;
  const kinds = banner.querySelectorAll<HTMLElement>("[data-kind]");

  let kind: DeviceKind = "desktop";
  let lost: string | undefined;
  let session: Session | undefined;
  let localVersion: string | undefined;
  let update: UpdateState = "idle";

  /** A text that changes fades into the new one. */
  const set = fadeText;

  function differs(): boolean {
    return session !== undefined && localVersion !== undefined &&
      (session.protocol !== PROTOCOL || session.version !== localVersion);
  }

  function render(): void {
    const t = host.translate;
    banner.hidden = false;
    banner.dataset.state = lost === undefined ? "connected" : "lost";
    swap([...kinds].filter(icon => icon.dataset.kind === kind), [...kinds].filter(icon => icon.dataset.kind !== kind));
    set(text, t("remoteControlling"));
    const refused = lost === "sshDenied" || lost === "sshHostKeyChanged" || lost === "sshHostKeyUnknown" || lost === "notPaired";
    set(link, t(lost === undefined ? "remoteConnected" : refused ? "remoteRefused" : "remoteLost"));
    // The shell is shown as the device told it. It can only be turned on at that device, so it is never a control here.
    // It stays out of the way while the link is lost, because the answer may be out of date.
    const state = lost === undefined ? session?.shell : undefined;
    setHidden(shell, session === undefined || lost !== undefined);
    const on = state !== undefined && state !== null;
    shell.toggleAttribute("data-on", on);
    set(shellText, on
      ? (state.until === "untilOff" ? t("permShellUntilOff") : t("permShellUntil").replace("{time}", formatClock(state.until, host.language())))
      : t("permShell"));
    if (on) shell.removeAttribute("data-tip");
    else shell.setAttribute("data-tip", t("shellOnlyThere"));
    shell.setAttribute("tabindex", on ? "-1" : "0");
    // A link that is lost hides the version, because it may be out of date.
    const note = lost === undefined && differs();
    // The line opens and closes over a short time, and keeps its last text while it closes.
    updateLine.toggleAttribute("data-open", note);
    updateLine.toggleAttribute("inert", !note);
    if (!note) return;
    const messages: Record<UpdateState, Message> = {
      idle: "remoteVersionDiffers", working: "remoteUpdating", restarting: "remoteUpdateRestarts",
      none: "remoteNoUpdate", failed: "updateFailed",
    };
    set(updateText, t(messages[update]));
    set(updateButton, t("remoteUpdate"));
    setHidden(updateButton, update === "restarting" || update === "none");
    updateButton.setAttribute("aria-disabled", String(update === "working"));
  }

  updateButton.addEventListener("click", () => {
    if (update === "working") return;
    update = "working";
    render();
    void (async () => {
      try {
        const status = await desktop.checkForUpdate();
        if (status.version === null) update = "none";
        else {
          await desktop.restartToUpdate();
          update = "restarting";
        }
      } catch {
        update = "failed";
      }
      render();
    })();
  });

  return {
    render,
    setDevice(next: DeviceKind): void {
      kind = next;
      render();
    },
    setLocalVersion(version: string | undefined): void {
      localVersion = version;
      render();
    },
    /** `error` is the code of a failed link, or nothing while it works. */
    setLink(error: string | undefined): void {
      lost = error;
      render();
    },
    setSession(next: Session): void {
      session = next;
      kind = next.device.kind;
      if (!differs()) update = "idle";
      render();
    },
  };
}
