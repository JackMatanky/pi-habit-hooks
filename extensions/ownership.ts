import type { FireMode } from "./policy.js";

import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import {
  basename,
  dirname,
  isAbsolute,
  join,
  relative,
  resolve,
} from "node:path";

import { runProcess } from "./runner.js";

/**
 * Who wrote which Rust file. Habit Hooks only ever checks, reports and blocks
 * on files the calling agent wrote:
 * - hard: this agent changed the file and no other agent's tool call overlapped
 *         in time, so a smell is its own;
 * - soft: another agent's tool call overlapped, so attribution is uncertain and
 *         findings are advisory only.
 * Subagents roll up into their root agent, so the main session answers for what
 * its subagents wrote.
 */
export interface FileSignature {
  mtimeMs: number;
  size: number;
}

export interface AgentOwnership {
  hard: Map<string, FileSignature>;
  soft: Map<string, FileSignature>;
  /** Gate refusals issued since the last user prompt. */
  blocks: number;
  /**
   * Fingerprint of the owned files the gate last refused for; the same files
   * are never refused twice.
   */
  blockedFingerprint?: string;
  /** Automatic runs are paused until then after a timeout. */
  suspendedUntil: number;
  /** Per-agent override of the global fire mode. */
  mode?: FireMode;
  lastGateOutcome?: {
    action: "blocked" | "released" | "clean" | "skipped" | "repeated";
    smellCount: number;
    durationMs: number;
    at: number;
  };
}

export interface ActiveWindow {
  agentRoot: string;
  repoRoot: string;
  start: number;
  toolCallId: string;
  /**
   * Read-only windows cannot have written anything, so they never make
   * another agent's attribution soft.
   */
  readOnly: boolean;
  preSnap?: Map<string, FileSignature>;
}

interface ClosedWindow {
  agentRoot: string;
  repoRoot: string;
  start: number;
  end: number;
}

const GIT_TIMEOUT_MS = 10_000;
/** Window files and closed windows older than this are ignored. */
const WINDOW_TTL_MS = 10 * 60_000;
const CLOSED_CAP = 200;

const parentMap = new Map<string, string>();
const ownershipMap = new Map<string, AgentOwnership>();
const activeWindows = new Map<string, ActiveWindow>();
const repoMutex = new Map<string, Promise<unknown>>();
let closedWindows: ClosedWindow[] = [];
const repoRoots = new Map<string, string>();
/** Repos whose per-process window file this process wrote; removed on exit. */
const publishedRepos = new Set<string>();
/**
 * Repos where `.habit-hooks/agents` cannot be written: cross-process detection
 * stays off, in-process still works.
 */
const unwritableRepos = new Set<string>();

process.once("exit", () => {
  for (const repoRoot of publishedRepos)
    rmSync(windowFile(repoRoot), { force: true });
});

// -----------------------------------------------------------------------------
// Identity
// -----------------------------------------------------------------------------

export function registerAgent(agentId: string, parentId?: string): void {
  if (parentId && parentId !== agentId) parentMap.set(agentId, parentId);
}

/**
 * A root agent's ownership dies with its session; a subagent's files already
 * live under its root.
 */
export function unregisterAgent(agentId: string): void {
  if (parentMap.delete(agentId)) return;
  // deleting the current entry while iterating a Map is well defined
  for (const key of ownershipMap.keys())
    if (key.startsWith(`${agentId}\0`)) ownershipMap.delete(key);
}

/**
 * The root agent a (sub)agent answers to; unknown or absent ids are the main
 * agent.
 */
export function resolveRoot(agentId?: string): string {
  let id = agentId ?? "main";
  for (let hops = 0; hops < 32; hops++) {
    const parent = parentMap.get(id);
    if (!parent) return id;
    id = parent;
  }
  return id;
}

export function getRepoRoot(cwd: string): string {
  const cached = repoRoots.get(cwd);
  if (cached) return cached;
  const top = spawnSync("git", ["rev-parse", "--show-toplevel"], {
    cwd,
    encoding: "utf8",
    timeout: GIT_TIMEOUT_MS,
  });
  if (top.status === 0 && top.stdout.trim()) {
    const root = top.stdout.trim();
    repoRoots.set(cwd, root);
    return root;
  }
  // not a repository (yet): do not cache, `git init` may follow
  try {
    return realpathSync(cwd);
  } catch {
    return cwd;
  }
}

