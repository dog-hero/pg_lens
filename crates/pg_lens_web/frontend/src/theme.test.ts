import { test } from "node:test";
import assert from "node:assert/strict";

import { loadStoredTheme, nextTheme, resolveInitialTheme, saveTheme } from "./theme.ts";

test("resolveInitialTheme defaults to dark", () => {
  assert.equal(resolveInitialTheme(null), "dark");
  assert.equal(resolveInitialTheme(""), "dark");
  assert.equal(resolveInitialTheme("bogus"), "dark");
});

test("resolveInitialTheme honors a persisted light choice", () => {
  assert.equal(resolveInitialTheme("light"), "light");
});

test("nextTheme flips between exactly two states", () => {
  assert.equal(nextTheme("dark"), "light");
  assert.equal(nextTheme("light"), "dark");
});

class FakeStorage implements Storage {
  private map = new Map<string, string>();
  get length(): number {
    return this.map.size;
  }
  clear(): void {
    this.map.clear();
  }
  getItem(key: string): string | null {
    return this.map.get(key) ?? null;
  }
  key(index: number): string | null {
    return [...this.map.keys()][index] ?? null;
  }
  removeItem(key: string): void {
    this.map.delete(key);
  }
  setItem(key: string, value: string): void {
    this.map.set(key, value);
  }
}

class ThrowingStorage implements Storage {
  length = 0;
  clear(): void {
    throw new Error("disabled");
  }
  getItem(): string | null {
    throw new Error("disabled");
  }
  key(): string | null {
    throw new Error("disabled");
  }
  removeItem(): void {
    throw new Error("disabled");
  }
  setItem(): void {
    throw new Error("disabled");
  }
}

test("save/load round-trip through a real Storage-like object", () => {
  const storage = new FakeStorage();
  saveTheme(storage, "light");
  assert.equal(loadStoredTheme(storage), "light");
});

test("load/save degrade to a no-op/null when storage throws (private browsing)", () => {
  const storage = new ThrowingStorage();
  assert.equal(loadStoredTheme(storage), null);
  saveTheme(storage, "light"); // must not throw
});
