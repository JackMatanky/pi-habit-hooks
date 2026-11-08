import { spawnSync } from "node:child_process";
import { mkdirSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve } from "node:path";
import { runProcess } from "./runner.js";
const GIT_TIMEOUT_MS = 10_000;
/** Window files and closed windows older than this are ignored. */
const WINDOW_TTL_MS = 10 * 60_000;
const CLOSED_CAP = 200;
const parentMap = new Map();
const ownershipMap = new Map();
const activeWindows = new Map();
const repoMutex = new Map();
let closedWindows = [];
const repoRoots = new Map();
/** Repos whose per-process window file this process wrote; removed on exit. */
const publishedRepos = new Set();
/** Repos where `.habit-hooks/agents` cannot be written: cross-process detection stays off, in-process still works. */
const unwritableRepos = new Set();
process.once("exit", () => {
    for (const repoRoot of publishedRepos)
        rmSync(windowFile(repoRoot), { force: true });
});
// ---------------------------------------------------------------------------------------------------------------
// Identity
export function registerAgent(agentId, parentId) {
    if (parentId && parentId !== agentId)
        parentMap.set(agentId, parentId);
}
/** A root agent's ownership dies with its session; a subagent's files already live under its root. */
export function unregisterAgent(agentId) {
    if (parentMap.delete(agentId))
        return;
    for (const key of [...ownershipMap.keys()])
        if (key.startsWith(`${agentId}\0`))
            ownershipMap.delete(key);
}
/** The root agent a (sub)agent answers to; unknown or absent ids are the main agent. */
export function resolveRoot(agentId) {
    let id = agentId ?? "main";
    for (let hops = 0; hops < 32; hops++) {
        const parent = parentMap.get(id);
        if (!parent)
            return id;
        id = parent;
    }
    return id;
}
export function getRepoRoot(cwd) {
    const cached = repoRoots.get(cwd);
    if (cached)
        return cached;
    const top = spawnSync("git", ["rev-parse", "--show-toplevel"], { cwd, encoding: "utf8", timeout: GIT_TIMEOUT_MS });
    if (top.status === 0 && top.stdout.trim()) {
        const root = top.stdout.trim();
        repoRoots.set(cwd, root);
        return root;
    }
    // not a repository (yet): do not cache, `git init` may follow
    try {
        return realpathSync(cwd);
    }
    catch {
        return cwd;
    }
}
export function getOwnership(agentRoot, repoRoot) {
    const key = `${agentRoot}\0${repoRoot}`;
    let own = ownershipMap.get(key);
    if (!own) {
        own = { hard: new Map(), soft: new Map(), blocks: 0, suspendedUntil: 0 };
        ownershipMap.set(key, own);
    }
    return own;
}
// ---------------------------------------------------------------------------------------------------------------
// Paths and signatures
/** A path as a tool received it (absolute, or relative to cwd) to the repo-root-relative form `git status` uses. */
export function toRepoPath(cwd, repoRoot, file) {
    let abs = isAbsolute(file) ? file : resolve(cwd, file);
    try {
        abs = join(realpathSync(dirname(abs)), basename(abs));
    }
    catch {
        // the directory does not exist (yet); keep the lexical path
    }
    const rel = relative(repoRoot, abs).split("\\").join("/");
    return rel === "" || rel.startsWith("..") || isAbsolute(rel) ? undefined : rel;
}
export function fileSignature(repoRoot, file) {
    try {
        const stat = statSync(join(repoRoot, file));
        return { mtimeMs: stat.mtimeMs, size: stat.size };
    }
    catch {
        return { mtimeMs: 0, size: 0 }; // deleted files are still attributable
    }
}
/** Paths from `git status --porcelain -z`; a rename or copy is followed by an entry holding the old path. */
export function parsePorcelain(stdout) {
    const entries = stdout.split("\0").filter(Boolean);
    const files = [];
    for (let i = 0; i < entries.length; i++) {
        files.push(entries[i].slice(3));
        if (/[RC]/.test(entries[i].slice(0, 2)))
            i++;
    }
    return files;
}
async function snapshotRust(repoRoot) {
    const snap = new Map();
    const status = await runProcess("git", ["status", "--porcelain", "-z", "--untracked-files=all", "--", "*.rs"], {
        cwd: repoRoot,
        timeoutMs: GIT_TIMEOUT_MS,
    });
    if (status.code !== 0)
        return snap;
    for (const file of parsePorcelain(status.stdout))
        snap.set(file, fileSignature(repoRoot, file));
    return snap;
}
// ---------------------------------------------------------------------------------------------------------------
// Per-repo run lock
/** Runs `job` after every earlier job for this repository settled (cargo locks its target dir anyway). */
export function withRepoLock(repoRoot, job) {
    const next = (repoMutex.get(repoRoot) ?? Promise.resolve()).then(job);
    repoMutex.set(repoRoot, next.then(() => undefined, () => undefined));
    return next;
}
// ---------------------------------------------------------------------------------------------------------------
// Windows: the span of one tool call, used to tell whose write a changed file is
function windowFile(repoRoot) {
    return join(repoRoot, ".habit-hooks", "agents", `${process.pid}.json`);
}
/** Publishes this process's write windows for other processes on the same repo; best effort. */
function publish(repoRoot) {
    if (unwritableRepos.has(repoRoot))
        return;
    const windows = [];
    for (const w of activeWindows.values()) {
        if (w.repoRoot === repoRoot && !w.readOnly)
            windows.push({ agentRoot: w.agentRoot, start: w.start });
    }
    for (const c of closedWindows) {
        if (c.repoRoot === repoRoot)
            windows.push({ agentRoot: c.agentRoot, start: c.start, end: c.end });
    }
    const file = windowFile(repoRoot);
    try {
        if (windows.length === 0) {
            rmSync(file, { force: true });
            return;
        }
        mkdirSync(dirname(file), { recursive: true });
        writeFileSync(file, JSON.stringify({ pid: process.pid, windows }));
        publishedRepos.add(repoRoot);
    }
    catch {
        unwritableRepos.add(repoRoot);
    }
}
function alive(pid) {
    try {
        process.kill(pid, 0);
        return true;
    }
    catch (err) {
        return err.code === "EPERM";
    }
}
/** Another process's write window overlapped [start, end]. Dead processes and stale files are skipped. */
function foreignOverlap(repoRoot, start, end) {
    const dir = dirname(windowFile(repoRoot));
    let names;
    try {
        names = readdirSync(dir);
    }
    catch {
        return false;
    }
    const own = `${process.pid}.json`;
    for (const name of names) {
        if (!name.endsWith(".json") || name === own)
            continue;
        const path = join(dir, name);
        try {
            if (Date.now() - statSync(path).mtimeMs > WINDOW_TTL_MS)
                continue;
            const record = JSON.parse(readFileSync(path, "utf8"));
            if (!alive(record.pid))
                continue;
            if (record.windows.some((w) => w.start <= end && (w.end === undefined || w.end > start)))
                return true;
        }
        catch {
            // unreadable or half-written: ignore
        }
    }
    return false;
}
/** Another root agent's write window overlapped [start, end], in this process or in another one. */
function overlapsOther(agentRoot, repoRoot, start, end) {
    for (const w of activeWindows.values()) {
        if (w.repoRoot === repoRoot && !w.readOnly && w.agentRoot !== agentRoot && w.start <= end)
            return true;
    }
    for (const c of closedWindows) {
        if (c.repoRoot === repoRoot && c.agentRoot !== agentRoot && c.start <= end && c.end > start)
            return true;
    }
    return foreignOverlap(repoRoot, start, end);
}
/**
 * Marks the start of a tool call. Read-only calls are recorded but never snapshot and never cause overlap.
 * `snapshot` is false when the call names the one file it writes, so no snapshot is needed to find it.
 */