export function getOwnership(
  agentRoot: string,
  repoRoot: string,
): AgentOwnership {
  const key = `${agentRoot}\0${repoRoot}`;
  let own = ownershipMap.get(key);
  if (!own) {
    own = {
      hard: new Map(),
      soft: new Map(),
      blocks: 0,
      suspendedUntil: 0,
    };
    ownershipMap.set(key, own);
  }
  return own;
}

// ---------------------------------------------------------------------------------------------------------------
// Paths and signatures

/** A path as a tool received it (absolute, or relative to cwd) to the repo-root-relative form `git status` uses. */
export function toRepoPath(
  cwd: string,
  repoRoot: string,
  file: string,
): string | undefined {
  let abs = isAbsolute(file) ? file : resolve(cwd, file);
  try {
    abs = join(realpathSync(dirname(abs)), basename(abs));
  } catch {
    // the directory does not exist (yet); keep the lexical path
  }
  const rel = relative(repoRoot, abs).split("\\").join("/");
  return rel === "" || rel.startsWith("..") || isAbsolute(rel)
    ? undefined
    : rel;
}

export function fileSignature(repoRoot: string, file: string): FileSignature {
  try {
    const stat = statSync(join(repoRoot, file));
    return { mtimeMs: stat.mtimeMs, size: stat.size };
  } catch {
    return { mtimeMs: 0, size: 0 }; // deleted files are still attributable
  }
}

/** Paths from `git status --porcelain -z`; a rename or copy is followed by an entry holding the old path. */
export function parsePorcelain(stdout: string): string[] {
  const entries = stdout.split("\0").filter(Boolean);
  const files: string[] = [];
  for (let i = 0; i < entries.length; i++) {
    files.push(entries[i].slice(3));
    if (/[RC]/.test(entries[i].slice(0, 2))) i++;
  }
  return files;
}

async function snapshotRust(
  repoRoot: string,
): Promise<Map<string, FileSignature>> {
  const snap = new Map<string, FileSignature>();
  const status = await runProcess(
    "git",
    ["status", "--porcelain", "-z", "--untracked-files=all", "--", "*.rs"],
    {
      cwd: repoRoot,
      timeoutMs: GIT_TIMEOUT_MS,
    },
  );
  if (status.code !== 0) return snap;
  for (const file of parsePorcelain(status.stdout))
    snap.set(file, fileSignature(repoRoot, file));
  return snap;
}

// ---------------------------------------------------------------------------------------------------------------
// Per-repo run lock

/** Runs `job` after every earlier job for this repository settled (cargo locks its target dir anyway). */
export function withRepoLock<T>(
  repoRoot: string,
  job: () => Promise<T>,
): Promise<T> {
  const next = (repoMutex.get(repoRoot) ?? Promise.resolve()).then(job);
  repoMutex.set(
    repoRoot,
    next.then(
      () => undefined,
      () => undefined,
    ),
  );
  return next;
}

// ---------------------------------------------------------------------------------------------------------------
// Windows: the span of one tool call, used to tell whose write a changed file is

function windowFile(repoRoot: string): string {
  return join(repoRoot, ".habit-hooks", "agents", `${process.pid}.json`);
}

/** Publishes this process's write windows for other processes on the same repo; best effort. */
function publish(repoRoot: string): void {
  if (unwritableRepos.has(repoRoot)) return;
  const windows: Array<{ agentRoot: string; start: number; end?: number }> = [];
  for (const w of activeWindows.values()) {
    if (w.repoRoot === repoRoot && !w.readOnly)
      windows.push({ agentRoot: w.agentRoot, start: w.start });
  }
  for (const c of closedWindows) {
    if (c.repoRoot === repoRoot)
      windows.push({
        agentRoot: c.agentRoot,
        start: c.start,
        end: c.end,
      });
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
  } catch {
    unwritableRepos.add(repoRoot);
  }
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    return (err as NodeJS.ErrnoException).code === "EPERM";
  }
}

/** Another process's write window overlapped [start, end]. Dead processes and stale files are skipped. */
function foreignOverlap(repoRoot: string, start: number, end: number): boolean {
  const dir = dirname(windowFile(repoRoot));
  let names: string[];
  try {
    names = readdirSync(dir);
  } catch {
    return false;
  }
  const own = `${process.pid}.json`;
  for (const name of names) {
    if (!name.endsWith(".json") || name === own) continue;
    const path = join(dir, name);
    try {
      if (Date.now() - statSync(path).mtimeMs > WINDOW_TTL_MS) continue;
      const record = JSON.parse(readFileSync(path, "utf8")) as {
        pid: number;
        windows: Array<{ start: number; end?: number }>;
      };
      if (!alive(record.pid)) continue;
      if (
        record.windows.some(
          (w) => w.start <= end && (w.end === undefined || w.end > start),
        )
      )
        return true;
    } catch {
      // unreadable or half-written: ignore
    }
  }
  return false;
}

