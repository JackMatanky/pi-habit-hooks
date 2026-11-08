import {
    callKey,
    classifyCall,
    runWorkingFor,
    SYSTEM_GUIDELINES,
    tracking,
} from "./tracking.js";
import {
    closeWindow,
    getOwnership,
    getRepoRoot,
    openWindow,
    rebuildFromBranch,
    registerAgent,
    resolveRoot,
    toRepoPath,
    unregisterAgent,
} from "./ownership.js";
import {
    getMode,
    isRustProject,
    modeFile,
    parseMode,
    persistMode,
    policy,
    statusText,
    classify,
    type FireMode,
} from "./policy.js";
import { defaultTimeoutMs, runHabitHooks, runHabitSnooze } from "./runner.js";

const MODE_DESCRIPTIONS: Record<FireMode, string> = {
    off: "never automatically (the tools and /habit-hooks still work when invoked)",
    turn: "once when the session tries to stop, only if Rust files changed",
    edit: "after every Rust edit, and once when the session tries to stop",
};

export interface ToolContent {
    type: "text";
    text: string;
}

export interface ToolResult {
    content: ToolContent[];
    details?: Record<string, unknown>;
    exitCode?: number;
    passed?: boolean;
    guidance?: string;
    output?: string;
}

export interface ExtensionContext {
    cwd?: string;
    signal?: AbortSignal;
    /** OMP only: `kind === "sub"` marks subagent sessions; `parentId` links a subagent to its spawner. */
    agent?: {
        kind?: string;
        id?: string;
        name?: string;
        depth?: number;
        parentId?: string;
    };
    /** Read-only session history; `getBranch()` returns the entries of the current branch. */
    sessionManager?: { getBranch?: () => unknown[] };
    ui?: {
        setStatus?: (
            key: string,
            status: string | { text: string } | undefined,
        ) => void;
        log?: (msg: string) => void;
    };
}

export interface PiAgentHost {
    registerTool: (tool: Record<string, unknown>) => void;
    registerCommand?: (...args: unknown[]) => void;
    injectSystemPrompt?: (prompt: string) => void;
    callTool?: (
        toolName: string,
        args: Record<string, unknown>,
    ) => Promise<unknown>;
    ui?: {
        setStatus?: (
            key: string,
            status: string | { text: string } | undefined,
        ) => void;
        log?: (msg: string) => void;
    };
    on?: (
        event: string,
        handler: (data: HostEvent, ctx?: ExtensionContext) => unknown,
    ) => void;
}

/** The slice of host event payloads read here; Pi and OMP spell some fields differently. */
export interface HostEvent {
    reason?: string;
    cwd?: string;
    ctx?: ExtensionContext;
    systemPrompt?: string | string[];
    isError?: boolean;
    toolCallId?: string;
    id?: string;
    toolName?: string;
    tool?: string;
    input?: Record<string, unknown>;
    params?: Record<string, unknown>;
    args?: Record<string, unknown>;
    details?: Record<string, unknown>;
}

interface LayaAnswer {
    answers?: { is_safe?: { choice?: string; confidence?: number } };
}

export async function validateUnwrapWithLaya(
    finding: {
        smell: string;
        issues?: Array<{ details?: { content?: string } }>;
    },
    pi: PiAgentHost,
): Promise<boolean | undefined> {
    if (
        finding.smell !== "unhandled-unwrap" ||
        typeof pi.callTool !== "function"
    ) {
        return undefined;
    }

    const content = finding.issues?.[0]?.details?.content ?? "";
    try {
        const resp = (await pi.callTool("mcp__laya_predict", {
            state: { text: content },
            questions: {
                is_safe: {
                    type: "choice",
                    instructions:
                        "Is this unwrap safe due to a local invariant or guard?",
                    criteria: {
                        safe: "The unwrap is safely guarded",
                        unsafe: "The unwrap may panic",
                    },
                },
            },
        })) as LayaAnswer | undefined; // MCP result shape is not typed by the host

        if (
            resp?.answers?.is_safe?.choice === "safe" &&
            (resp.answers.is_safe.confidence ?? 0) >= 0.6
        ) {
            return true;
        }
    } catch {
        // Silently continue if Laya MCP is not mounted
    }
    return false;
}

