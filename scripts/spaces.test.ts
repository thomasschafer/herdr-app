import { test, expect, spyOn } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/spaces/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Spaces", "class Spaces").replace(/^export /gm, "");

type Space = { id: string; folder: string; machine?: string | null };
function harness(replies: unknown[] = [], storage = new Map<string, string>(), snapshot: Space[] = []) {
  const calls: string[][] = [];
  const tasks: Promise<unknown>[] = [];
  const availability = [{ name: null as string | null, layout: "ready" }];
  const moves: { id: string; index: number }[] = [];
  const runs: [string, unknown][] = [];
  const handlers: Record<string, (...args: any[]) => void> = {};
  const element: any = new Proxy({}, { get: (_, method) => (...args: any[]) => {
    if (String(method).startsWith("on_")) handlers[String(method)] = args.at(-1);
    return element;
  } });
  const Spaces = new Function("View", "hc", "localStorage", "spaces", "panes", "run", "v_flex", "h_flex", "div", "svg", "shared", "machines", `${source}; return Spaces;`)(
    class {},
    async (args: string[]) => {
      calls.push(args);
      const reply = replies.shift();
      if (reply instanceof Error) throw reply;
      return reply ?? { code: 0, stdout: "{}", stderr: "" };
    },
    { setItem(key: string, value: string) { storage.set(key, value); }, getItem(key: string) { return storage.get(key) ?? null; } },
    () => snapshot, () => [],
    (name: string, args: any) => {
      if (name === "space.move") moves.push(args);
      else runs.push([name, args]);
    },
    ...Array(4).fill(() => element), () => null, () => availability,
  );
  const view = new Spaces();
  view.reconnecting = {};
  view.folded = new Set(["cloud"]);
  view.refreshMachines = () => {};
  const cx = { timer: { every() {} }, notify() {}, spawn(fn: (cx: unknown) => Promise<unknown>) { tasks.push(fn(cx)); } };
  return { view, calls, tasks, cx, storage, moves, runs, handlers, snapshot, availability };
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

function orderedHarness(ids: string[]) {
  const h = harness([], new Map(), ids.map(id => ({ id, folder: "/" + id })));
  h.view.git = {};
  h.view.shown = new Set(ids);
  h.view.born = {};
  h.view.pressed = null;
  h.view.over = null;
  const git = (id: string, repo: string, linked: boolean) => {
    const s = h.snapshot.find(s => s.id === id)!;
    h.view.git[JSON.stringify([s.machine ?? null, s.folder])] = { repo: JSON.stringify([s.machine ?? null, `/src/${repo}/.git`]), linked, name: repo };
  };
  const flush = () => {
    for (const { id, index } of h.moves.splice(0)) {
      const from = h.snapshot.findIndex(s => s.id === id);
      h.snapshot.splice(index - 1, 0, h.snapshot.splice(from, 1)[0]);
    }
  };
  const rows = () => h.view.ordered();
  const order = () => h.snapshot.map(s => s.id);
  return { ...h, git, flush, rows, order };
}

test("the sidebar reconciles a full batch once, then the core echo is a no-op", () => {
  const h = orderedHarness(["other", "a", "main", "tail", "b"]);
  h.git("main", "repo", false);
  h.git("a", "repo", true);
  h.git("b", "repo", true);
  expect(h.rows().map((r: any) => r.w.id)).toEqual(["other", "main", "a", "b", "tail"]);
  expect(h.moves).toEqual([{ id: "main", index: 2 }, { id: "b", index: 4 }]);
  h.rows();
  expect(h.moves).toHaveLength(2);
  h.flush();
  h.rows();
  expect(h.moves).toEqual([]);
  expect(h.order()).toEqual(["other", "main", "a", "b", "tail"]);
});

const ids = (rows: any[]) => rows.map((r) => r.w?.id ?? `head:${r.head.name}`);

test("unshown spaces are preserved and a worktree without its checkout goes under its repo's name", () => {
  const h = orderedHarness(["new", "orphan", "a", "main"]);
  h.view.shown.delete("new");
  h.view.born.new = Date.now();
  h.git("orphan", "elsewhere", true);
  h.git("main", "repo", false);
  h.git("a", "repo", true);
  const rows = h.rows();
  expect(ids(rows)).toEqual(["head:elsewhere", "orphan", "main", "a"]);
  expect(rows[0].head).toEqual({ repo: JSON.stringify([null, "/src/elsewhere/.git"]), name: "elsewhere", machine: null, folder: "/src/elsewhere", kids: ["orphan"] });
  expect(rows.map((r: any) => r.tree ?? null)).toEqual([null, "└─", null, "└─"]);
  h.flush();
  expect(h.order()).toEqual(["new", "orphan", "main", "a"]);
});

test("machines and duplicate main-checkout spaces keep their own order", () => {
  const h = orderedHarness(["a", "main", "main2", "remote-a", "remote-main"]);
  for (const s of h.snapshot) if (s.id.startsWith("remote-")) s.machine = "cloud";
  h.git("main", "local-repo", false);
  h.git("main2", "local-repo", false);
  h.git("a", "local-repo", true);
  h.git("remote-main", "remote-repo", false);
  h.git("remote-a", "remote-repo", true);
  h.rows();
  h.flush();
  expect(h.order()).toEqual(["main", "a", "main2", "remote-main", "remote-a"]);
  expect(h.rows().filter((r: any) => r.tree).map((r: any) => r.w.id)).toEqual(["a", "remote-a"]);
  expect(h.moves).toEqual([]);
});

test("dragging a main checkout moves the right id and its children follow", () => {
  const h = orderedHarness(["other", "topic", "main", "last"]);
  h.git("main", "repo", false);
  h.git("topic", "repo", true);
  const first = h.rows();
  h.flush();
  // The row was built before reconciliation: neither its old index nor the
  // dragged row's old index may be used to resolve the drop.
  h.view.pressed = "main";
  h.view.over = "last";
  h.view.row(first.find((r: any) => r.w.id === "last"), {});
  h.handlers.on_mouse_up({}, h.cx);
  expect(h.moves).toEqual([{ id: "main", index: 4 }]);
  h.flush();
  h.rows();
  h.flush();
  expect(h.order()).toEqual(["other", "last", "main", "topic"]);
  h.rows();
  expect(h.moves).toEqual([]);
});

test("a machine becoming ready retries a previously rejected batch", () => {
  const h = orderedHarness(["topic", "main"]);
  h.git("main", "repo", false);
  h.git("topic", "repo", true);
  h.availability[0].layout = "unavailable";
  h.rows();
  h.moves.splice(0); // The unavailable machine rejected the moves.
  h.rows();
  expect(h.moves).toEqual([]);
  h.availability[0].layout = "ready";
  h.rows();
  expect(h.moves).toEqual([{ id: "main", index: 1 }]);
  h.flush();
  h.rows();
  expect(h.moves).toEqual([]);
});

test("a lost write retries after settling even without an intermediate ordered render", () => {
  const now = spyOn(Date, "now").mockReturnValue(10000);
  try {
    const h = orderedHarness(["topic", "main"]);
    h.git("main", "repo", false);
    h.git("topic", "repo", true);
    h.rows();
    h.flush();
    // A mirror conflict restores the original same-ID order before rendering.
    h.snapshot.reverse();
    now.mockReturnValue(10999);
    h.rows();
    expect(h.moves).toEqual([]);
    now.mockReturnValue(11000);
    let notified = false;
    h.view.due = () => new Set();
    h.view.checkNew({ notify() { notified = true; } });
    expect(notified).toBe(true);
    h.rows();
    expect(h.moves).toEqual([{ id: "main", index: 1 }]);
    h.rows();
    expect(h.moves).toHaveLength(1);
    h.flush();
    h.rows();
    now.mockReturnValue(12000);
    h.rows();
    expect(h.moves).toEqual([]);
    expect(h.view.ordering).toBeNull();
  } finally {
    now.mockRestore();
  }
});

test("settled retries wait for the affected machine to be ready", () => {
  const now = spyOn(Date, "now").mockReturnValue(10000);
  try {
    const h = orderedHarness(["topic", "main"]);
    h.git("main", "repo", false);
    h.git("topic", "repo", true);
    h.availability[0].layout = "unavailable";
    h.rows();
    h.moves.splice(0);
    now.mockReturnValue(12000);
    expect(h.view.orderingDue(Date.now())).toBe(false);
    h.rows();
    expect(h.moves).toEqual([]);
    h.availability[0].layout = "ready";
    h.rows();
    expect(h.moves).toEqual([{ id: "main", index: 1 }]);
  } finally {
    now.mockRestore();
  }
});

test("no reconciliation without a rendered sidebar", () => {
  const h = orderedHarness(["topic", "main"]);
  h.git("main", "repo", false);
  h.git("topic", "repo", true);
  expect(h.moves).toEqual([]);
  h.rows();
  h.flush();
  expect(h.order()).toEqual(["main", "topic"]);
});

test("closing a main checkout leaves its worktrees together under the repo's name", () => {
  const h = orderedHarness(["x", "main", "a", "b", "y"]);
  h.git("main", "repo", false);
  h.git("a", "repo", true);
  h.git("b", "repo", true);
  h.rows();
  expect(h.moves).toEqual([]);
  h.snapshot.splice(1, 1);
  const rows = h.rows();
  expect(ids(rows)).toEqual(["x", "head:repo", "a", "b", "y"]);
  expect(rows.map((r: any) => r.tree ?? null)).toEqual([null, null, "├─", "└─", null]);
  expect(h.moves).toEqual([]);
});

test("scattered worktrees of a repo with no checkout open gather where the first is", () => {
  const h = orderedHarness(["a", "x", "b", "c"]);
  h.git("a", "repo", true);
  h.git("c", "other", true);
  h.git("b", "repo", true);
  expect(ids(h.rows())).toEqual(["head:repo", "a", "b", "x", "head:other", "c"]);
  h.flush();
  expect(h.order()).toEqual(["a", "b", "x", "c"]);
  h.rows();
  expect(h.moves).toEqual([]);
});

test("a header opens its checkout, which takes the header's place", () => {
  const h = orderedHarness(["x", "a", "b", "y"]);
  h.git("a", "repo", true);
  h.git("b", "repo", true);
  const head = h.rows()[1].head;
  h.view.heads = { [`head:${head.repo}`]: head };
  h.view.header(head, {}, false);
  h.view.pressed = `head:${head.repo}`;
  h.handlers.on_mouse_up({}, h.cx);
  expect(h.runs).toEqual([["space.new", { folder: "/src/repo", machine: "" }]]);
  // The app adds the new space at the end.
  h.snapshot.push({ id: "main", folder: "/src/repo" });
  h.git("main", "repo", false);
  expect(ids(h.rows())).toEqual(["x", "main", "a", "b", "y"]);
  h.flush();
  expect(h.order()).toEqual(["x", "main", "a", "b", "y"]);
  h.rows();
  expect(h.view.reopening).toBe(null);
  expect(h.moves).toEqual([]);
});

test("a header of a bare repo has no checkout to open", () => {
  const h = orderedHarness(["a"]);
  h.view.git[JSON.stringify([null, "/a"])] = { repo: JSON.stringify([null, "/srv/repo.git"]), linked: true, name: "repo" };
  const head = h.rows()[0].head;
  expect(head.folder).toBe(null);
  h.view.header(head, {}, false);
  h.view.pressed = `head:${head.repo}`;
  h.handlers.on_mouse_up({}, h.cx);
  expect(h.runs).toEqual([]);
});

test("dragging a header moves its whole group, and a drop on it lands above its first worktree", () => {
  const h = orderedHarness(["a", "b", "x", "y"]);
  h.git("a", "repo", true);
  h.git("b", "repo", true);
  const head = h.rows()[0].head;
  const key = `head:${head.repo}`;
  h.view.heads = { [key]: head };
  h.view.pressed = key;
  h.view.drop("y");
  h.flush();
  expect(h.order()).toEqual(["x", "y", "a", "b"]);
  h.view.pressed = key;
  h.view.drop("x");
  h.flush();
  expect(h.order()).toEqual(["a", "b", "x", "y"]);
  h.view.pressed = key;
  h.view.drop("b");
  expect(h.moves).toEqual([]);
  h.view.pressed = "y";
  h.view.header(head, {}, false);
  h.view.over = key;
  h.handlers.on_mouse_up({}, h.cx);
  h.flush();
  expect(h.order()).toEqual(["y", "a", "b", "x"]);
  expect(ids(h.rows())).toEqual(["y", "head:repo", "a", "b", "x"]);
});

test("a header shows whether a worktree under it needs you, in both widths", () => {
  const h = orderedHarness(["a", "b"]);
  h.git("a", "repo", true);
  h.git("b", "repo", true);
  const head = h.rows()[0].head;
  expect(h.view.headStatus(head)).toBe(null);
  Object.assign(h.snapshot[1], { status: "done" });
  expect(h.view.headStatus(head)).toBe("done");
  Object.assign(h.snapshot[0], { status: "blocked" });
  expect(h.view.headStatus(head)).toBe("blocked");
  h.view.header(head, {}, true);
  h.view.header(head, {}, false);
});
