import { afterEach, expect, test } from "bun:test";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, realpathSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Runs the shipped dialog's create() against real git repositories in a temporary home.
const source = (await readFile(new URL("../defaults/mods/worktree/main.js", import.meta.url), "utf8"))
  .replace(/^import [^;]*;\n/gm, "").replace("export default class Worktree", "class Worktree");

// Local submodule URLs are file:// clones, which git refuses unless allowed.
const env = { ...process.env, GIT_CONFIG_COUNT: "1", GIT_CONFIG_KEY_0: "protocol.file.allow", GIT_CONFIG_VALUE_0: "always" };
const git = (cwd: string, ...args: string[]) => execFileSync("git", args, { cwd, env, encoding: "utf8" }).trim();

// Each test runs git dozens of times.
const slow = 60_000;

const homes: string[] = [];
afterEach(() => { for (const h of homes.splice(0)) rmSync(h, { recursive: true, force: true }); });

function harness(home: string, failing: (args: string[]) => boolean = () => false) {
  const commands: unknown[][] = [];
  const exec = async (cmd: string, args: string[], { cwd }: { cwd: string }) => {
    if (failing(args)) return { code: 1, stdout: "", stderr: "fatal: offline" };
    const r = spawnSync(cmd, args, { cwd, env, encoding: "utf8" });
    return { code: r.status, stdout: r.stdout, stderr: r.stderr };
  };
  const values: Record<string, unknown> = {
    View: class {}, div() {}, v_flex() {}, h_flex() {}, InputState: {}, Input: {}, Scrollbar: {},
    run: (...args: unknown[]) => commands.push(args), props: () => ({}), closeOverlay() {}, theme: () => ({}),
    spaces: () => [], panes: () => [], exec, home: () => home,
  };
  const { Worktree, repoOf } = new Function(...Object.keys(values), `${source}; return { Worktree, repoOf };`)(...Object.values(values));
  return { Worktree, repoOf, commands, home };
}

function repo(dir: string) {
  git(tmpdir(), "init", "-q", "-b", "main", dir);
  git(dir, "config", "user.email", "test@example.com");
  git(dir, "config", "user.name", "Test");
  git(dir, "commit", "-q", "--allow-empty", "-m", "start");
  return dir;
}

// A project with a submodule `vendor/lib` that has a submodule `deps/inner`, and
// another, `docs`, that was never checked out.
function project() {
  const home = realpathSync(mkdtempSync(join(tmpdir(), "herdr-worktree-")));
  homes.push(home);
  const inner = repo(join(home, "inner"));
  const lib = repo(join(home, "lib"));
  git(lib, "submodule", "add", "-q", inner, "deps/inner");
  git(lib, "commit", "-q", "-m", "inner");
  const docs = repo(join(home, "docs-origin"));
  const main = repo(join(home, "project"));
  git(main, "submodule", "add", "-q", lib, "vendor/lib");
  git(main, "submodule", "add", "-q", docs, "docs");
  git(main, "commit", "-q", "-m", "submodules");
  git(main, "submodule", "update", "-q", "--init", "--recursive", "vendor/lib");
  git(main, "submodule", "deinit", "-q", "docs");
  rmSync(join(main, ".git", "modules", "docs"), { recursive: true });
  return { home, main };
}

async function create(h: ReturnType<typeof harness>, folder: string, branch: string) {
  const view = new h.Worktree();
  view.on = null;
  view.userHome = h.home;
  view.repo = await h.repoOf(folder, null);
  view.text = branch;
  return view.create();
}

test("a new worktree gets the submodules checked out in its repo, nested too, with their own objects", async () => {
  const { home, main } = project();
  const h = harness(home);
  expect(await create(h, main, "feature/one")).toBe("");
  const made = join(home, ".herdr", "worktrees", "project", "feature-one");
  expect(h.commands).toEqual([["space.new", { folder: made, machine: null, tab: undefined, argv: undefined }]]);
  expect(git(join(made, "vendor/lib"), "rev-parse", "HEAD")).toBe(git(main, "rev-parse", "HEAD:vendor/lib"));
  expect(git(join(made, "vendor/lib/deps/inner"), "rev-parse", "HEAD")).toBe(git(join(main, "vendor/lib"), "rev-parse", "HEAD:deps/inner"));
  expect(existsSync(join(made, "docs", ".git")), "one never checked out stays that way").toBe(false);
  for (const sub of ["vendor/lib", "vendor/lib/deps/inner"]) {
    const objects = git(join(made, sub), "rev-parse", "--path-format=absolute", "--git-path", "objects");
    expect(existsSync(join(objects, "info", "alternates")), `${sub} borrows nothing once made`).toBe(false);
  }
  expect(git(made, "status", "--porcelain")).toBe("");
}, slow);

test("a branch without a submodule leaves it, and what's inside it, out", async () => {
  const { home, main } = project();
  git(main, "switch", "-q", "-c", "no-lib");
  git(main, "rm", "-q", "vendor/lib");
  git(main, "commit", "-q", "-m", "drop lib");
  git(main, "switch", "-q", "main");
  git(main, "submodule", "update", "-q", "--init", "--recursive", "vendor/lib");
  expect(await create(harness(home), main, "no-lib")).toBe("");
  const made = join(home, ".herdr", "worktrees", "project", "no-lib");
  expect(existsSync(join(made, "vendor"))).toBe(false);
}, slow);

test("a worktree made from a worktree gets the submodules checked out there", async () => {
  const { home, main } = project();
  const h = harness(home);
  expect(await create(h, main, "first")).toBe("");
  const first = join(home, ".herdr", "worktrees", "project", "first");
  expect(await create(h, first, "second")).toBe("");
  const second = join(home, ".herdr", "worktrees", "project", "second");
  expect(git(join(second, "vendor/lib/deps/inner"), "rev-parse", "HEAD")).toBe(git(join(first, "vendor/lib"), "rev-parse", "HEAD:deps/inner"));
}, slow);

test("when submodules fail the worktree still opens, and the dialog says what's missing", async () => {
  const { home, main } = project();
  const h = harness(home, (args) => args[0] === "submodule");
  const error = await create(h, main, "offline");
  expect(error).toBe("The worktree is made, but its submodules aren't: fatal: offline");
  expect(h.commands.map((c) => c[0])).toEqual(["space.new"]);
}, slow);
