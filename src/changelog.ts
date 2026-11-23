import type { Language, Message } from "./messages";
import type { ToolId } from "./modules";
import { formatDay } from "./time";

// The changes of each release, newest first. A release adds its entry here.
// Settings lists them, and the panel shows the new ones once after an update.

type Change = { en: string; es: string; /** The tool a change adds. Its entry links to that tool. */ tool?: ToolId };
export type Release = { version: string; date: string; changes: readonly Change[] };

export const releases: readonly Release[] = [
  {
    version: "0.6.1",
    date: "2026-11-23",
    changes: [
      { en: "The setup guide opens at once, with no other view before it, and its buttons stay in place.",
        es: "La guía de configuración se abre al instante, sin otra vista antes, y sus botones no se mueven." },
      { en: "The setup guide shows how to grant the Accessibility permission.",
        es: "La guía de configuración muestra cómo conceder el permiso de Accesibilidad." },
      { en: "What’s new opens in the center and no longer makes the panel scroll.",
        es: "Novedades se abre en el centro y ya no hace que el panel se desplace." },
    ],
  },
  {
    version: "0.6.0",
    date: "2026-11-23",
    changes: [
      { en: "Caffeinate can keep your Mac awake with the lid closed: only on the charger, or always.",
        es: "Caffeinate puede mantener tu Mac despierto con la tapa cerrada: solo con el cargador, o siempre." },
    ],
  },
  {
    version: "0.5.0",
    date: "2026-11-23",
    changes: [
      { en: "A setup guide welcomes a new installation.", es: "Una guía de configuración recibe a cada instalación nueva." },
      { en: "What’s new shows the changes after an update, and Settings keeps the list.",
        es: "Novedades muestra los cambios después de actualizar, y Ajustes guarda la lista." },
      { en: "The tools that were on start again after an update.",
        es: "Las herramientas que estaban encendidas vuelven a iniciarse después de actualizar." },
      { en: "Each Caffeinate option explains what it does.", es: "Cada opción de Caffeinate explica qué hace." },
    ],
  },
  {
    version: "0.4.2",
    date: "2026-11-23",
    changes: [
      { en: "Check for updates from Settings.", es: "Busca actualizaciones desde Ajustes." },
      { en: "Automatic checks run every hour.", es: "La búsqueda automática se hace cada hora." },
    ],
  },
  {
    version: "0.4.1",
    date: "2026-11-23",
    changes: [{ en: "AI Proxy has its help clip.", es: "AI Proxy tiene su clip de ayuda." }],
  },
  {
    version: "0.4.0",
    date: "2026-11-23",
    changes: [
      { en: "AI Proxy opens your AI tools through another computer on your network.",
        es: "AI Proxy abre tus herramientas de IA a través de otro equipo de tu red.", tool: "proxy" },
      { en: "Every tool view has a new, compact design.", es: "Cada vista de herramienta tiene un diseño nuevo y compacto." },
      { en: "A long session shows its time in days.", es: "Una sesión larga muestra su tiempo en días." },
    ],
  },
  {
    version: "0.3.1",
    date: "2026-11-20",
    changes: [{ en: "Settings shows the app version.", es: "Ajustes muestra la versión de la app." }],
  },
];

function compare(a: string, b: string): number {
  const left = a.split(".").map(Number);
  const right = b.split(".").map(Number);
  for (let part = 0; part < 3; part++) {
    const difference = (left[part] || 0) - (right[part] || 0);
    if (difference) return difference;
  }
  return 0;
}

/** The releases a user has not seen. With no seen version, only the running one counts. */
export function releasesSince(seen: string | null, current: string): Release[] {
  return releases.filter(({ version }) =>
    compare(version, current) <= 0 && (seen === null ? version === current : compare(version, seen) > 0));
}

export function mountChangelog({ translate, language, onClose }: {
  translate: (message: Message) => string;
  language: () => Language;
  onClose: () => void;
}) {
  const overlay = document.getElementById("changelog-window")!;
  const list = document.getElementById("changelog-list")!;
  const close = document.querySelector<HTMLButtonElement>("#changelog-close")!;
  let shown: readonly Release[] = releases;
  let trigger: HTMLElement | undefined;

  function render(): void {
    const locale = language();
    list.replaceChildren(...shown.map(release => {
      const section = document.createElement("section");
      const heading = document.createElement("h3");
      const day = document.createElement("time");
      day.dateTime = release.date;
      day.textContent = formatDay(release.date, locale);
      heading.append(release.version, day);
      const changes = document.createElement("ul");
      for (const change of release.changes) {
        const item = document.createElement("li");
        item.append(change[locale]);
        if (change.tool) {
          const open = document.createElement("button");
          open.type = "button";
          open.className = "text-link";
          open.dataset.openTool = change.tool;
          open.textContent = translate("changelogOpenTool");
          item.append(" ", open);
        }
        changes.append(item);
      }
      section.append(heading, changes);
      return section;
    }));
  }

  function setOpen(open: boolean): void {
    if (overlay.hasAttribute("data-open") === open) return;
    overlay.toggleAttribute("data-open", open);
    overlay.inert = !open;
    if (open) close.focus();
    else {
      if (trigger?.isConnected) trigger.focus();
      onClose();
    }
  }

  close.addEventListener("click", () => setOpen(false));
  overlay.addEventListener("click", event => { if (event.target === overlay) setOpen(false); });
  overlay.addEventListener("keydown", event => { if (event.key === "Escape") setOpen(false); });
  list.addEventListener("click", event => {
    const tool = (event.target as Element).closest<HTMLElement>("[data-open-tool]")?.dataset.openTool;
    if (!tool) return;
    trigger = undefined;
    setOpen(false);
    document.querySelector<HTMLButtonElement>(`[data-view="${tool}"]`)?.click();
  });

  return {
    render,
    /** Shows the given releases, or every release when none is given. */
    open(only?: readonly Release[], from?: HTMLElement): void {
      shown = only ?? releases;
      trigger = from;
      render();
      list.scrollTop = 0;
      setOpen(true);
    },
  };
}
