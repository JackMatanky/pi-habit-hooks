import { createHash } from "node:crypto";
import {
    existsSync,
    mkdirSync,
    readdirSync,
    readFileSync,
    realpathSync,
    statSync,
    writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { dirname, join, relative } from "node:path";
import {
    getOwnership,
    getRepoRoot,
    parsePorcelain,
    resolveRoot,
    toRepoPath,
    withRepoLock,
    type AgentOwnership,
} from "./ownership.js";
import {
    defaultTimeoutMs,
    runHabitHooks,
    runProcess,
    type HabitHooksResult,
    type RunHabitHooksOptions,
} from "./runner.js";

/**
 * When Habit Hooks fires on its own:
 * - off:  never (tools and slash commands still work when invoked explicitly)
 * - turn: once when the main session tries to stop, only for changed Rust files
 *         (default)
 * - edit: `turn`, plus a scan after every Rust edit
 */
export type FireMode = "off" | "turn" | "edit";

let sessionMode: FireMode | undefined;

export function parseMode(value: string | undefined): FireMode | undefined {
    return value === "off" || value === "turn" || value === "edit"
        ? value
        : undefined;
}

/**
 * User-level file shared with the `habit-hooks --staged` hook;
 * HABIT_HOOKS_MODE_FILE overrides the path.
 */
export function modeFile(): string {
    return (
        process.env.HABIT_HOOKS_MODE_FILE ??
        join(homedir(), ".config", "habit-hooks", "mode")
    );
}

function readPersisted(): FireMode | undefined {
    try {
        return parseMode(readFileSync(modeFile(), "utf8").trim());
    } catch {
        return undefined;
    }
}

/**
 * Precedence: this process's slash command, then HABIT_HOOKS_MODE, then the
 * persisted file, then `turn`.
 */
export function getMode(): FireMode {
    return (
        sessionMode ??
        parseMode(process.env.HABIT_HOOKS_MODE) ??
        readPersisted() ??
        "turn"
    );
}

/** In-memory override for this process only; `undefined` clears it. */
export function setMode(mode: FireMode | undefined): void {
    sessionMode = mode;
}

/**
 * What `/habit-hooks off|turn|edit` does: applies now and survives restarts,
 * other sessions and git hooks.
 */
export function persistMode(mode: FireMode): void {
    sessionMode = undefined;
    mkdirSync(dirname(modeFile()), { recursive: true });
    writeFileSync(modeFile(), `${mode}\n`);
}

const CACHE_TTL_MS = 10 * 60_000;
const CACHE_ENTRIES = 16;
const SUSPEND_MS = 5 * 60_000;
const EDIT_TIMEOUT_MS = 30_000;
const GIT_TIMEOUT_MS = 10_000;
const WALK_BUDGET = 2000;
const SKIP_DIRS = [".git", ".habit-hooks", "target", "node_modules"];

function maxBlocks(): number {
    const n = Number(process.env.HABIT_HOOKS_MAX_BLOCKS);
    return Number.isInteger(n) && n >= 0 ? n : 2;
}

/**
 * Cargo.toml plus at least one real .rs file, found with a bounded
 * breadth-first walk.
 */
export function isRustProject(root: string): boolean {
    if (!existsSync(join(root, "Cargo.toml"))) return false;
    const queue = [root];
    let visited = 0;
    for (let i = 0; i < queue.length; i++) {
        let entries;
        try {
            entries = readdirSync(queue[i], { withFileTypes: true });
        } catch {
            continue;
        }
        for (const entry of entries) {
            // shortcut: a huge tree with a manifest is assumed Rust rather than walked to the end
            if (++visited > WALK_BUDGET) return true;
            if (entry.isFile() && entry.name.endsWith(".rs")) return true;
            if (entry.isDirectory() && !SKIP_DIRS.includes(entry.name))
                queue.push(join(queue[i], entry.name));
        }
    }
    return false;
}

export interface ChangedRust {
    files: string[];
    /** Absolute, symlink-free repository root; `files` are relative to it. */
    root: string;
    /**
     * Changes whenever the set of files or any of their contents (mtime/size)
     * changes.
     */
    fingerprint: string;
}

/** Stable hash of the names and mtime/size of `files` under `root`. */
export function fingerprintFiles(root: string, files: string[]): string {
    const hash = createHash("sha1");
    for (const file of [...files].sort()) {
        let signature = "gone";
        try {
            const stat = statSync(join(root, file));
            signature = `${stat.mtimeMs}:${stat.size}`;
        } catch {
            // deleted files still contribute their name
        }
        hash.update(`${file}\0${signature}\0`);
    }
    return hash.digest("hex");
}

/**
 * Changed or untracked .rs files, or null when there are none or git cannot
 * tell.
 */
export async function changedRust(
    cwd: string,
    signal?: AbortSignal,
): Promise<ChangedRust | null> {
    const [status, top] = await Promise.all([
        runProcess(
            "git",
            [
                "status",
                "--porcelain",
                "-z",
                "--untracked-files=all",
                "--",
                "*.rs",
            ],
            {
                cwd,
                timeoutMs: GIT_TIMEOUT_MS,
                signal,
            },
        ),
        runProcess("git", ["rev-parse", "--show-toplevel"], {
            cwd,
            timeoutMs: GIT_TIMEOUT_MS,
            signal,
        }),
    ]);
    if (status.code !== 0 || top.code !== 0) return null;

    const root = top.stdout.trim();
    const files = parsePorcelain(status.stdout);
    if (files.length === 0) return null;
    return { files, root, fingerprint: fingerprintFiles(root, files) };
}

export type Kind = "clean" | "smells" | "timeout" | "failed";

export function classify(result: HabitHooksResult): Kind {
    if (result.timedOut) return "timeout";
    if (result.exitCode === 0) return "clean";
    return result.exitCode === 1 ? "smells" : "failed";
}

export function statusText(kind: Kind, output: string): string {
    switch (kind) {
        case "clean":
            return "✅ clean";
        case "smells":
            return `⚠️ ${(output.match(/^###\s+[a-z0-9-]+/gm) ?? []).length || 1} smell(s)`;
        case "timeout":
            return "⏱ timed out";
        default:
            return "⚠️ habit-hooks failed";
    }
}

export interface GateOutcome {
    block: boolean;
    feedback?: string;
    status?: string;
}

export interface EditScan {
    status?: string;
    output?: string;
}

interface CachedRun {
    result: HabitHooksResult;
    at: number;
}

/**
 * The files one agent answers for right now: owned by it and still changed in
 * the working tree.
 */
interface OwnedScope {
    root: string;
    /** Repo-root-relative, as in ownership records. */
    files: string[];
    /**
     * The same files relative to the run's cwd, which is what the CLI expects.
     */
    relative: string[];
    fingerprint: string;
}

export interface RunScope {
    /**
     * Reuse key and fingerprint of the files being checked; defaults to every
     * changed Rust file.
     */
    changed?: ChangedRust | null;
    /** Restrict the run to these files (relative to cwd). */
    files?: string[];
    /** Whose automatic runs a timeout pauses. */
    agentId?: string;
}

/**
 * `file` (relative to the repo root) as the run's cwd sees it; `root` is
 * symlink-free, so cwd must be too.
 */
function fromCwd(cwd: string, root: string, file: string): string {
    let base = cwd;
    try {
        base = realpathSync(cwd);
    } catch {
        // a vanished cwd fails the run anyway
    }
    return relative(base, join(root, file));
}

/**
 * Does the CLI report mention this file? Reports name files relative to the
 * run's cwd.
 */
function mentions(
    output: string,
    cwd: string,
    root: string,
    file: string,
): boolean {
    return output.includes(fromCwd(cwd, root, file)) || output.includes(file);
}

/**
 * Decides whether an automatic run is worth starting, and for whom. Runs are
 * scoped to the files the asking agent wrote (see ownership.ts), so another
 * agent's edits never trigger, block or pollute this agent's checks. Runs per
 * repository are serialized (cargo locks its target dir anyway), a finished run
 * is reused until its files change, and failures that are not the agent's fault
 * never block it.
 */
export class FirePolicy {
    private readonly cache = new Map<string, CachedRun>();

    constructor(
        private readonly run: (
            options: RunHabitHooksOptions,
        ) => Promise<HabitHooksResult> = runHabitHooks,
    ) {}

    /** Runs `job` after every earlier job for this repository settled. */
    exclusive<T>(cwd: string, job: () => Promise<T>): Promise<T> {
        return withRepoLock(getRepoRoot(cwd), job);
    }

    /** A new user prompt starts with a fresh gate budget for that agent. */
    resetBlocks(cwd: string, agentId?: string): void {
        getOwnership(resolveRoot(agentId), getRepoRoot(cwd)).blocks = 0;
    }

    /**
     * Working-tree run shared by the stop gate, the `habit_hooks` tool and the
     * slash command.
     */
    async runWorking(
        cwd: string,
        signal?: AbortSignal,
        scope: RunScope = {},
    ): Promise<HabitHooksResult> {
        const changed =
            scope.changed === undefined
                ? await changedRust(cwd, signal)
                : scope.changed;
        const repoRoot = getRepoRoot(cwd);
        return this.exclusive(cwd, async () => {
            const key = changed
                ? `${repoRoot}\0${changed.fingerprint}`
                : undefined;
            const hit = key ? this.cache.get(key) : undefined;
            if (hit && Date.now() - hit.at < CACHE_TTL_MS) return hit.result;

            const result = await this.run({
                scope: "working",
                cwd,
                signal,
                files: scope.files,
            });
            const kind = classify(result);
            if (key && (kind === "clean" || kind === "smells")) {
                this.cache.set(key, { result, at: Date.now() });
                for (const oldest of [...this.cache.keys()].slice(
                    0,
                    Math.max(0, this.cache.size - CACHE_ENTRIES),
                )) {
                    this.cache.delete(oldest);
                }
            }
            if (kind === "timeout")
                getOwnership(
                    resolveRoot(scope.agentId),
                    repoRoot,
                ).suspendedUntil = Date.now() + SUSPEND_MS;
            return result;
        });
    }

    /**
     * Owned files that are still changed; attributions of files that were
     * committed or reverted are dropped.
     */
    private async ownedScope(
        cwd: string,
        signal: AbortSignal | undefined,
        own: AgentOwnership,
    ): Promise<OwnedScope | undefined> {
        const changed = await changedRust(cwd, signal);
        const live = new Set(changed?.files ?? []);
        for (const owned of [own.hard, own.soft]) {
            for (const file of owned.keys())
                if (!live.has(file)) owned.delete(file);
        }
        if (!changed) return undefined;
        // a deleted file cannot be scanned
        const files = changed.files.filter(
            (f) =>
                (own.hard.has(f) || own.soft.has(f)) &&
                existsSync(join(changed.root, f)),
        );
        if (files.length === 0) return undefined;
        return {
            root: changed.root,
            files,
            relative: files.map((f) => fromCwd(cwd, changed.root, f)),
            fingerprint: fingerprintFiles(changed.root, files),
        };
    }

    /**
     * Checks only what `agentId` wrote; undefined when it wrote no Rust file
     * that is still changed.
     */
    async runOwned(
        cwd: string,
        signal?: AbortSignal,
        agentId?: string,
    ): Promise<HabitHooksResult | undefined> {
        const own = getOwnership(resolveRoot(agentId), getRepoRoot(cwd));
        const scope = await this.ownedScope(cwd, signal, own);
        if (!scope) return undefined;
        const changed = {
            files: scope.files,
            fingerprint: scope.fingerprint,
            root: scope.root,
        };
        return this.runWorking(cwd, signal, {
            changed,
            files: scope.relative,
            agentId,
        });
    }

    /**
     * The only automatic check in `turn` mode: called when a session tries to
     * stop. Looks only at files that session wrote and refuses only for smells
     * in files it wrote without another agent's call overlapping.
     */
    async gate(
        cwd: string,
        signal?: AbortSignal,
        agentId?: string,
    ): Promise<GateOutcome> {
        const own = getOwnership(resolveRoot(agentId), getRepoRoot(cwd));
        if (
            (own.mode ?? getMode()) === "off" ||
            !existsSync(join(cwd, "Cargo.toml"))
        )
            return { block: false };
        if (Date.now() < own.suspendedUntil)
            return { block: false, status: "⏱ paused after timeout" };

        const started = Date.now();
        const finish = (
            outcome: GateOutcome,
            action: NonNullable<AgentOwnership["lastGateOutcome"]>["action"],
            smellCount = 0,
        ) => {
            own.lastGateOutcome = {
                action,
                smellCount,
                durationMs: Date.now() - started,
                at: Date.now(),
            };
            return outcome;
        };

        const scope = await this.ownedScope(cwd, signal, own);
        if (!scope) {
            own.blocks = 0;
            return finish({ block: false }, "skipped");
        }

        const result = await this.runWorking(cwd, signal, {
            changed: {
                files: scope.files,
                fingerprint: scope.fingerprint,
                root: scope.root,
            },
            files: scope.relative,
            agentId,
        });
        const kind = classify(result);
        const status = statusText(kind, result.output);
        const smellCount =
            (result.output.match(/^###\s+[a-z0-9-]+/gm) ?? []).length ||
            (kind === "smells" ? 1 : 0);
        if (kind === "clean") {
            own.blocks = 0;
            return finish({ block: false, status }, "clean");
        }
        if (kind !== "smells")
            return finish({ block: false, status }, "skipped");

        const feedback = `⚠️ Habit Hooks Quality Gate: Enforced code smell detected...\n\n${result.output}\n\nPlease refactor before completing.`;
        // Only a smell in a file this agent wrote alone is its to fix; findings
        // about other files (a crate that does not compile because of someone
        // else's edit) and about shared files are reported, never enforced.
        const ownFault = scope.files.some(
            (f) =>
                own.hard.has(f) && mentions(result.output, cwd, scope.root, f),
        );
        if (!ownFault) {
            const incomplete = /Incomplete Run/.test(result.output);
            const advisory = incomplete
                ? "⚠️ incomplete run (build broken outside your files)"
                : `${status} (advisory, shared files)`;
            return finish(
                { block: false, status: advisory, feedback },
                "skipped",
                smellCount,
            );
        }
        if (own.blockedFingerprint === scope.fingerprint) {
            // the agent already got this exact report for these exact files;
            // blocking again can only loop
            return finish(
                { block: false, status: `${status} (already reported)` },
                "repeated",
                smellCount,
            );
        }
        if (own.blocks >= maxBlocks())
            return finish(
                { block: false, status: `${status} (gate released)` },
                "released",
                smellCount,
            );

        own.blocks++;
        own.blockedFingerprint = scope.fingerprint;
        return finish({ block: true, status, feedback }, "blocked", smellCount);
    }

    /**
     * `edit` mode only: scan right after a Rust edit landed, and only a file
     * the agent wrote itself.
     */
    async scanEdited(
        cwd: string,
        file: string,
        signal?: AbortSignal,
        agentId?: string,
    ): Promise<EditScan> {
        const repoRoot = getRepoRoot(cwd);
        const own = getOwnership(resolveRoot(agentId), repoRoot);
        const repoPath = toRepoPath(cwd, repoRoot, file);
        if (!repoPath || !own.hard.has(repoPath)) return {};
        if (
            !existsSync(join(cwd, "Cargo.toml")) ||
            Date.now() < own.suspendedUntil
        )
            return {};

        const result = await this.exclusive(cwd, () =>
            this.run({
                scope: "working",
                files: [file],
                cwd,
                signal,
                timeoutMs: Math.min(defaultTimeoutMs(), EDIT_TIMEOUT_MS),
            }),
        );
        const kind = classify(result);
        if (kind === "timeout") own.suspendedUntil = Date.now() + SUSPEND_MS;
        return {
            status: statusText(kind, result.output),
            output: kind === "smells" ? result.output : undefined,
        };
    }
}

export const policy = new FirePolicy();
