export declare function resolveBinary(name: "habit-hooks" | "habit-snooze" | "habit-sensors" | "habit-mapper" | "habit-judge"): string;
export interface RunHabitHooksOptions {
    scope?: "working" | "staged" | "all" | "since";
    sinceRef?: string;
    files?: string[];
    fix?: boolean;
    cwd?: string;
    /** Wall-clock limit for the whole run; defaults to HABIT_HOOKS_TIMEOUT_MS or 90s. */
    timeoutMs?: number;
    signal?: AbortSignal;
}
export interface HabitHooksResult {
    exitCode: number;
    output: string;
    passed: boolean;
    timedOut?: boolean;
    aborted?: boolean;
}
export declare const EXIT_TIMED_OUT = 124;
export declare const EXIT_ABORTED = 130;
export declare const EXIT_SPAWN_FAILED = 127;
export declare function defaultTimeoutMs(): number;
export interface ProcessResult {
    code: number | null;
    stdout: string;
    stderr: string;
    timedOut: boolean;
    aborted: boolean;
    error?: Error;
}
/** Async replacement for spawnSync: never blocks the host event loop, always bounded by timeout/abort. */
export declare function runProcess(command: string, args: string[], opts: {
    cwd: string;
    env?: NodeJS.ProcessEnv;
    input?: string;
    timeoutMs?: number;
    signal?: AbortSignal;
    maxBytes?: number;
}): Promise<ProcessResult>;
export declare function runHabitHooks(options?: RunHabitHooksOptions): Promise<HabitHooksResult>;
export interface RunHabitSnoozeOptions {
    action: "list" | "add" | "prune";
    reason?: string;
    cwd?: string;
    inputJson?: string;
}
export declare function runHabitSnooze(options: RunHabitSnoozeOptions): {
    exitCode: number;
    output: string;
};
