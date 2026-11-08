import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { initRepo } from "./helpers.js";
import { closeWindow, getOwnership, getRepoRoot, openWindow, rebuildFromBranch, registerAgent, resolveRoot, toRepoPath, unregisterAgent, } from "../extensions/ownership.js";
/** A committed repo with src/a.rs, src/b.rs and src/c.rs; returns its (realpath) root. */
async function withRepo(body) {
    const dir = initRepo("habit-hooks-own-", Object.fromEntries(["a", "b", "c"].map((n) => [`src/${n}.rs`, `pub fn ${n}() {}\n`])));
    try {
        await body(getRepoRoot(dir));
    }
    finally {
        rmSync(dir, { recursive: true, force: true });
    }
}
let revision = 0;
/** Every write has a distinct size, so a changed signature never depends on timestamp resolution. */
function write(root, name) {
    writeFileSync(join(root, "src", `${name}.rs`), `pub fn ${name}() {}\n${"// r\n".repeat(++revision)}`);
}
test("an uncontested write is hard-owned, and files it did not touch are never claimed", () => withRepo(async (root) => {
    write(root, "c"); // dirty before the call, by nobody in particular
    await openWindow("A", root, "t1", false);
    write(root, "a");
    const owned = await closeWindow("A", root, "t1");
    assert.deepEqual(owned, { hard: ["src/a.rs"], soft: [] });
    assert.deepEqual([...getOwnership("A", root).hard.keys()], ["src/a.rs"]);
}));
test("a named file needs no snapshot and is attributed to the window's agent", () => withRepo(async (root) => {
    await openWindow("A", root, "t1", false, false);
    write(root, "a");
    assert.deepEqual(await closeWindow("A", root, "t1", "src/a.rs"), {
        hard: ["src/a.rs"],
        soft: [],
    });
}));
test("writes during another agent's write window are soft, on both sides, until a clean rewrite", () => withRepo(async (root) => {
    await openWindow("A", root, "a1", false, false);
    await openWindow("B", root, "b1", false);
    write(root, "a");
    write(root, "b");
    const a = await closeWindow("A", root, "a1", "src/a.rs");
    const b = await closeWindow("B", root, "b1");
    assert.deepEqual(a, { hard: [], soft: ["src/a.rs"] });
    assert.ok(b.soft.includes("src/b.rs") && b.hard.length === 0);
    assert.equal(getOwnership("A", root).hard.size, 0);
    // later, alone: the file becomes the agent's own
    await openWindow("A", root, "a2", false, false);
    write(root, "a");
    assert.deepEqual(await closeWindow("A", root, "a2", "src/a.rs"), {
        hard: ["src/a.rs"],
        soft: [],
    });
    assert.equal(getOwnership("A", root).soft.size, 0);
}));
test("a read-only window of another agent never makes a write soft", () => withRepo(async (root) => {
    await openWindow("B", root, "b1", true);
    await openWindow("A", root, "a1", false, false);
    write(root, "a");
    assert.deepEqual(await closeWindow("A", root, "a1", "src/a.rs"), {
        hard: ["src/a.rs"],
        soft: [],
    });
    await closeWindow("B", root, "b1");
}));
test("a subagent's writes belong to its root and never overlap with it", () => withRepo(async (root) => {
    registerAgent("sub-1", "main-1");
    assert.equal(resolveRoot("sub-1"), "main-1");
    assert.equal(resolveRoot("unrelated"), "unrelated");
    assert.equal(resolveRoot(undefined), "main");
    await openWindow(resolveRoot("main-1"), root, "m1", false, false);
    await openWindow(resolveRoot("sub-1"), root, "s1", false, false);
    write(root, "b");
    assert.deepEqual(await closeWindow(resolveRoot("sub-1"), root, "s1", "src/b.rs"), { hard: ["src/b.rs"], soft: [] });
    await closeWindow(resolveRoot("main-1"), root, "m1");
    // the root's ownership survives a subagent shutting down, and dies with the root
    unregisterAgent("sub-1");
    assert.equal(resolveRoot("sub-1"), "sub-1");
    assert.equal(getOwnership("main-1", root).hard.size, 1);
    unregisterAgent("main-1");
    assert.equal(getOwnership("main-1", root).hard.size, 0);
}));
test("a live process with an open write window makes overlapping writes soft; a dead one is ignored", () => withRepo(async (root) => {
    const agents = join(root, ".habit-hooks", "agents");
    mkdirSync(agents, { recursive: true });
    const dead = spawnSync(process.execPath, ["-e", "process.stdout.write(String(process.pid))"], { encoding: "utf8" });
    const windowOf = (pid) => JSON.stringify({
        pid,
        windows: [{ agentRoot: "elsewhere", start: Date.now() - 1000 }],
    });
    writeFileSync(join(agents, `${Number(dead.stdout)}.json`), windowOf(Number(dead.stdout)));
    await openWindow("A", root, "a1", false, false);
    write(root, "a");
    assert.deepEqual(await closeWindow("A", root, "a1", "src/a.rs"), {
        hard: ["src/a.rs"],
        soft: [],
    });
    writeFileSync(join(agents, `${process.ppid}.json`), windowOf(process.ppid));
    await openWindow("A", root, "a2", false, false);
    write(root, "b");
    assert.deepEqual(await closeWindow("A", root, "a2", "src/b.rs"), {
        hard: [],
        soft: ["src/b.rs"],
    });
}));
test("ownership is rebuilt from stamps on the session branch and reset on rewind", () => withRepo(async (root) => {
    const stamp = (hard, soft) => ({
        type: "message",
        message: {
            role: "toolResult",
            details: { habitHooks: { hard, soft } },
        },
    });
    const branch = [
        { type: "message", message: { role: "user" } },
        stamp(["src/a.rs"], ["src/b.rs"]),
        stamp([], ["src/a.rs", "src/c.rs"]), // a later soft claim never demotes a hard file
        stamp(["src/b.rs"], []), // but a later hard claim promotes a soft one
    ];
    rebuildFromBranch("R", root, branch);
    const own = getOwnership("R", root);
    assert.deepEqual([...own.hard.keys()].sort(), ["src/a.rs", "src/b.rs"]);
    assert.deepEqual([...own.soft.keys()], ["src/c.rs"]);
    rebuildFromBranch("R", root, branch.slice(0, 1)); // rewound before any write
    assert.equal(own.hard.size + own.soft.size, 0);
}));
test("tool paths resolve to repo-relative paths, through symlinked temp dirs and from subdirectories", () => withRepo(async (root) => {
    assert.equal(toRepoPath(root, root, "src/a.rs"), "src/a.rs");
    assert.equal(toRepoPath(root, root, join(root, "src/a.rs")), "src/a.rs");
    assert.equal(toRepoPath(join(root, "src"), root, "a.rs"), "src/a.rs");
    assert.equal(toRepoPath(join(root, "src"), root, "../src/b.rs"), "src/b.rs");
    assert.equal(toRepoPath(root, root, "../outside.rs"), undefined);
    assert.equal(toRepoPath(root, root, "src/new/deeper.rs"), "src/new/deeper.rs");
}));
