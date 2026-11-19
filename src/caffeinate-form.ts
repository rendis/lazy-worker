import { desktop, type CaffeinateFlag, type CaffeinateSettings } from "./desktop";

const flags: CaffeinateFlag[] = ["i", "d", "m", "s", "u"];

export function mountCaffeinateForm() {
  const form = document.querySelector<HTMLFormElement>("#caffeinate-form")!;
  const main = form.closest("main")!;
  const modes = form.querySelector<HTMLElement>(".caffeinate-modes")!;
  const modeInputs = [...form.querySelectorAll<HTMLInputElement>("[name=caffeinate-mode]")];
  const fields = form.querySelector<HTMLElement>(".caffeinate-mode-fields")!;
  const groups = [...fields.querySelectorAll<HTMLElement>("[data-caffeinate-fields]")];
  const duration = form.querySelector<HTMLInputElement>("#caffeinate-duration")!;
  const unit = form.querySelector<HTMLSelectElement>("#caffeinate-unit")!;
  const pid = form.querySelector<HTMLInputElement>("#caffeinate-pid")!;
  const preview = form.querySelector<HTMLElement>("#caffeinate-preview")!;
  const options = [...form.querySelectorAll<HTMLInputElement>("[data-caffeinate-option]")];
  const controls = form.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>("input, select, button");
  const picker = form.querySelector<HTMLElement>(".caffeinate-process-picker")!;
  const search = picker.querySelector<HTMLInputElement>("#caffeinate-process")!;
  const popup = picker.querySelector<HTMLElement>(".caffeinate-process-popup")!;
  const list = picker.querySelector<HTMLUListElement>("#caffeinate-processes")!;
  const refresh = picker.querySelector<HTMLButtonElement>("#caffeinate-refresh-processes")!;
  const processFeedback = picker.querySelectorAll<HTMLElement>("[data-process-status]");
  let disabled = false;
  let activeFields: HTMLElement | undefined;
  let processes: Awaited<ReturnType<typeof desktop.listProcesses>> = [];
  let processesRequested = false;
  let loading = false;
  let loadFailed = false;
  let pickerOpen = false;
  let query = "";
  let activeOption: HTMLElement | null = null;

  function read(): CaffeinateSettings {
    return {
      mode: (modeInputs.find(input => input.checked)?.value ?? "untilStopped") as CaffeinateSettings["mode"],
      flags: Object.fromEntries(options.map(option => [option.dataset.caffeinateOption, option.checked])) as CaffeinateSettings["flags"],
      seconds: duration.valueAsNumber * (unit.value === "minutes" ? 60 : 1),
      pid: pid.value === "" ? null : pid.valueAsNumber,
    };
  }

  function fitFields(): void {
    const height = activeFields?.offsetHeight ?? 0;
    if (height || !activeFields) fields.style.height = `${height}px`;
  }

  function positionPicker(): void {
    if (!pickerOpen || !activeFields) return;
    const viewport = window.visualViewport;
    const viewportTop = viewport?.offsetTop ?? 0;
    const viewportBottom = viewportTop + (viewport?.height ?? window.innerHeight);
    const mainTop = Math.max(main.getBoundingClientRect().top, viewportTop);
    const anchor = activeFields.getBoundingClientRect();
    const below = viewportBottom - anchor.bottom;
    const above = anchor.top - mainTop;
    const openAbove = below < 135 && above > below;
    popup.toggleAttribute("data-above", openAbove);
    popup.style.maxHeight = `${Math.max(44, (openAbove ? above : below) - 9)}px`;
  }

  const resize = new ResizeObserver(() => {
    fitFields();
    positionPicker();
  });
  groups.forEach(group => resize.observe(group));

  function showSavedProcess(): void {
    search.value = processes.find(process => process.pid === pid.valueAsNumber)?.name ?? "";
  }

  function highlightProcess(option: HTMLElement | null): void {
    activeOption?.setAttribute("aria-selected", "false");
    activeOption = option;
    if (!option) {
      search.removeAttribute("aria-activedescendant");
      return;
    }
    option.setAttribute("aria-selected", "true");
    search.setAttribute("aria-activedescendant", option.id);
    const top = option.offsetTop;
    const bottom = top + option.offsetHeight;
    if (top < list.scrollTop) list.scrollTop = top;
    if (bottom > list.scrollTop + list.clientHeight) list.scrollTop = bottom - list.clientHeight;
  }

  function renderProcessList(): void {
    if (!pickerOpen) {
      showSavedProcess();
      return;
    }
    const filter = query.trim().toLowerCase();
    const matches = processes.filter(process => process.name.toLowerCase().includes(filter) || String(process.pid).includes(filter));
    const highlightedPid = activeOption?.dataset.pid;
    const entries = matches.map(process => {
      const option = document.createElement("li");
      option.id = `caffeinate-process-${process.pid}`;
      option.dataset.pid = String(process.pid);
      option.setAttribute("role", "option");
      option.setAttribute("aria-selected", "false");
      const name = document.createElement("span");
      name.textContent = process.name;
      name.title = process.name;
      const identifier = document.createElement("span");
      identifier.textContent = `PID ${process.pid}`;
      option.append(name, identifier);
      return option;
    });
    list.replaceChildren(...entries);
    highlightProcess(entries.find(option => option.dataset.pid === highlightedPid) ?? null);
    const status = loading ? "loading" : loadFailed ? "error" : matches.length === 0 ? "empty" : "";
    for (const message of processFeedback) message.hidden = message.dataset.processStatus !== status;
  }

  async function loadProcesses(): Promise<void> {
    if (loading) return;
    processesRequested = true;
    loading = true;
    list.setAttribute("aria-busy", "true");
    refresh.setAttribute("aria-disabled", "true");
    renderProcessList();
    try {
      processes = await desktop.listProcesses();
      loadFailed = false;
    } catch {
      loadFailed = true;
    } finally {
      loading = false;
      list.removeAttribute("aria-busy");
      refresh.removeAttribute("aria-disabled");
      renderProcessList();
    }
  }

  function setPickerOpen(open: boolean): void {
    pickerOpen = open;
    refresh.disabled = !open;
    popup.toggleAttribute("data-open", open);
    popup.setAttribute("aria-hidden", String(!open));
    search.setAttribute("aria-expanded", String(open));
  }

  function openPicker(filter: string): void {
    if (pickerOpen) return;
    query = filter;
    setPickerOpen(true);
    positionPicker();
    if (loading) renderProcessList();
    else void loadProcesses();
  }

  function closePicker(): void {
    if (!pickerOpen) return;
    setPickerOpen(false);
    highlightProcess(null);
    showSavedProcess();
  }

  function chooseProcess(option: HTMLElement | null): void {
    const selectedPid = option?.dataset.pid;
    if (!selectedPid) return;
    const changed = pid.value !== selectedPid;
    pid.value = selectedPid;
    closePicker();
    if (changed) pid.dispatchEvent(new Event("input", { bubbles: true }));
  }

  function render(): void {
    const settings = read();
    const nextFields = groups.find(group => group.dataset.caffeinateFields === settings.mode);
    if (nextFields !== activeFields) {
      activeFields = nextFields;
      modes.dataset.selection = settings.mode;
      for (const group of groups) {
        const active = group === activeFields;
        group.toggleAttribute("data-active", active);
        group.setAttribute("aria-hidden", String(!active));
      }
      fitFields();
    }
    if (disabled || settings.mode !== "process") closePicker();
    else if (!processesRequested) void loadProcesses();
    if (!pickerOpen) showSavedProcess();
    for (const control of controls) {
      control.disabled = disabled || (control === refresh && !pickerOpen) || control.closest("[data-caffeinate-fields]:not([data-active])") !== null;
    }

    const selected = flags.filter(flag => settings.flags[flag]).map(flag => "-" + flag);
    const command = ["/usr/bin/caffeinate", ...selected];
    if (settings.mode === "timer") command.push("-t", Number.isFinite(settings.seconds) ? String(settings.seconds) : "<seconds>");
    if (settings.mode === "process") command.push("-w", settings.pid ? String(settings.pid) : "<pid>");
    preview.textContent = command.join(" ");
  }

  function set(settings: CaffeinateSettings): void {
    for (const input of modeInputs) input.checked = input.value === settings.mode;
    const useMinutes = settings.seconds % 60 === 0;
    duration.value = String(useMinutes ? settings.seconds / 60 : settings.seconds);
    unit.value = useMinutes ? "minutes" : "seconds";
    pid.value = settings.pid === null ? "" : String(settings.pid);
    for (const option of options) option.checked = settings.flags[option.dataset.caffeinateOption as CaffeinateFlag];
    render();
  }

  function settingsForSave(previous: CaffeinateSettings): CaffeinateSettings {
    const settings = read();
    if (!Object.values(settings.flags).some(Boolean)) throw "caffeinateNoFlags";
    const validDuration = Number.isInteger(settings.seconds) && settings.seconds > 0 && settings.seconds <= 4_294_967_295;
    const validPid = settings.pid !== null && Number.isInteger(settings.pid) && settings.pid > 0 && settings.pid <= 2_147_483_647;
    if (settings.mode === "timer" && !validDuration) throw "caffeinateInvalidDuration";
    if (settings.mode === "process" && !validPid) throw "caffeinateInvalidPid";
    if (!validDuration) settings.seconds = previous.seconds;
    if (!validPid) settings.pid = null;
    return settings;
  }

  window.addEventListener("resize", positionPicker);
  window.addEventListener("scroll", positionPicker, { passive: true });
  main.addEventListener("scroll", positionPicker, { passive: true });
  window.visualViewport?.addEventListener("resize", positionPicker);
  window.visualViewport?.addEventListener("scroll", positionPicker);
  search.addEventListener("focus", () => search.select());
  search.addEventListener("click", () => {
    if (pickerOpen) return;
    openPicker("");
    search.select();
  });
  search.addEventListener("input", () => {
    if (!pickerOpen) openPicker(search.value);
    else {
      query = search.value;
      highlightProcess(null);
      renderProcessList();
      list.scrollTop = 0;
    }
  });
  search.addEventListener("keydown", event => {
    if (event.isComposing) return;
    if (event.key === "Enter") {
      event.preventDefault();
      if (!pickerOpen) openPicker("");
      else chooseProcess(activeOption ?? (list.children.length === 1 ? list.querySelector("li") : null));
    }
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    if (!pickerOpen) openPicker("");
    const entries = [...list.querySelectorAll("li")];
    const current = entries.findIndex(entry => entry === activeOption);
    const target = event.key === "ArrowDown" ? Math.min(current + 1, entries.length - 1)
      : current === -1 ? entries.length - 1 : Math.max(current - 1, 0);
    highlightProcess(entries[target] ?? null);
  });
  picker.addEventListener("keydown", event => {
    if (event.key !== "Escape" || event.isComposing || !pickerOpen) return;
    event.preventDefault();
    search.focus();
    closePicker();
    search.select();
  });
  picker.addEventListener("focusout", event => {
    if (!(event.relatedTarget instanceof Node) || !picker.contains(event.relatedTarget)) closePicker();
  });
  document.addEventListener("pointerdown", event => {
    if (event.target instanceof Node && !picker.contains(event.target)) closePicker();
  });
  list.addEventListener("mousedown", event => {
    if (event.target instanceof HTMLElement && event.target.closest("li")) event.preventDefault();
  });
  list.addEventListener("click", event => {
    if (event.target instanceof HTMLElement) chooseProcess(event.target.closest("li"));
  });
  refresh.addEventListener("mousedown", event => event.preventDefault());
  refresh.addEventListener("click", () => { void loadProcesses(); });

  form.addEventListener("input", event => {
    if (event.target !== search) render();
  });
  form.addEventListener("submit", event => event.preventDefault());
  return {
    form,
    set,
    settingsForSave,
    onChange(listener: () => void): void {
      form.addEventListener("input", event => {
        if (event.target !== search) listener();
      });
    },
    setDisabled(value: boolean): void {
      if (disabled === value) return;
      disabled = value;
      render();
    },
  };
}