/** Another root agent's write window overlapped [start, end], in this process or in another one. */
function overlapsOther(
  agentRoot: string,
  repoRoot: string,
  start: number,
  end: number,
): boolean {
  for (const w of activeWindows.values()) {
    if (
      w.repoRoot === repoRoot &&
      !w.readOnly &&
      w.agentRoot !== agentRoot &&
      w.start <= end
    )
      return true;
  }
  for (const c of closedWindows) {
    if (
      c.repoRoot === repoRoot &&
      c.agentRoot !== agentRoot &&
      c.start <= end &&
      c.end > start
    )
      return true;
  }
  return foreignOverlap(repoRoot, start, end);
}

/**
 * Marks the start of a tool call. Read-only calls are recorded but never snapshot and never cause overlap.
 * `snapshot` is false when the call names the one file it writes, so no snapshot is needed to find it.
 */
export async function openWindow(
  agentRoot: string,
  repoRoot: string,
  toolCallId: string,
  isReadOnly: boolean,
  snapshot: boolean = !isReadOnly,
): Promise<void> {
  const win: ActiveWindow = {
    agentRoot,
    repoRoot,
    start: Date.now(),
    toolCallId,
    readOnly: isReadOnly,
  };
  activeWindows.set(toolCallId, win); // registered before the snapshot so a concurrent close already sees the overlap
  if (!isReadOnly) publish(repoRoot);
  if (snapshot && !isReadOnly) win.preSnap = await snapshotRust(repoRoot);
}

/**
 * Ends a tool call and attributes the Rust files it wrote: the named `directFile`, or else every file whose
 * signature changed since the opening snapshot. Files written while another agent's call overlapped are soft.
 */
export async function closeWindow(
  agentRoot: string,
  repoRoot: string,
  toolCallId: string,
  directFile?: string,
): Promise<{ hard: string[]; soft: string[] }> {
  const win = activeWindows.get(toolCallId);
  const changed = new Map<string, FileSignature>();
  if (directFile) {
    changed.set(directFile, fileSignature(repoRoot, directFile));
  } else if (win?.preSnap) {
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
    closedWindows = closedWindows
      .filter((c) => end - c.end < WINDOW_TTL_MS)
      .slice(-CLOSED_CAP);
    publish(repoRoot);
  }

  const out = { hard: [] as string[], soft: [] as string[] };
  if (changed.size === 0) return out;

  const soft = overlapsOther(agentRoot, repoRoot, start, end);
  const own = getOwnership(agentRoot, repoRoot);
  for (const [file, sig] of changed) {
    if (!soft) {
      own.hard.set(file, sig);
      own.soft.delete(file);
      out.hard.push(file);
    } else if (!own.hard.has(file)) {
      own.soft.set(file, sig);
      out.soft.push(file);
    }
  }
  return out;
}

// ---------------------------------------------------------------------------------------------------------------
// Persistence on the session branch
// ---------------------------------------------------------------------------------------------------------------

function stampOf(
  entry: unknown,
): { hard: string[]; soft: string[] } | undefined {
  const e = entry as {
    details?: unknown;
    message?: { details?: unknown };
  } | null;
  const stamp = (
    (e?.message?.details ?? e?.details) as
      | { habitHooks?: { hard?: unknown; soft?: unknown } }
      | undefined
  )?.habitHooks;
  if (!stamp) return undefined;
  const strings = (v: unknown) =>
    Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  return { hard: strings(stamp.hard), soft: strings(stamp.soft) };
}

/** Replays the `details.habitHooks` stamps of a session branch, so ownership survives resume and rewind. */
export function rebuildFromBranch(
  agentRoot: string,
  repoRoot: string,
  branchEntries: unknown[],
): void {
  const own = getOwnership(agentRoot, repoRoot);
  own.hard.clear();
  own.soft.clear();
  for (const entry of branchEntries) {
    const stamp = stampOf(entry);
    if (!stamp) continue;
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
