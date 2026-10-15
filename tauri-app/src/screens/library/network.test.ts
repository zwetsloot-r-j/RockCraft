import { afterEach, describe, expect, it, vi } from "vitest";
import {
  fmtMB,
  loadServers,
  loadShareEnabled,
  localBadge,
  parseServer,
  saveServers,
  saveShareEnabled,
  transferLine,
  transferPercent,
} from "./network";

describe("parseServer", () => {
  it("accepts host:port and fills in the default port", () => {
    expect(parseServer("192.168.1.5:27800")).toBe("192.168.1.5:27800");
    expect(parseServer(" piano-pc.local ")).toBe("piano-pc.local:27800");
    expect(parseServer("[::1]:27801")).toBe("[::1]:27801");
    expect(parseServer("[fd00::7]")).toBe("[fd00::7]:27800");
  });

  it("rejects bad ports and shapes", () => {
    for (const bad of ["", "host:0", "host:65536", "host:12ab", "a b:1", "::1:27800", "[]:1", "x:1:2"]) {
      expect(parseServer(bad), bad).toBeNull();
    }
  });
});

describe("transfers", () => {
  const base = { id: 1, server: "s", name: "frog-song", bytes_done: 12 * 1024 * 1024, bytes_total: 48 * 1024 * 1024 };

  it("formats sizes and percent", () => {
    expect(fmtMB(3.44 * 1024 * 1024)).toBe("3.4 MB");
    expect(fmtMB(146 * 1024 * 1024)).toBe("146 MB");
    expect(transferPercent(base)).toBe(25);
    expect(transferPercent({ bytes_done: 5, bytes_total: 0 })).toBe(0);
  });

  it("summarises each state", () => {
    expect(transferLine({ ...base, direction: "down", state: "running" })).toBe(
      "↓ frog-song  12 MB / 48 MB  25%",
    );
    expect(transferLine({ ...base, direction: "up", state: "done", dir: null })).toBe("↑ frog-song  done");
    expect(transferLine({ ...base, direction: "down", state: "exists", slug: "frog-song" })).toBe(
      "↓ frog-song  name taken (frog-song)",
    );
  });
});

describe("localBadge", () => {
  it("maps the local state", () => {
    expect(localBadge("same")).toBe("in library");
    expect(localBadge("different")).toBe("different version");
    expect(localBadge("absent")).toBeNull();
  });
});

describe("settings", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("round-trips through localStorage", () => {
    const store = new Map<string, string>();
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
    });
    expect(loadShareEnabled()).toBe(false);
    saveShareEnabled(true);
    expect(loadShareEnabled()).toBe(true);
    saveServers(["192.168.1.5:27800", "bogus address"]);
    expect(loadServers()).toEqual(["192.168.1.5:27800"]);
  });

  it("survives a throwing localStorage", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
    });
    expect(loadShareEnabled()).toBe(false);
    expect(loadServers()).toEqual([]);
    expect(() => saveShareEnabled(true)).not.toThrow();
    expect(() => saveServers([])).not.toThrow();
  });
});
