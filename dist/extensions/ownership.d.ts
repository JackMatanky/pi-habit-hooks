import type { FireMode } from "./policy.js";
/**
 * Who wrote which Rust file. Habit Hooks only ever checks, reports and blocks on files the calling agent wrote:
 * - hard: this agent changed the file and no other agent's tool call overlapped in time, so a smell is its own;
 * - soft: another agent's tool call overlapped, so attribution is uncertain and findings are advisory only.
 * Subagents roll up into their root agent, so the main session answers for what its subagents wrote.
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
    /** Fingerprint of the owned files the gate last refused for; the same files are never refused twice. */
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
    /** Read-only windows cannot have written anything, so they never make another agent's attribution soft. */
    readOnly: boolean;
    preSnap?: Map<string, FileSignature>;
}
export declare function registerAgent(agentId: string, parentId?: string): void;
/** A root agent's ownership dies with its session; a subagent's files already live under its root. */
export declare function unregisterAgent(agentId: string): void;
/** The root agent a (sub)agent answers to; unknown or absent ids are the main agent. */
export declare function resolveRoot(agentId?: string): string;
export declare function getRepoRoot(cwd: string): string;
export declare function getOwnership(agentRoot: string, repoRoot: string): AgentOwnership;
/** A path as a tool received it (absolute, or relative to cwd) to the repo-root-relative form `git status` uses. */
export declare function toRepoPath(cwd: string, repoRoot: string, file: string): string | undefined;
export declare function fileSignature(repoRoot: string, file: string): FileSignature;
/** Paths from `git status --porcelain -z`; a rename or copy is followed by an entry holding the old path. */
export declare function parsePorcelain(stdout: string): string[];
/** Runs `job` after every earlier job for this repository settled (cargo locks its target dir anyway). */
export declare function withRepoLock<T>(repoRoot: string, job: () => Promise<T>): Promise<T>;
/**
 * Marks the start of a tool call. Read-only calls are recorded but never snapshot and never cause overlap.
 * `snapshot` is false when the call names the one file it writes, so no snapshot is needed to find it.
 */
export declare function openWindow(agentRoot: string, repoRoot: string, toolCallId: string, isReadOnly: boolean, snapshot?: boolean): Promise<void>;
/**
 * Ends a tool call and attributes the Rust files it wrote: the named `directFile`, or else every file whose
 * signature changed since the opening snapshot. Files written while another agent's call overlapped are soft.
 */
export declare function closeWindow(agentRoot: string, repoRoot: string, toolCallId: string, directFile?: string): Promise<{
    hard: string[];
    soft: string[];
}>;
/** Replays the `details.habitHooks` stamps of a session branch, so ownership survives resume and rewind. */
export declare function rebuildFromBranch(agentRoot: string, repoRoot: string, branchEntries: unknown[]): void;