export { classifyCall, isReadOnlyCommand } from "./tracking.js";

export default function (pi: PiAgentHost) {
    // One labelled line under the editor, like other packages: "habit-hooks: turn · ⚠️ 3 smell(s)".
    // Shown only once a Rust project is known; `off` shows just the mode so it is obvious why nothing fires.
    let badgeVisible = false;
    let lastResult: string | undefined;
    const renderBadge = (ctx: ExtensionContext | undefined) => {
        let line: string | undefined;
        if (badgeVisible) {
            const own = getOwnership(
                resolveRoot(ctx?.agent?.id),
                getRepoRoot(ctx?.cwd ?? process.cwd()),
            );
            const mode = own.mode ?? getMode();
            const files = own.hard.size + own.soft.size;
            line = `habit-hooks: ${mode}`;
            if (mode !== "off" && files > 0)
                line += ` · ${files} file${files === 1 ? "" : "s"}`;
            if (mode !== "off" && lastResult) line += ` · ${lastResult}`;
        }
        (ctx?.ui ?? pi.ui)?.setStatus?.("habit-hooks", line);
    };
    const setBadge = (
        ctx: ExtensionContext | undefined,
        text: string | undefined,
    ) => {
        if (text !== undefined) badgeVisible = true;
        lastResult = text;
        renderBadge(ctx);
    };

    // Shared by `execute` (Pi) and `handler` (OMP). Working-tree runs share one cached run per project.
    const runTool = async (
        params: Record<string, unknown>,
        ctx?: ExtensionContext,
        signal: AbortSignal | undefined = ctx?.signal,
    ): Promise<ToolResult> => {
        const cwd = ctx?.cwd ?? process.cwd();
        const scope =
            typeof params.scope === "string"
                ? (params.scope as "working" | "staged" | "all" | "since")
                : "working";
        const sinceRef =
            typeof params.sinceRef === "string" ? params.sinceRef : undefined;
        const files = Array.isArray(params.files)
            ? (params.files as string[])
            : undefined;
        const fix = typeof params.fix === "boolean" ? params.fix : false;

        setBadge(ctx, "⏳ habit-hooks…");
        const { result, advice } =
            scope === "working" && !files?.length && !fix
                ? await runWorkingFor(cwd, signal, ctx?.agent?.id)
                : {
                      result: await policy.exclusive(cwd, () =>
                          runHabitHooks({
                              scope,
                              sinceRef,
                              files,
                              fix,
                              cwd,
                              signal,
                          }),
                      ),
                      advice: undefined,
                  };
        setBadge(ctx, statusText(classify(result), result.output));

        return {
            content: [
                {
                    type: "text",
                    text: advice
                        ? `${result.output}\n${advice}`
                        : result.output,
                },
            ],
            details: {
                exitCode: result.exitCode,
                passed: result.passed,
                timedOut: result.timedOut ?? false,
            },
            exitCode: result.exitCode,
            passed: result.passed,
            guidance: result.output,
        };
    };

    // 1. Register habit_hooks tool
    pi.registerTool({
        name: "habit_hooks",
        label: "Habit Hooks",
        description:
            "Run Habit Hooks Rust code-quality coaching over current working changes, staged changes, or the entire codebase.",
        parameters: {
            type: "object",
            properties: {
                scope: {
                    type: "string",
                    enum: ["working", "staged", "all", "since"],
                    description:
                        "Scope of files to scan. Defaults to 'working'.",
                    default: "working",
                },
                sinceRef: {
                    type: "string",
                    description:
                        "Git ref to compare against when scope is 'since' (e.g. 'main' or 'HEAD~1').",
                },
                files: {
                    type: "array",
                    items: { type: "string" },
                    description: "Specific file paths to scan.",
                },
                fix: {
                    type: "boolean",
                    description:
                        "Attempt safe auto-fix (e.g. cargo clippy --fix) before running checks.",
                    default: false,
                },
            },
        },
        // Standard Pi/OMP execute method
        async execute(
            _toolCallId: string,
            params: Record<string, unknown>,
            signal?: AbortSignal,
            _onUpdate?: unknown,
            ctx?: ExtensionContext,
        ): Promise<ToolResult> {
            return runTool(params, ctx, signal ?? ctx?.signal);
        },
        // Backward-compatible handler method
        async handler(
            args: Record<string, unknown>,
            context?: ExtensionContext,
        ): Promise<ToolResult> {
            return runTool(args, context);
        },
    });

    // 2. Register habit_snooze tool
    pi.registerTool({
        name: "habit_snooze",
        label: "Habit Snooze",
        description:
            "Manage Habit Hooks smell exemptions and snooze entries. Snoozes automatically lapse when the file's Git blob SHA changes.",
        parameters: {
            type: "object",
            properties: {
                action: {
                    type: "string",
                    enum: ["list", "add", "prune"],
                    description:
                        "Action to perform: 'list' active snoozes, 'add' findings to snooze, or 'prune' dead snoozes.",
                    default: "list",
                },
                reason: {
                    type: "string",
                    description:
                        "Documented technical justification when adding a snooze exemption.",
                },
                inputJson: {
                    type: "string",
                    description:
                        "JSON array of findings to snooze when action is 'add'.",
                },
            },
            required: ["action"],
        },
        async execute(
            _toolCallId: string,
            params: Record<string, unknown>,
            _signal?: AbortSignal,
            _onUpdate?: unknown,
            ctx?: ExtensionContext,
        ): Promise<ToolResult> {
            const cwd = ctx?.cwd ?? process.cwd();
            const action =
                typeof params.action === "string"
                    ? (params.action as "list" | "add" | "prune")
                    : "list";
            const reason =
                typeof params.reason === "string" ? params.reason : undefined;
            const inputJson =
                typeof params.inputJson === "string"
                    ? params.inputJson
                    : undefined;

            const result = runHabitSnooze({
                action,
                reason,
                inputJson,
                cwd,
            });

            return {
                content: [{ type: "text", text: result.output }],
                details: { exitCode: result.exitCode },
                exitCode: result.exitCode,
                output: result.output,
            };
        },
        async handler(
            args: Record<string, unknown>,
            context?: ExtensionContext,
        ): Promise<ToolResult> {
            const cwd = context?.cwd ?? process.cwd();
            const action =
                typeof args.action === "string"
                    ? (args.action as "list" | "add" | "prune")
                    : "list";
            const reason =
                typeof args.reason === "string" ? args.reason : undefined;
            const inputJson =
                typeof args.inputJson === "string" ? args.inputJson : undefined;

            const result = runHabitSnooze({
                action,
                reason,
                inputJson,
                cwd,
            });

            return {
                content: [{ type: "text", text: result.output }],
                details: { exitCode: result.exitCode },
                exitCode: result.exitCode,
                output: result.output,
            };
        },
    });

    // 3. Register Slash Commands (supports both Pi string-name signature and object signature)
    if (typeof pi.registerCommand === "function") {
        const registerCmd = (
            name: string,
            description: string,
            handler: (
                args: string,
                context?: ExtensionContext,
            ) => Promise<void>,
        ) => {
            try {
                if (typeof pi.registerCommand === "function") {
                    if (pi.registerCommand.length === 1) {
                        pi.registerCommand({ name, description, handler });
                    } else if (pi.registerCommand.length === 3) {
                        pi.registerCommand(name, description, handler);
                    } else {
                        pi.registerCommand(name, { description, handler });
                    }
                }
            } catch {
                try {
                    pi.registerCommand!({ name, description, handler });
                } catch {
                    // Ignore if host doesn't support command registration
                }
            }
        };

        registerCmd(
            "habit-hooks",
            "Run Habit Hooks, or choose when it fires: /habit-hooks [--all | status | off | turn | edit]",
            async (args, context) => {
                const word = args.trim().split(/\s+/)[0];
                const mode = parseMode(word);
                if (mode) {
                    persistMode(mode);
                    renderBadge(context);
                    context?.ui?.log?.(
                        `Habit Hooks now fires ${MODE_DESCRIPTIONS[mode]}. Saved to ${modeFile()}; also applies to the pre-commit hook.`,
                    );
                    return;
                }
                if (word === "status") {
                    const cwd = context?.cwd ?? process.cwd();
                    const own = getOwnership(
                        resolveRoot(context?.agent?.id),
                        getRepoRoot(cwd),
                    );
                    const list = (files: Iterable<string>) =>
                        [...files].join(", ") || "none";
                    const last = own.lastGateOutcome;
                    context?.ui?.log?.(
                        [
                            `Habit Hooks fires ${MODE_DESCRIPTIONS[own.mode ?? getMode()]}. Run timeout ${Math.round(defaultTimeoutMs() / 1000)}s (HABIT_HOOKS_TIMEOUT_MS).`,
                            `Rust files this agent wrote (checked and enforced): ${list(own.hard.keys())}`,
                            `Rust files shared with another agent's overlapping tool call (checked, never enforced): ${list(own.soft.keys())}`,
                            last
                                ? `Last gate: ${last.action}, ${last.smellCount} smell(s), ${last.durationMs}ms, ${new Date(last.at).toLocaleTimeString()}`
                                : "Last gate: none yet",
                        ].join("\n"),
                    );
                    return;
                }

                const cwd = context?.cwd ?? process.cwd();
                if (args.includes("--all")) {
                    const result = await policy.exclusive(cwd, () =>
                        runHabitHooks({
                            scope: "all",
                            cwd,
                            signal: context?.signal,
                        }),
                    );
                    context?.ui?.log?.(result.output);
                    return;
                }
                const { result, advice } = await runWorkingFor(
                    cwd,
                    context?.signal,
                    context?.agent?.id,
                );
                context?.ui?.log?.(
                    advice ? `${result.output}\n${advice}` : result.output,
                );
            },
        );

        registerCmd(
            "snooze",
            "Manage Habit Hooks snoozed exemptions",
            async (args, context) => {
                const cwd = context?.cwd ?? process.cwd();
                const action = args.includes("--prune") ? "prune" : "list";
                const result = runHabitSnooze({ action, cwd });
                context?.ui?.log?.(result.output);
            },
        );
    }

    if (typeof pi.on === "function") {
        pi.on("session_start", (_event, ctx) => {
            const cwd = ctx?.cwd ?? process.cwd();
            const agent = ctx?.agent;
            if (agent?.id && agent.parentId)
                registerAgent(agent.id, agent.parentId);
            badgeVisible = isRustProject(cwd);
            // A root session rebuilds what it owns from its own branch, so ownership survives resume and rewind.
            // A subagent's files belong to its root, whose branch it must not overwrite.
            if (badgeVisible && !agent?.parentId) {
                const branch = ctx?.sessionManager?.getBranch?.();
                if (Array.isArray(branch))
                    rebuildFromBranch(
                        resolveRoot(agent?.id),
                        getRepoRoot(cwd),
                        branch,
                    );
            }
            lastResult = undefined;
            renderBadge(ctx);
        });

        pi.on("session_shutdown", (_event, ctx) => {
            unregisterAgent(ctx?.agent?.id ?? "main");
        });

        // 4. Coaching instructions: only for the main agent, only in a Rust project, never when firing is off.
        pi.on("before_agent_start", (event, ctx?: ExtensionContext) => {
            if (
                event?.reason === "revive" ||
                ctx?.agent?.kind === "sub" ||
                getMode() === "off"
            )
                return;
            const cwd = ctx?.cwd ?? event?.cwd ?? process.cwd();
            policy.resetBlocks(cwd, ctx?.agent?.id); // every user prompt gets a fresh gate budget
            if (!isRustProject(cwd)) return;

            const prompt = event.systemPrompt;
            if (Array.isArray(prompt)) {
                if (
                    !prompt.some((b) =>
                        b.includes("Habit Hooks Quality Coaching Rules"),
                    )
                ) {
                    return { systemPrompt: [...prompt, SYSTEM_GUIDELINES] };
                }
            } else if (typeof prompt === "string") {
                if (!prompt.includes("Habit Hooks Quality Coaching Rules")) {
                    return {
                        systemPrompt: `${prompt}\n\n${SYSTEM_GUIDELINES}`,
                    };
                }
            }
        });

        // 5. Attribution, part one: a tool call opens a window so what it writes can be told from other agents' writes.
        // Nothing is checked here: tool_call is pre-exec, so a scan would read the file before the edit lands.
        pi.on("tool_call", async (event, ctx) => {
            const cwd = ctx?.cwd ?? process.cwd();
            if (!tracking(cwd)) return;
            const toolName = event?.toolName ?? event?.tool;
            const call = classifyCall(
                toolName,
                event?.input ?? event?.params ?? event?.args,
            );
            const agentRoot = resolveRoot(ctx?.agent?.id);
            await openWindow(
                agentRoot,
                getRepoRoot(cwd),
                callKey(event, agentRoot),
                call.readOnly,
                !call.directPath,
            );
        });

        // 6. Attribution, part two, then (`edit` mode only) the scan. tool_result is post-exec, so the scan sees the edited file.
        pi.on("tool_result", async (event, ctx) => {
            const cwd = ctx?.cwd ?? process.cwd();
            const agentId = ctx?.agent?.id;
            const toolName = event?.toolName ?? event?.tool;
            const input = event?.input ?? event?.params ?? event?.args;

            let details: Record<string, unknown> | undefined;
            if (tracking(cwd)) {
                const repoRoot = getRepoRoot(cwd);
                const agentRoot = resolveRoot(agentId);
                const { directPath } = classifyCall(toolName, input);
                // a failed edit wrote nothing; a failed shell command may still have, which the snapshot diff catches
                const direct =
                    directPath && !event?.isError
                        ? toRepoPath(cwd, repoRoot, directPath)
                        : undefined;
                const owned = await closeWindow(
                    agentRoot,
                    repoRoot,
                    callKey(event, agentRoot),
                    direct,
                );
                if (owned.hard.length || owned.soft.length) {
                    // persisted on the session branch, which is how ownership survives a resume
                    details = { ...event?.details, habitHooks: owned };
                    if (badgeVisible) renderBadge(ctx);
                }
            }

            let additionalContext: string | undefined;
            const file = input?.path ?? input?.filePath ?? input?.file;
            if (
                getMode() === "edit" &&
                ctx?.agent?.kind !== "sub" &&
                !event?.isError &&
                (toolName === "edit" || toolName === "write") &&
                typeof file === "string" &&
                /\.rs$/i.test(file)
            ) {
                const scan = await policy.scanEdited(
                    cwd,
                    file,
                    ctx?.signal,
                    agentId,
                );
                if (scan.status !== undefined) setBadge(ctx, scan.status);
                if (scan.output)
                    additionalContext = `⚠️ Habit Hooks coaching feedback for ${file}:\n\n${scan.output}`;
            }

            if (details || additionalContext)
                return {
                    ...(details && { details }),
                    ...(additionalContext && { additionalContext }),
                };
        });

        // 7. The gate. session_stop is the main-session stop hook (awaited, never fired for subagents).
        // turn_end/agent_end are per-turn notifications whose results the hosts ignore, so nothing runs there.
        pi.on("session_stop", async (data, ctxArg) => {
            const ctx = ctxArg ?? data?.ctx;
            const cwd = ctx?.cwd ?? data?.cwd ?? process.cwd();
            const outcome = await policy.gate(cwd, ctx?.signal, ctx?.agent?.id);
            setBadge(ctx, outcome.status);
            if (!outcome.block) return { interrupt: false };
            return {
                interrupt: true,
                decision: "block",
                reason: outcome.feedback,
                message: outcome.feedback,
            };
        });
    }
}
