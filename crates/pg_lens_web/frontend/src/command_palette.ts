// Command Palette for pg_lens web (Cmd+K / Ctrl+K)
// Quick switching between lenses, databases, and admin/stream actions.

export interface PaletteAction {
  id: string;
  group: "Lenses" | "Servers" | "Databases" | "Actions";
  label: string;
  detail?: string;
  shortcut?: string;
  iconId?: string;
  run: () => void;
}

export function filterPaletteActions(query: string, actions: PaletteAction[]): PaletteAction[] {
  const q = query.trim().toLowerCase();
  if (!q) {
    return [...actions];
  }
  return actions.filter((a) => {
    return (
      a.label.toLowerCase().includes(q) ||
      (a.detail?.toLowerCase().includes(q) ?? false) ||
      a.group.toLowerCase().includes(q)
    );
  });
}

export class CommandPalette {
  private backdropEl: HTMLElement;
  private inputEl: HTMLInputElement;
  private listEl: HTMLElement;
  private actions: PaletteAction[] = [];
  private selectedIndex = 0;
  private filteredActions: PaletteAction[] = [];

  constructor(
    backdropId = "palette-backdrop",
    inputId = "palette-input",
    listId = "palette-list",
  ) {
    this.backdropEl = document.getElementById(backdropId) as HTMLElement;
    this.inputEl = document.getElementById(inputId) as HTMLInputElement;
    this.listEl = document.getElementById(listId) as HTMLElement;

    this.backdropEl?.addEventListener("click", (e) => {
      if (e.target === this.backdropEl) {
        this.close();
      }
    });

    this.inputEl?.addEventListener("input", () => {
      this.filter(this.inputEl.value);
    });

    this.inputEl?.addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        this.moveSelection(1);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        this.moveSelection(-1);
      } else if (e.key === "Enter") {
        e.preventDefault();
        this.executeSelected();
      } else if (e.key === "Escape") {
        e.preventDefault();
        this.close();
      }
    });
  }

  setActions(actions: PaletteAction[]): void {
    this.actions = actions;
    if (this.isOpen()) {
      this.filter(this.inputEl.value);
    }
  }

  isOpen(): boolean {
    return this.backdropEl.classList.contains("active");
  }

  open(): void {
    this.backdropEl.classList.add("active");
    this.inputEl.value = "";
    this.selectedIndex = 0;
    this.filter("");
    setTimeout(() => this.inputEl.focus(), 50);
  }

  close(): void {
    this.backdropEl.classList.remove("active");
    this.inputEl.blur();
  }

  private filter(query: string): void {
    this.filteredActions = filterPaletteActions(query, this.actions);
    this.selectedIndex = 0;
    this.render();
  }

  private moveSelection(delta: number): void {
    if (this.filteredActions.length === 0) return;
    this.selectedIndex = (this.selectedIndex + delta + this.filteredActions.length) % this.filteredActions.length;
    this.render();
  }

  private executeSelected(): void {
    const action = this.filteredActions[this.selectedIndex];
    if (action) {
      this.close();
      action.run();
    }
  }

  private render(): void {
    this.listEl.replaceChildren();

    if (this.filteredActions.length === 0) {
      const empty = document.createElement("div");
      empty.className = "palette-empty";
      empty.textContent = "No matching commands or databases found.";
      this.listEl.appendChild(empty);
      return;
    }

    let currentGroup = "";
    for (let i = 0; i < this.filteredActions.length; i++) {
      const action = this.filteredActions[i];
      if (!action) continue;

      if (action.group !== currentGroup) {
        currentGroup = action.group;
        const groupEl = document.createElement("div");
        groupEl.className = "palette-group-title";
        groupEl.textContent = currentGroup;
        this.listEl.appendChild(groupEl);
      }

      const itemEl = document.createElement("div");
      itemEl.className = `palette-item ${i === this.selectedIndex ? "active" : ""}`;

      if (action.iconId) {
        const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        svg.setAttribute("class", "icon palette-item-icon");
        svg.setAttribute("aria-hidden", "true");
        const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
        use.setAttributeNS("http://www.w3.org/1999/xlink", "href", `#${action.iconId}`);
        svg.appendChild(use);
        itemEl.appendChild(svg);
      }

      const labelEl = document.createElement("span");
      labelEl.className = "palette-item-label";
      labelEl.textContent = action.label;
      if (action.detail) {
        const detailSpan = document.createElement("span");
        detailSpan.style.color = "var(--fg-dim)";
        detailSpan.style.fontSize = "0.78rem";
        detailSpan.style.marginLeft = "6px";
        detailSpan.textContent = `(${action.detail})`;
        labelEl.appendChild(detailSpan);
      }
      itemEl.appendChild(labelEl);

      if (action.shortcut) {
        const kbd = document.createElement("kbd");
        kbd.className = "palette-item-shortcut";
        kbd.textContent = action.shortcut;
        itemEl.appendChild(kbd);
      }

      itemEl.addEventListener("mouseenter", () => {
        this.selectedIndex = i;
        this.render();
      });

      itemEl.addEventListener("click", () => {
        this.close();
        action.run();
      });

      this.listEl.appendChild(itemEl);
    }

    // Scroll active item into view
    const activeEl = this.listEl.querySelector(".palette-item.active");
    if (activeEl instanceof HTMLElement) {
      activeEl.scrollIntoView({ block: "nearest" });
    }
  }
}
