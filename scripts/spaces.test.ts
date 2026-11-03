import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/spaces/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Spaces", "class Spaces").replace(/^export /gm, "");

function harness(replies: unknown[] = [], storage = new Map<string, string>(), list: unknown[] = []) {
  const calls: string[][] = [];
  const tasks: Promise<unknown>[] = [];
  const Spaces = new Function("View", "hc", "localStorage", "spaces", "panes", `${source}; return Spaces;`)(
    class {},
    async (args: string[]) => {
      calls.push(args);
      const reply = replies.shift();
      if (reply instanceof Error) throw reply;
      return reply ?? { code: 0, stdout: "{}", stderr: "" };
    },
    { setItem(key: string, value: string) { storage.set(key, value); }, getItem(key: string) { return storage.get(key) ?? null; } },
    () => list,
    () => [],
  );
  const view = new Spaces();
  view.reconnecting = {};
  view.folded = new Set(["cloud"]);
  view.refreshMachines = () => {};
  const cx = { timer: { every() {} }, notify() {}, spawn(fn: (cx: unknown) => Promise<unknown>) { tasks.push(fn(cx)); } };
  return { view, calls, tasks, cx, storage };
}

const machine = { name: "cloud", id: "old", target: "user@host", session: null, via: false };

test("Connect removes then pairs the same SSH machine, ignoring repeat clicks", async () => {
  const { view, calls, tasks, cx } = harness();
  view.reconnect(machine, cx);
  view.reconnect(machine, cx);
  await Promise.all(tasks);
  expect(calls).toEqual([
    ["machine", "rm", "cloud", "--json"],
    ["machine", "add", "user@host", "--name", "cloud", "--json"],
  ]);
  expect(view.reconnecting).toEqual({});
  expect(view.folded.has("cloud")).toBe(false);
});

test("Connect preserves a via command and nondefault core session", async () => {
  const { view, calls, tasks, cx } = harness();
  view.reconnect({ ...machine, via: true, target: "ssh -p 2222 host", session: "work" }, cx);
  await Promise.all(tasks);
  expect(calls[1]).toEqual([
    "machine", "add", "--via", "ssh -p 2222 host", "--name", "cloud", "--session", "work", "--json",
  ]);
});

test("a failed removal never adds, and a failed add remains retryable without another removal", async () => {
  const failure = { code: 1, stdout: "", stderr: '{"error":{"message":"no connection"}}' };
  const removed = harness([failure]);
  removed.view.reconnect(machine, removed.cx);
  await Promise.all(removed.tasks);
  expect(removed.calls).toHaveLength(1);
  expect(removed.view.reconnecting.cloud.error).toBe("no connection");
  expect(removed.view.reconnecting.cloud.removed).toBe(false);

  const added = harness([{ code: 0 }, failure]);
  added.view.reconnect(machine, added.cx);
  await Promise.all(added.tasks);
  expect(added.view.reconnecting.cloud.machine).toEqual(machine);
  expect(added.view.reconnecting.cloud.busy).toBe(false);
  added.view.reconnect(machine, added.cx);
  await Promise.all(added.tasks);
  expect(added.calls.map(c => c[1])).toEqual(["rm", "add", "add"]);
  expect(added.view.reconnecting).toEqual({});
});

test("temporary reconnects remain temporary", async () => {
  const { view, calls, tasks, cx } = harness();
  view.reconnect({ ...machine, temporary: true }, cx);
  await Promise.all(tasks);
  expect(calls[1]).toContain("--temp");
});

test("failed pairing survives a sidebar restart and retries without removal", async () => {
  const failed = harness([{ code: 0 }, { code: 1, stderr: "core too old" }]);
  failed.view.reconnect(machine, failed.cx);
  await Promise.all(failed.tasks);
  const restarted = harness([], failed.storage);
  restarted.view.init({}, restarted.cx);
  expect(restarted.view.reconnecting.cloud.machine).toEqual(machine);
  expect(restarted.view.reconnecting.cloud.error).toBe("core too old");
  expect(restarted.view.reconnecting.cloud.busy).toBe(false);
  restarted.view.reconnect(machine, restarted.cx);
  await Promise.all(restarted.tasks);
  expect(restarted.calls.map(c => c[1])).toEqual(["add"]);
  const completed = harness([], failed.storage);
  completed.view.init({}, completed.cx);
  expect(completed.view.reconnecting).toEqual({});
});

test("unexpected transport failures clear the busy state", async () => {
  const { view, tasks, cx } = harness([new Error("offline")]);
  view.reconnect(machine, cx);
  await Promise.all(tasks);
  expect(view.reconnecting.cloud.busy).toBe(false);
  expect(view.reconnecting.cloud.error).toBe("offline");
});

test("rows keep the app's order and mark each worktree under its listed checkout", () => {
  const space = (id: string, worktree_of: string | null = null) => ({ id, folder: `/r/${id}`, machine: null, tabs: [], worktree_of });
  const list = [space("a"), space("a1", "a"), space("a2", "a"), space("b"), space("b1", "b"), space("c"), space("c1", "hidden")];
  const { view } = harness([], new Map(), list);
  view.shown = new Set(list.map((s) => s.id));
  view.git = {};
  view.born = {};
  const rows = view.ordered().map((r: { w: { id: string }; i: number; tree: string | null }) => [r.w.id, r.i, r.tree]);
  expect(rows).toEqual([
    ["a", 0, null], ["a1", 1, "├─"], ["a2", 2, "└─"],
    ["b", 3, null], ["b1", 4, "└─"],
    ["c", 5, null], ["c1", 6, null],
  ]);
  view.shown.delete("a2");
  expect(view.ordered().find((r: { w: { id: string } }) => r.w.id === "a1").tree).toBe("└─");
  view.shown.delete("a");
  expect(view.ordered().find((r: { w: { id: string } }) => r.w.id === "a1").tree).toBe(null);
});
