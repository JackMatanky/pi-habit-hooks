import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
const __dirname = dirname(fileURLToPath(import.meta.url));
function findPackageRoot(startDir) {
    let current = startDir;
    while (current !== dirname(current)) {
        if (existsSync(join(current, "package.json")) || existsSync(join(current, "Cargo.toml"))) {
            return current;
        }
        current = dirname(current);
    }
    return startDir;
}
const PACKAGE_ROOT = findPackageRoot(__dirname);
export function resolveBinary(name) {
    const exeName = process.platform === "win32" ? `${name}.exe` : name;
    const candidates = [
        join(PACKAGE_ROOT, "target", "release", exeName),
        join(PACKAGE_ROOT, "target", "debug", exeName),
        join(PACKAGE_ROOT, "bin", exeName),
    ];
    for (const candidate of candidates) {
        if (existsSync(candidate)) {
            return candidate;
        }
    }
    // Fallback to name in PATH
    return name;
}
// Same values as timeout(1) / SIGINT / "command not found" so they never collide with habit-hooks' own 0/1/2.
export const EXIT_TIMED_OUT = 124;
export const EXIT_ABORTED = 130;
export const EXIT_SPAWN_FAILED = 127;
export function defaultTimeoutMs() {
    const ms = Number(process.env.HABIT_HOOKS_TIMEOUT_MS);
    return Number.isFinite(ms) && ms > 0 ? ms : 90_000;
}
const liveKills = new Set();
process.once("exit", () => {
    for (const kill of liveKills)
        kill();
});
// Children lead their own process group so one kill also reaps cargo/rustc/sensor grandchildren.
function killTree(child) {
    try {
        if (process.platform !== "win32" && child.pid)
            process.kill(-child.pid, "SIGKILL");
        else
            child.kill("SIGKILL");
    }
    catch {
        // already exited
    }
}
/** Async replacement for spawnSync: never blocks the host event loop, always bounded by timeout/abort. */
export function runProcess(command, args, opts) {
    const { promise, resolve } = Promise.withResolvers();
    const child = spawn(command, args, {
        cwd: opts.cwd,
        env: opts.env,
        detached: process.platform !== "win32",
        stdio: ["pipe", "pipe", "pipe"],
    });
    const cap = opts.maxBytes ?? 4 * 1024 * 1024;
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    let aborted = false;
    let done = false;
    const kill = () => killTree(child);
    liveKills.add(kill);
    const onAbort = () => {
        aborted = true;
        kill();
    };
    const timer = opts.timeoutMs
        ? setTimeout(() => {
            timedOut = true;
            kill();
        }, opts.timeoutMs)
        : undefined;
    const finish = (code, error) => {
        if (done)
            return;
        done = true;
        clearTimeout(timer);
        opts.signal?.removeEventListener("abort", onAbort);
        liveKills.delete(kill);
        resolve({ code, stdout, stderr, timedOut, aborted, error });
    };
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (d) => {
        if (stdout.length < cap)
            stdout += d;
    });
    child.stderr.on("data", (d) => {
        if (stderr.length < cap)
            stderr += d;
    });
    child.stdin.on("error", () => { });
    child.on("error", (err) => finish(null, err));
    child.on("close", (code) => finish(code));
    opts.signal?.addEventListener("abort", onAbort, { once: true });
    if (opts.signal?.aborted)
        onAbort();
    child.stdin.end(opts.input);
    return promise;
}
function toResult(proc, timeoutMs) {
    if (proc.timedOut) {
        return {
            exitCode: EXIT_TIMED_OUT,
            passed: false,
            timedOut: true,
            output: `⏱ habit-hooks timed out after ${Math.round(timeoutMs / 1000)}s and was stopped (a first run compiles clippy dependencies). Re-run it explicitly, or raise HABIT_HOOKS_TIMEOUT_MS.\n`,
        };
    }
    if (proc.aborted) {
        return { exitCode: EXIT_ABORTED, passed: false, aborted: true, output: "habit-hooks was aborted.\n" };
    }
    if (proc.error) {
        return { exitCode: EXIT_SPAWN_FAILED, passed: false, output: `habit-hooks failed to start: ${proc.error.message}\n` };
    }
    const exitCode = proc.code ?? 137;
    return { exitCode, output: proc.stdout + proc.stderr, passed: exitCode === 0 };
}
export async function runHabitHooks(options = {}) {
    const cwd = options.cwd ?? process.cwd();
    const timeoutMs = options.timeoutMs ?? defaultTimeoutMs();
    const deadline = Date.now() + timeoutMs;
    const env = { ...process.env, HABIT_HOOKS_PLUGINS_DIR: join(PACKAGE_ROOT, "plugins") };
    // If auto-fix requested, run cargo clippy --fix first
    if (options.fix) {
        await runProcess("cargo", ["clippy", "--fix", "--allow-dirty", "--allow-no-vcs"], {
            cwd,
            timeoutMs,
            signal: options.signal,
        });
    }
    const args = [];
    switch (options.scope) {
        case "all":
            args.push("--all");
            break;
        case "since":
            if (options.sinceRef) {
                args.push("--since", options.sinceRef);
            }
            break;
        case "working":
        case "staged":
        default:
            args.push("--changed");
            break;
    }
    if (options.files && options.files.length > 0) {
        for (const f of options.files) {
            args.push("--file", f);
        }
    }
    const proc = await runProcess(resolveBinary("habit-hooks"), args, {
        cwd,
        env,
        timeoutMs: Math.max(1, deadline - Date.now()), // 0 would mean "no timeout"
        signal: options.signal,
    });
    return toResult(proc, timeoutMs);
}
export function runHabitSnooze(options) {
    const cwd = options.cwd ?? process.cwd();
    const binary = resolveBinary("habit-snooze");
    const args = [];
    if (options.action === "list") {
        args.push("--list");
    }
    else if (options.action === "add") {
        args.push("--snooze");
        if (options.reason) {
            args.push("--reason", options.reason);
        }
    }
    else if (options.action === "prune") {
        args.push("--prune");
    }
    const proc = spawnSync(binary, args, {
        cwd,
        input: options.inputJson ?? "[]",
        encoding: "utf-8",
        timeout: 15_000,
        env: {
            ...process.env,
            HABIT_HOOKS_PLUGINS_DIR: join(PACKAGE_ROOT, "plugins"),
        },
    });
    const output = (proc.stdout || "") + (proc.stderr || "");
    const exitCode = proc.status ?? 0;
    return { exitCode, output };
}
