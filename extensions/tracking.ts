import type { HabitHooksResult } from "./runner.js";

import { existsSync } from "node:fs";
import { join } from "node:path";

import { getOwnership, getRepoRoot, resolveRoot } from "./ownership.js";
import { getMode, policy, type FireMode } from "./policy.js";

export const SYSTEM_GUIDELINES = `
## Habit Hooks Quality Coaching Rules
1. Before completing a turn that modifies Rust source files, run 'habit_hooks' to ensure no code smells were introduced.
2. If violations are reported, refactor the code according to the outcome-oriented guidance before yielding.
3. If suppression is strictly necessary (e.g. FFI boundary), always use scoped '#[expect(lint, reason = "...")]' with an explicit technical justification. NEVER use '#![allow(...)]' at the crate or module level.
4. If a smell cannot be immediately refactored, use 'habit_snooze' with an explicit reason. Note that snoozes lapse automatically whenever the affected file is modified.
`;

/**
 * Tools that can write Rust files; everything else is treated as read-only and
 * is never snapshot.
 */
const WRITING_TOOLS = new Set([
  "edit",
  "write",
  "ast_edit",
  "bash",
  "eval",
  "python",
]);

const READ_ONLY_COMMAND =
  /^(?:cd\s+\S+|ls|cat|head|tail|wc|pwd|echo|which|rg|grep|stat|diff|sleep|true|git\s+(?:status|diff|log|show|branch|rev-parse|blame|ls-files)|cargo\s+(?:check|build|test|nextest|doc|metadata|tree|bench|clippy|--version))(?:\s|$)/;

/** Conservative: unknown, redirected, in-place or `--fix` commands count as writers. */
export function isReadOnlyCommand(command: string): boolean {
  if (/[>`]|\$\(|\btee\b|--fix\b|\s-i\b/.test(command)) return false;
  return command.split(/&&|\|\||[;|\n]/).every((segment) => {
    const s = segment.trim();
    return s === "" || READ_ONLY_COMMAND.test(s);
  });
}

/**
 * What a tool call can write: nothing, one named Rust file (`directPath`), or
 * anything (found by snapshot diff).
 */
export function classifyCall(
  toolName: string | undefined,
  input: Record<string, unknown> | undefined,
): { readOnly: boolean; directPath?: string } {
  if (!toolName || !WRITING_TOOLS.has(toolName)) return { readOnly: true };
  if (toolName === "bash")
    return { readOnly: isReadOnlyCommand(String(input?.command ?? "")) };
  if (toolName === "eval" || toolName === "python") return { readOnly: false };

  const raw = input?.path ?? input?.filePath ?? input?.file;
  const path = typeof raw === "string" ? raw : undefined;
  if (path && /\.rs$/i.test(path)) return { readOnly: false, directPath: path };
  return { readOnly: path !== undefined && toolName !== "ast_edit" };
}

/**
 * Pairs a tool_call with its tool_result; the fallback is only ambiguous for
 * parallel calls of one tool by one agent.
 */
export function callKey(
  event:
    | { toolCallId?: string; id?: string; toolName?: string; tool?: string }
    | undefined,
  agentRoot: string,
): string {
  return (
    event?.toolCallId ??
    event?.id ??
    `${agentRoot}:${event?.toolName ?? event?.tool ?? ""}`
  );
}

/**
 * Ownership is only tracked, and only costs anything, in a Cargo project with
 * firing switched on.
 */
export function tracking(cwd: string, agentId?: string): boolean {
  return modeFor(cwd, agentId) !== "off" && existsSync(join(cwd, "Cargo.toml"));
}

/** The fire mode that applies to this agent: its own override, else the global one. */
export function modeFor(cwd: string, agentId?: string): FireMode {
  return getOwnership(resolveRoot(agentId), getRepoRoot(cwd)).mode ?? getMode();
}

/**
 * An agent's own files if it wrote any, else the whole working tree plus a hint
 * how to scan everything.
 */
export async function runWorkingFor(
  cwd: string,
  signal: AbortSignal | undefined,
  agentId: string | undefined,
): Promise<{ result: HabitHooksResult; advice?: string }> {
  const owned = await policy.runOwned(cwd, signal, agentId);
  if (owned) return { result: owned };
  return {
    result: await policy.runWorking(cwd, signal, { agentId }),
    advice:
      'No Rust files written by this agent were found, so the whole working tree was checked. Pass scope "all" to scan the full codebase.',
  };
}
