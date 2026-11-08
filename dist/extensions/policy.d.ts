import { type HabitHooksResult, type RunHabitHooksOptions } from "./runner.js";
/**
 * When Habit Hooks fires on its own:
 * - off:  never (tools and slash commands still work when invoked explicitly)
 * - turn: once when the main session tries to stop, only for changed Rust files
 *         (default)
 * - edit: `turn`, plus a scan after every Rust edit
 */
export type FireMode = "off" | "turn" | "edit";
export declare function parseMode(value: string | undefined): FireMode | undefined;
/**
 * User-level file shared with the `habit-hooks --staged` hook;
 * HABIT_HOOKS_MODE_FILE overrides the path.
 */
export declare function modeFile(): string;
/**
 * Precedence: this process's slash command, then HABIT_HOOKS_MODE, then the
 * persisted file, then `turn`.
 */
export declare function getMode(): FireMode;
/** In-memory override for this process only; `undefined` clears it. */
export declare function setMode(mode: FireMode | undefined): void;
/**
 * What `/habit-hooks off|turn|edit` does: applies now and survives restarts,
 * other sessions and git hooks.
 */
export declare function persistMode(mode: FireMode): void;
/**
 * Cargo.toml plus at least one real .rs file, found with a bounded
 * breadth-first walk.
 */
export declare function isRustProject(root: string): boolean;
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
export declare function fingerprintFiles(root: string, files: string[]): string;
/**
 * Changed or untracked .rs files, or null when there are none or git cannot
 * tell.
 */
export declare function changedRust(cwd: string, signal?: AbortSignal): Promise<ChangedRust | null>;
export type Kind = "clean" | "smells" | "timeout" | "failed";
export declare function classify(result: HabitHooksResult): Kind;
export declare function statusText(kind: Kind, output: string): string;
export interface GateOutcome {
    block: boolean;
    feedback?: string;
    status?: string;
}
export interface EditScan {
    status?: string;
    output?: string;
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
 * Decides whether an automatic run is worth starting, and for whom. Runs are
 * scoped to the files the asking agent wrote (see ownership.ts), so another
 * agent's edits never trigger, block or pollute this agent's checks. Runs per
 * repository are serialized (cargo locks its target dir anyway), a finished run
 * is reused until its files change, and failures that are not the agent's fault
 * never block it.
 */
export declare class FirePolicy {
    private readonly run;
    private readonly cache;
    constructor(run?: (options: RunHabitHooksOptions) => Promise<HabitHooksResult>);
    /** Runs `job` after every earlier job for this repository settled. */
    exclusive<T>(cwd: string, job: () => Promise<T>): Promise<T>;
    /** A new user prompt starts with a fresh gate budget for that agent. */
    resetBlocks(cwd: string, agentId?: string): void;
    /**
     * Working-tree run shared by the stop gate, the `habit_hooks` tool and the
     * slash command.
     */
    runWorking(cwd: string, signal?: AbortSignal, scope?: RunScope): Promise<HabitHooksResult>;
    /**
     * Owned files that are still changed; attributions of files that were
     * committed or reverted are dropped.
     */
    private ownedScope;
    /**
     * Checks only what `agentId` wrote; undefined when it wrote no Rust file
     * that is still changed.
     */
    runOwned(cwd: string, signal?: AbortSignal, agentId?: string): Promise<HabitHooksResult | undefined>;
    /**
     * The only automatic check in `turn` mode: called when a session tries to
     * stop. Looks only at files that session wrote and refuses only for smells
     * in files it wrote without another agent's call overlapping.
     */
    gate(cwd: string, signal?: AbortSignal, agentId?: string): Promise<GateOutcome>;
    /**
     * `edit` mode only: scan right after a Rust edit landed, and only a file
     * the agent wrote itself.
     */
    scanEdited(cwd: string, file: string, signal?: AbortSignal, agentId?: string): Promise<EditScan>;
}
export declare const policy: FirePolicy;
