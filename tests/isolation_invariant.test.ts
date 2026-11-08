import type {
  HabitHooksResult,
  RunHabitHooksOptions,
} from "../extensions/runner.js";

import assert from "node:assert/strict";
import { rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { test } from "vitest";

import registerExtension from "../extensions/index.js";
import { FirePolicy } from "../extensions/policy.js";
import { initRepo, isolateModeFile } from "./helpers.js";

isolateModeFile();

type Handler = (
  event: Record<string, unknown>,
  ctx: Record<string, unknown>,
) => Promise<unknown>;

async function withRepo(body: (dir: string) => Promise<void>): Promise<void> {
  const dir = initRepo("habit-hooks-iso-", {
    "Cargo.toml": '[package]\nname = "t"\n',
    "src/.gitkeep": "",
  });
  try {
    await body(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function extension() {
  const handlers: Record<string, Handler[]> = {};
  registerExtension({
    registerTool: () => {},
    on: (event: string, handler: Function) => {
      (handlers[event] ??= []).push(handler as Handler);
    },
  });
  /** One edit tool call by `agent`, from tool_call through tool_result, exactly as the host fires them. */
  const edit = async (
    dir: string,
    agent: string,
    file: string,
    callId: string,
  ) => {
    const ctx = { cwd: dir, agent: { kind: "main", id: agent } };
    const event = {
      toolName: "edit",
      toolCallId: callId,
      input: { path: file },
    };
    await handlers["tool_call"][0](event, ctx);
    writeFileSync(join(dir, file), `pub fn ${agent}() {}\n// ${callId}\n`);
    return handlers["tool_result"][0](event, ctx);
  };
  return { handlers, edit };
}

/** The CLI stand-in: a smell for anything involving src/a.rs, clean otherwise; records what each run was scoped to. */
function fakeCli() {
  const scopes: string[] = [];
  const run = async (
    options: RunHabitHooksOptions,
  ): Promise<HabitHooksResult> => {
    scopes.push((options.files ?? ["<whole tree>"]).join());
    return options.files?.includes("src/a.rs")
      ? {
          exitCode: 1,
          passed: false,
          output:
            "### too-many-parameters\n- src/a.rs:1: too many parameters\n",
          report: {
            enforcedFiles: ["src/a.rs"],
            incompleteRun: false,
            smellCount: 1,
          },
        }
      : { exitCode: 0, passed: true, output: "" };
  };
  return { run, scopes };
}

test("two agents editing one repo: each stop gate sees only its own file, and only the guilty agent is refused", () =>
  withRepo(async (dir) => {
    const { edit } = extension();
    const cli = fakeCli();
    const policy = new FirePolicy(cli.run);

    const stampA = await edit(dir, "agent-a", "src/a.rs", "call-a");
    const stampB = await edit(dir, "agent-b", "src/b.rs", "call-b");
    assert.deepEqual(stampA, {
      details: { habitHooks: { hard: ["src/a.rs"], soft: [] } },
    });
    assert.deepEqual(stampB, {
      details: { habitHooks: { hard: ["src/b.rs"], soft: [] } },
    });

    const [b, a] = await Promise.all([
      policy.gate(dir, undefined, "agent-b"),
      policy.gate(dir, undefined, "agent-a"),
    ]);
    assert.equal(b.block, false);
    assert.equal(b.status, "✅ clean");
    assert.equal(a.block, true);
    assert.match(a.feedback ?? "", /src\/a\.rs/);
    assert.deepEqual(cli.scopes.slice().sort(), ["src/a.rs", "src/b.rs"]);
  }));

test("overlapping tool calls make the attribution soft, so neither agent is refused for the other's smell", () =>
  withRepo(async (dir) => {
    const { handlers } = extension();
    const cli = fakeCli();
    const policy = new FirePolicy(cli.run);
    const call = (agent: string, callId: string) => ({
      event: {
        toolName: "bash",
        toolCallId: callId,
        input: { command: "cargo fmt" },
      },
      ctx: { cwd: dir, agent: { kind: "main", id: agent } },
    });

    // both agents run a possibly-writing shell command at the same time
    const a = call("agent-a", "sh-a");
    const b = call("agent-b", "sh-b");
    await handlers["tool_call"][0](a.event, a.ctx);
    await handlers["tool_call"][0](b.event, b.ctx);
    writeFileSync(join(dir, "src/a.rs"), "pub fn bad() {}\n");
    await handlers["tool_result"][0](a.event, a.ctx);
    await handlers["tool_result"][0](b.event, b.ctx);

    const outcome = await policy.gate(dir, undefined, "agent-a");
    assert.equal(outcome.block, false);
    assert.match(outcome.status ?? "", /advisory/);
  }));