export async function openWindow(agentRoot, repoRoot, toolCallId, isReadOnly, snapshot = !isReadOnly) {
    const win = { agentRoot, repoRoot, start: Date.now(), toolCallId, readOnly: isReadOnly };
    activeWindows.set(toolCallId, win); // registered before the snapshot so a concurrent close already sees the overlap
    if (!isReadOnly)
        publish(repoRoot);
    if (snapshot && !isReadOnly)
        win.preSnap = await snapshotRust(repoRoot);
}
/**
 * Ends a tool call and attributes the Rust files it wrote: the named `directFile`, or else every file whose
 * signature changed since the opening snapshot. Files written while another agent's call overlapped are soft.
 */
export async function closeWindow(agentRoot, repoRoot, toolCallId, directFile) {
    const win = activeWindows.get(toolCallId);
    const changed = new Map();
    if (directFile) {
        changed.set(directFile, fileSignature(repoRoot, directFile));
    }
    else if (win?.preSnap) {
        const before = win.preSnap;
        for (const [file, sig] of await snapshotRust(repoRoot)) {
            const was = before.get(file);
            if (!was || was.mtimeMs !== sig.mtimeMs || was.size !== sig.size)
                changed.set(file, sig);
        }
    }
    const end = Date.now();
    activeWindows.delete(toolCallId);
    const start = win?.start ?? end;
    if (win && !win.readOnly) {
        closedWindows.push({ agentRoot, repoRoot, start, end });
        closedWindows = closedWindows.filter((c) => end - c.end < WINDOW_TTL_MS).slice(-CLOSED_CAP);
        publish(repoRoot);
    }
    const out = { hard: [], soft: [] };
    if (changed.size === 0)
        return out;
    const soft = overlapsOther(agentRoot, repoRoot, start, end);
    const own = getOwnership(agentRoot, repoRoot);
    for (const [file, sig] of changed) {
        if (!soft) {
            own.hard.set(file, sig);
            own.soft.delete(file);
            out.hard.push(file);
        }
        else if (!own.hard.has(file)) {
            own.soft.set(file, sig);
            out.soft.push(file);
        }
    }
    return out;
}
// ---------------------------------------------------------------------------------------------------------------
// Persistence on the session branch
// ---------------------------------------------------------------------------------------------------------------
function stampOf(entry) {
    const e = entry;
    const stamp = (e?.message?.details ?? e?.details)?.habitHooks;
    if (!stamp)
        return undefined;
    const strings = (v) => (Array.isArray(v) ? v.filter((x) => typeof x === "string") : []);
    return { hard: strings(stamp.hard), soft: strings(stamp.soft) };
}
/** Replays the `details.habitHooks` stamps of a session branch, so ownership survives resume and rewind. */
export function rebuildFromBranch(agentRoot, repoRoot, branchEntries) {
    const own = getOwnership(agentRoot, repoRoot);
    own.hard.clear();
    own.soft.clear();
    for (const entry of branchEntries) {
        const stamp = stampOf(entry);
        if (!stamp)
            continue;
        for (const file of stamp.hard) {
            own.hard.set(file, fileSignature(repoRoot, file));
            own.soft.delete(file);
        }
        for (const file of stamp.soft) {
            if (!own.hard.has(file))
                own.soft.set(file, fileSignature(repoRoot, file));
        }
    }
}
