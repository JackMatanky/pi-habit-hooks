import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { setImmediate, setTimeout as sleep } from "node:timers/promises";
import { join } from "node:path";
import { getOwnership, getRepoRoot } from "../extensions/ownership.js";
import { FirePolicy, getMode, modeFile, persistMode, setMode } from "../extensions/policy.js";
import { runProcess } from "../extensions/runner.js";
const SMELLS = "### too-many-parameters\n- src/lib.rs:1: too many parameters\n";
function git(cwd, ...args) {
    execFileSync("git", args, { cwd, stdio: "ignore" });
}
/** Records `files` as written by `agent` alone, as a tool_result for an uncontested edit would. */
function owns(dir, file, agent = "main", soft = false) {
    const own = getOwnership(agent, getRepoRoot(dir));
    own[soft ? "soft" : "hard"].set(file, { mtimeMs: 0, size: 0 });
}
/** A committed Cargo project whose src/lib.rs has an uncommitted edit. */
function dirtyRustRepo() {
    const dir = mkdtempSync(join(tmpdir(), "habit-hooks-policy-"));
    git(dir, "init", "-q", "-b", "main");
    git(dir, "config", "user.email", "t@example.com");
    git(dir, "config", "user.name", "t");
    writeFileSync(join(dir, "Cargo.toml"), '[package]\nname = "t"\n');
    mkdirSync(join(dir, "src"));
    writeFileSync(join(dir, "src/lib.rs"), "pub fn a() {}\n");
    git(dir, "add", ".");
    git(dir, "commit", "-qm", "base");
    edit(dir, 1);
    owns(dir, "src/lib.rs");
    return dir;
}
/** Each revision has a different size, so the changed-files fingerprint changes. */
function edit(dir, revision) {
    writeFileSync(join(dir, "src/lib.rs"), `pub fn a() {}\n${"// edit\n".repeat(revision)}`);
}
function result(exitCode, output = "", extra = {}) {
    return { exitCode, output, passed: exitCode === 0, ...extra };
}
/** Stand-in for the habit-hooks binary: records calls, takes a moment, never overlaps itself unnoticed. */
function fakeRun(...results) {
    const seen = [];
    let active = 0;
    let maxActive = 0;
    const run = async (options) => {
        seen.push(options);
        maxActive = Math.max(maxActive, ++active);
        await Promise.resolve(); // yield, so a run that overlapped another would show up in maxActive
        active--;
        return results[Math.min(seen.length - 1, results.length - 1)];
    };
    return { run, seen, maxActive: () => maxActive };
}
async function withRepo(body) {
    const dir = dirtyRustRepo();
    try {
        await body(dir);
    }
    finally {
        setMode(undefined);
        rmSync(dir, { recursive: true, force: true });
    }
}
test("concurrent stop gates share one run, and an unchanged tree is never re-checked", () => withRepo(async (dir) => {
    const fake = fakeRun(result(0));
    const policy = new FirePolicy(fake.run);
    const outcomes = await Promise.all([policy.gate(dir), policy.gate(dir), policy.gate(dir)]);
    assert.ok(outcomes.every((o) => !o.block && o.status === "✅ clean"));
    await policy.gate(dir);
    assert.equal(fake.seen.length, 1);
    edit(dir, 2);
    await policy.gate(dir);
    assert.equal(fake.seen.length, 2);
}));
test("the explicit tool path reuses the run the gate already paid for", () => withRepo(async (dir) => {
    const fake = fakeRun(result(0, "all good"));
    const policy = new FirePolicy(fake.run);
    await policy.gate(dir);
    const explicit = await policy.runWorking(dir);
    assert.equal(explicit.output, "all good");
    assert.equal(fake.seen.length, 1);
}));
test("the gate refuses once per unchanged set of files, even across user prompts", () => withRepo(async (dir) => {
    const fake = fakeRun(result(1, SMELLS));
    const policy = new FirePolicy(fake.run);
    const first = await policy.gate(dir);
    assert.equal(first.block, true);
    assert.match(first.feedback ?? "", /too-many-parameters/);
    // same files, same report: the agent already knows, so it may finish, and a new prompt changes nothing
    const again = await policy.gate(dir);
    assert.equal(again.block, false);
    assert.match(again.status ?? "", /already reported/);
    policy.resetBlocks(dir);
    assert.equal((await policy.gate(dir)).block, false);
    assert.equal(fake.seen.length, 1);
    // the agent changed the files, so a fresh report is worth one more refusal
    edit(dir, 2);
    assert.equal((await policy.gate(dir)).block, true);
}));
test("the gate refuses at most twice per user prompt even as files keep changing", () => withRepo(async (dir) => {
    const policy = new FirePolicy(fakeRun(result(1, SMELLS)).run);
    assert.equal((await policy.gate(dir)).block, true);
    edit(dir, 2);
    assert.equal((await policy.gate(dir)).block, true);
    edit(dir, 3);
    const released = await policy.gate(dir);
    assert.equal(released.block, false);
    assert.match(released.status ?? "", /gate released/);
    policy.resetBlocks(dir); // next user prompt
    edit(dir, 4);
    assert.equal((await policy.gate(dir)).block, true);
}));
test("timeouts and tool failures never block the agent, and a timeout pauses automatic runs", () => withRepo(async (dir) => {
    const timeout = fakeRun(result(124, "timed out", { timedOut: true }));
    const policy = new FirePolicy(timeout.run);
    const first = await policy.gate(dir);
    assert.equal(first.block, false);
    assert.equal(first.status, "⏱ timed out");
    edit(dir, 2);
    const paused = await policy.gate(dir);
    assert.equal(paused.block, false);
    assert.equal(paused.status, "⏱ paused after timeout");
    assert.equal(timeout.seen.length, 1);
    // an explicit request still runs while automatic runs are paused
    await policy.runWorking(dir);
    assert.equal(timeout.seen.length, 2);
    getOwnership("main", getRepoRoot(dir)).suspendedUntil = 0; // suspension belongs to the agent, not to the policy instance
    const broken = new FirePolicy(fakeRun(result(2, "invalid stdin")).run);
    const outcome = await broken.gate(dir);
    assert.equal(outcome.block, false);
    assert.equal(outcome.status, "⚠️ habit-hooks failed");
}));
test("nothing runs when firing is off, no Rust file changed, or the project is not a Cargo project", () => withRepo(async (dir) => {
    const fake = fakeRun(result(0));
    const policy = new FirePolicy(fake.run);
    setMode("off");
    await policy.gate(dir);
    setMode(undefined);
    assert.equal(fake.seen.length, 0);
    git(dir, "commit", "-qam", "commit the edit");
    await policy.gate(dir);
    assert.equal(fake.seen.length, 0);
    const plain = mkdtempSync(join(tmpdir(), "habit-hooks-plain-"));
    try {
        await policy.gate(plain);
        assert.equal(fake.seen.length, 0);
    }
    finally {
        rmSync(plain, { recursive: true, force: true });
    }
}));
test("runs for one project never overlap, whoever asks", () => withRepo(async (dir) => {
    const fake = fakeRun(result(0));
    const policy = new FirePolicy(fake.run);
    await Promise.all([
        policy.scanEdited(dir, "src/lib.rs"),
        policy.scanEdited(dir, "src/lib.rs"),
        policy.runWorking(dir),
        policy.exclusive(dir, () => fake.run({ scope: "all", cwd: dir })),
    ]);
    assert.equal(fake.seen.length, 4);
    assert.equal(fake.maxActive(), 1);
}));
test("an edit scan targets the edited file with a bounded timeout and reports smells", () => withRepo(async (dir) => {
    const fake = fakeRun(result(1, SMELLS));
    const policy = new FirePolicy(fake.run);
    const scan = await policy.scanEdited(dir, "src/lib.rs");
    assert.deepEqual(fake.seen[0].files, ["src/lib.rs"]);
    assert.ok((fake.seen[0].timeoutMs ?? Infinity) <= 30_000);
    assert.equal(scan.status, "⚠️ 1 smell(s)");
    assert.equal(scan.output, SMELLS);
}));
const INCOMPLETE = "⚠️ Habit Hooks: Incomplete Run\n\nAutomated checks could not run to completion.\n\n- sensor:clippy: Crate compilation failed in files outside sensor scope\n";
test("the gate checks only the files the asking agent wrote and ignores everyone else's", () => withRepo(async (dir) => {
    const fake = fakeRun(result(1, SMELLS));
    const policy = new FirePolicy(fake.run);
    // src/lib.rs is dirty and owned by "main"; "other" wrote nothing
    const bystander = await policy.gate(dir, undefined, "other");
    assert.deepEqual(bystander, { block: false });
    assert.equal(fake.seen.length, 0);
    const owner = await policy.gate(dir, undefined, "main");
    assert.equal(owner.block, true);
    assert.deepEqual(fake.seen[0].files, ["src/lib.rs"]);
}));
test("with two agents in one repo, each gate scans only its own file and only the guilty one is blocked", () => withRepo(async (dir) => {
    writeFileSync(join(dir, "src/a.rs"), "pub fn bad() {}\n");
    writeFileSync(join(dir, "src/b.rs"), "pub fn good() {}\n");
    owns(dir, "src/a.rs", "agent-a");
    owns(dir, "src/b.rs", "agent-b");
    getOwnership("main", getRepoRoot(dir)).hard.clear(); // src/lib.rs is nobody's here
    const seen = [];
    const policy = new FirePolicy(async (options) => {
        seen.push(options.files ?? []);
        return options.files?.includes("src/a.rs") ? result(1, "### too-many-parameters\n- src/a.rs:1: bad\n") : result(0);
    });
    const [b, a] = await Promise.all([policy.gate(dir, undefined, "agent-b"), policy.gate(dir, undefined, "agent-a")]);
    assert.equal(b.block, false);
    assert.equal(b.status, "✅ clean");
    assert.equal(a.block, true);
    assert.deepEqual(seen.map((files) => files.join()).sort(), ["src/a.rs", "src/b.rs"]);
}));
test("smells in soft-owned files, and a build broken elsewhere, are reported but never block", () => withRepo(async (dir) => {
    const own = getOwnership("main", getRepoRoot(dir));
    own.soft.set("src/lib.rs", own.hard.get("src/lib.rs"));
    own.hard.clear();
    const soft = await new FirePolicy(fakeRun(result(1, SMELLS)).run).gate(dir);
    assert.equal(soft.block, false);
    assert.match(soft.status ?? "", /advisory/);
    assert.match(soft.feedback ?? "", /too-many-parameters/);
    owns(dir, "src/lib.rs"); // hard-owned again, but the report only says the crate does not build
    edit(dir, 2);
    const broken = await new FirePolicy(fakeRun(result(1, INCOMPLETE)).run).gate(dir);
    assert.equal(broken.block, false);
    assert.match(broken.status ?? "", /incomplete run/);
}));
test("attribution is dropped once a file is committed, and the agent's tool path covers only its own files", () => withRepo(async (dir) => {
    const fake = fakeRun(result(0, "ok"));
    const policy = new FirePolicy(fake.run);
    const own = getOwnership("main", getRepoRoot(dir));
    assert.equal((await policy.runOwned(dir, undefined, "main"))?.output, "ok");
    assert.deepEqual(fake.seen[0].files, ["src/lib.rs"]);
    assert.equal(await policy.runOwned(dir, undefined, "other"), undefined);
    git(dir, "commit", "-qam", "commit the edit");
    assert.equal(await policy.runOwned(dir, undefined, "main"), undefined);
    assert.equal(own.hard.size, 0);
}));
test("an edit scan only covers files the agent wrote alone", () => withRepo(async (dir) => {
    const fake = fakeRun(result(1, SMELLS));
    const policy = new FirePolicy(fake.run);
    assert.deepEqual(await policy.scanEdited(dir, "src/lib.rs", undefined, "other"), {});
    const own = getOwnership("main", getRepoRoot(dir));
    own.soft.set("src/lib.rs", own.hard.get("src/lib.rs"));
    own.hard.clear();
    assert.deepEqual(await policy.scanEdited(dir, "src/lib.rs"), {});
    assert.equal(fake.seen.length, 0);
}));
test("runProcess returns a pending promise instead of blocking the event loop", async () => {
    const pending = runProcess("sleep", ["0.3"], { cwd: process.cwd() });
    let settled = false;
    void pending.then(() => {
        settled = true;
    });
    await setImmediate(); // a spawnSync-based runner would already have settled by now
    assert.equal(settled, false);
    assert.equal((await pending).code, 0);
});
// Real time on purpose: this checks that an actual timeout makes the OS reap grandchildren, which fake timers cannot show.
test("runProcess timeout kills the whole process group, not just the direct child", async () => {
    const dir = mkdtempSync(join(tmpdir(), "habit-hooks-kill-"));
    try {
        const pidFile = join(dir, "grandchild.pid");
        const result = await runProcess("sh", ["-c", `sleep 60 & echo $! > "${pidFile}"; wait`], {
            cwd: dir,
            timeoutMs: 500,
        });
        assert.equal(result.timedOut, true);
        const pid = Number(readFileSync(pidFile, "utf8"));
        const alive = () => {
            try {
                process.kill(pid, 0);
                return true;
            }
            catch {
                return false;
            }
        };
        // the killed child is a zombie until init reaps it, so give the kernel a few turns
        for (let i = 0; i < 40 && alive(); i++)
            await sleep(50);
        assert.equal(alive(), false, "grandchild survived the timeout");
    }
    finally {
        rmSync(dir, { recursive: true, force: true });
    }
});
test("runProcess stops when the caller aborts", async () => {
    const controller = new AbortController();
    const pending = runProcess("sleep", ["30"], { cwd: process.cwd(), signal: controller.signal });
    controller.abort();
    assert.equal((await pending).aborted, true);
});
test("a persisted off survives a restart, and env or a process override still win", () => {
    const dir = mkdtempSync(join(tmpdir(), "habit-hooks-mode-"));
    const saved = { file: process.env.HABIT_HOOKS_MODE_FILE, env: process.env.HABIT_HOOKS_MODE };
    process.env.HABIT_HOOKS_MODE_FILE = join(dir, "nested", "mode");
    delete process.env.HABIT_HOOKS_MODE;
    try {
        assert.equal(getMode(), "turn");
        persistMode("off");
        setMode(undefined); // a fresh process has no in-memory override
        assert.equal(getMode(), "off");
        assert.equal(readFileSync(modeFile(), "utf8"), "off\n");
        process.env.HABIT_HOOKS_MODE = "edit";
        assert.equal(getMode(), "edit");
        setMode("turn");
        assert.equal(getMode(), "turn");
    }
    finally {
        setMode(undefined);
        for (const [key, value] of [["HABIT_HOOKS_MODE_FILE", saved.file], ["HABIT_HOOKS_MODE", saved.env]]) {
            if (value === undefined)
                delete process.env[key];
            else
                process.env[key] = value;
        }
        rmSync(dir, { recursive: true, force: true });
    }
});
