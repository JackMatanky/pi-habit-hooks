import { type FireMode } from "./policy.js";
import type { HabitHooksResult } from "./runner.js";
export declare const SYSTEM_GUIDELINES = "\n## Habit Hooks Quality Coaching Rules\n1. Before completing a turn that modifies Rust source files, run 'habit_hooks' to ensure no code smells were introduced.\n2. If violations are reported, refactor the code according to the outcome-oriented guidance before yielding.\n3. If suppression is strictly necessary (e.g. FFI boundary), always use scoped '#[expect(lint, reason = \"...\")]' with an explicit technical justification. NEVER use '#![allow(...)]' at the crate or module level.\n4. If a smell cannot be immediately refactored, use 'habit_snooze' with an explicit reason. Note that snoozes lapse automatically whenever the affected file is modified.\n";
/** Conservative: unknown, redirected, in-place or `--fix` commands count as writers. */
export declare function isReadOnlyCommand(command: string): boolean;
/**
 * What a tool call can write: nothing, one named Rust file (`directPath`), or
 * anything (found by snapshot diff).
 */
export declare function classifyCall(toolName: string | undefined, input: Record<string, unknown> | undefined): {
    readOnly: boolean;
    directPath?: string;
};
/**
 * Pairs a tool_call with its tool_result; the fallback is only ambiguous for
 * parallel calls of one tool by one agent.
 */
export declare function callKey(event: {
    toolCallId?: string;
    id?: string;
    toolName?: string;
    tool?: string;
} | undefined, agentRoot: string): string;
/**
 * Ownership is only tracked, and only costs anything, in a Cargo project with
 * firing switched on.
 */
export declare function tracking(cwd: string, agentId?: string): boolean;
/** The fire mode that applies to this agent: its own override, else the global one. */
export declare function modeFor(cwd: string, agentId?: string): FireMode;
/**
 * An agent's own files if it wrote any, else the whole working tree plus a hint
 * how to scan everything.
 */
export declare function runWorkingFor(cwd: string, signal: AbortSignal | undefined, agentId: string | undefined): Promise<{
    result: HabitHooksResult;
    advice?: string;
}>;
