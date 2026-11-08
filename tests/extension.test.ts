import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { test } from "vitest";

import registerExtension, {
  validateUnwrapWithLaya,
} from "../extensions/index.js";
import { getMode, modeFile, setMode } from "../extensions/policy.js";
import {
  resolveBinary,
  runHabitHooks,
  runHabitSnooze,
} from "../extensions/runner.js";
import { isolateModeFile } from "./helpers.js";

isolateModeFile();

type Handlers = Record<string, Function[]>;
type Status = string | { text: string } | undefined;

function host(extra: Record<string, unknown> = {}) {
  const handlers: Handlers = {};
  const statuses: Status[] = [];
  const pi = {
    registerTool: () => {},
    ui: {
      setStatus: (_key: string, value: Status) => {
        statuses.push(value);
      },
    },
    on: (event: string, handler: Function) => {
      handlers[event] = handlers[event] || [];
      handlers[event].push(handler);
    },
    ...extra,
  };
  return { pi, handlers, statuses };
}

function tempProject(files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), "habit-hooks-ext-"));
  for (const [name, body] of Object.entries(files)) {
    mkdirSync(join(dir, name, ".."), { recursive: true });
    writeFileSync(join(dir, name), body);
  }
  return dir;
}

test("resolveBinary resolves built CLI binaries", () => {
  const binaryHooks = resolveBinary("habit-hooks");
  assert.ok(binaryHooks.includes("habit-hooks"));
  const binaryJudge = resolveBinary("habit-judge");
  assert.ok(binaryJudge.includes("habit-judge"));
});

test("runHabitHooks runs on workspace and returns clean status or coaching", async () => {
  const result = await runHabitHooks({ scope: "working" });
  assert.equal(typeof result.exitCode, "number");
  assert.equal(typeof result.passed, "boolean");
  assert.ok(result.output.length > 0);
  // the CLI's structured twin of the rendered text, which the gate uses to decide blame
  assert.equal(typeof result.report?.smellCount, "number");
  assert.ok(Array.isArray(result.report?.enforcedFiles));
});

test("runHabitHooks stops a run that exceeds even a very small timeout", async () => {
  // a whole-repo scan cannot finish in 1ms; a floor on the timeout (regression) would let it run to completion
  const result = await runHabitHooks({ scope: "all", timeoutMs: 1 });
  assert.equal(result.timedOut, true);
  assert.equal(result.exitCode, 124);
  assert.match(result.output, /timed out/);
});

test("runHabitSnooze supports list action", () => {
  const result = runHabitSnooze({ action: "list" });
  assert.equal(typeof result.exitCode, "number");
  assert.ok(result.output.length >= 0);
});

test("registers tools and commands with host", () => {
  const registeredTools: Record<string, unknown>[] = [];
  const registeredCommands: string[] = [];
  let promptInjected = false;

  registerExtension({
    registerTool: (tool: Record<string, unknown>) => {
      registeredTools.push(tool);
    },
    registerCommand: (...args: unknown[]) => {
      if (typeof args[0] === "string") {
        registeredCommands.push(args[0]);
      } else if (args[0] && typeof args[0] === "object" && "name" in args[0]) {
        registeredCommands.push(String(args[0].name));
      }
    },
    injectSystemPrompt: (_prompt: string) => {
      promptInjected = true;
    },
  });

  assert.deepEqual(
    registeredTools.map((t) => t.name),
    ["habit_hooks", "habit_snooze"],
  );
  assert.deepEqual(registeredCommands, ["habit-hooks", "snooze"]);
  assert.equal(promptInjected, false);

  // Verify both execute and handler exist on registered tools
  for (const t of registeredTools) {
    assert.equal(typeof t.execute, "function");
    assert.equal(typeof t.handler, "function");
  }
});

test("only the main-session stop hook is a gate; per-turn and per-call events never run checks", () => {
  const { pi, handlers } = host();
  registerExtension(pi);

  assert.equal(handlers["session_stop"]?.length, 1);
  // turn_end fires after every model turn and hosts ignore its result; before_yield does not exist.
  assert.equal(handlers["turn_end"], undefined);
  assert.equal(handlers["before_yield"], undefined);
  // tool_call is pre-exec: it only opens an attribution window, so a scan there would read the file before the edit lands.
  assert.equal(handlers["tool_call"]?.length, 1);
});

test("no automatic firing without Rust sources, in subagents, or when switched off", async () => {
  const empty = tempProject({
    "Cargo.toml": "[workspace]\n",
    "README.md": "no Rust sources here\n",
    "target/ignored.rs": "not project source\n",
  });
  const rust = tempProject({
    "Cargo.toml": '[package]\nname = "t"\n',
    "src/main.rs": "fn main() {}\n",
  });
  try {
    const { pi, handlers, statuses } = host({
      injectSystemPrompt: () => {
        throw new Error("unconditional prompt injection");
      },
    });
    registerExtension(pi);
    const start = handlers["before_agent_start"][0];

    assert.equal(start({ systemPrompt: ["Base"] }, { cwd: empty }), undefined);
    assert.deepEqual(await handlers["session_stop"][0]({}, { cwd: empty }), {
      interrupt: false,
    });
    // the handler clears its badge (undefined) so a stale warning can't linger, but never shows anything here
    assert.deepEqual(
      statuses.filter((s) => s !== undefined),
      [],
    );

    const injected = start({ systemPrompt: ["Base"] }, { cwd: rust });
    assert.ok(
      injected.systemPrompt.some((p: string) =>
        p.includes("Habit Hooks Quality Coaching Rules"),
      ),
    );

    assert.equal(
      start({ systemPrompt: ["Base"] }, { cwd: rust, agent: { kind: "sub" } }),
      undefined,
    );
    assert.equal(
      start({ systemPrompt: ["Base"], reason: "revive" }, { cwd: rust }),
      undefined,
    );

    setMode("off");
    assert.equal(start({ systemPrompt: ["Base"] }, { cwd: rust }), undefined);
  } finally {
    setMode(undefined);
    rmSync(empty, { recursive: true, force: true });
    rmSync(rust, { recursive: true, force: true });
  }
});

test("tool_result attributes Rust writes to the agent and scans only in edit mode, only for Rust edits, never in subagents", async () => {
  const rust = tempProject({
    "Cargo.toml": '[package]\nname = "t"\n',
    "src/lib.rs": "pub fn a() {}\n",
  });
  try {
    const { pi, handlers, statuses } = host();
    registerExtension(pi);
    const onResult = handlers["tool_result"][0];
    const rustEdit = { toolName: "edit", input: { path: "src/lib.rs" } };
    const stamp = (hard: string[], soft: string[] = []) => ({
      details: { habitHooks: { hard, soft } },
    });

    // default `turn` mode: an edit is recorded on the tool result, and nothing fires
    assert.deepEqual(
      await onResult(rustEdit, { cwd: rust, agent: { id: "agent-x" } }),
      stamp(["src/lib.rs"]),
    );

    setMode("edit");
    assert.equal(
      await onResult(
        { toolName: "edit", input: { path: "README.md" } },
        { cwd: rust },
      ),
      undefined,
    );
    assert.equal(
      await onResult({ ...rustEdit, isError: true }, { cwd: rust }),
      undefined,
    );
    // a subagent's write is attributed too (to itself here: no parent), but never scanned
    assert.deepEqual(
      await onResult(rustEdit, {
        cwd: rust,
        agent: { kind: "sub", id: "agent-y" },
      }),
      stamp(["src/lib.rs"]),
    );
    assert.deepEqual(statuses, []);

    setMode("off");
    assert.equal(
      await onResult(rustEdit, { cwd: rust, agent: { id: "agent-z" } }),
      undefined,
    );
  } finally {
    setMode(undefined);
    rmSync(rust, { recursive: true, force: true });
  }
});

test("the badge counts the files this agent wrote, and /habit-hooks status lists them by ownership", async () => {
  const rust = tempProject({
    "Cargo.toml": '[package]\nname = "t"\n',
    "src/lib.rs": "pub fn a() {}\n",
    "src/b.rs": "pub fn b() {}\n",
  });
  try {
    const commands: Record<string, { handler: Function }> = {};
    const logs: string[] = [];
    const { pi, handlers, statuses } = host({
      registerCommand: (cmd: { name: string; handler: Function }) => {
        commands[cmd.name] = cmd;
      },
    });
    registerExtension(pi);
    const ui = {
      log: (m: string) => logs.push(m),
      setStatus: (_key: string, value: Status) => void statuses.push(value),
    };
    const ctx = { cwd: rust, agent: { id: "badge-agent" }, ui };

    handlers["session_start"][0]({}, ctx);
    assert.equal(statuses.at(-1), "habit-hooks: turn");

    await handlers["tool_result"][0](
      { toolName: "write", input: { path: "src/lib.rs" } },
      ctx,
    );
    assert.equal(statuses.at(-1), "habit-hooks: turn · 1 file");
    await handlers["tool_result"][0](
      { toolName: "write", input: { path: "src/b.rs" } },
      ctx,
    );
    assert.equal(statuses.at(-1), "habit-hooks: turn · 2 files");

    // another agent sees none of it
    handlers["session_start"][0](
      {},
      { cwd: rust, agent: { id: "someone-else" }, ui: ctx.ui },
    );
    assert.equal(statuses.at(-1), "habit-hooks: turn");

    await commands["habit-hooks"].handler("status", ctx);
    assert.match(
      logs[0],
      /this agent wrote \(checked and enforced\): src\/lib\.rs, src\/b\.rs/,
    );
    assert.match(logs[0], /never enforced\): none/);
    assert.match(logs[0], /Last gate: none yet/);
  } finally {
    setMode(undefined);
    rmSync(rust, { recursive: true, force: true });
  }
});
test("/habit-hooks <mode> --agent overrides one agent only, and `default --agent` undoes it", async () => {
  const rust = tempProject({
    "Cargo.toml": '[package]\nname = "t"\n',
    "src/lib.rs": "pub fn a() {}\n",
  });
  try {
    const commands: Record<string, { handler: Function }> = {};
    const logs: string[] = [];
    const { pi, handlers } = host({
      registerCommand: (cmd: { name: string; handler: Function }) => {
        commands[cmd.name] = cmd;
      },
    });
    registerExtension(pi);
    const ui = { log: (m: string) => logs.push(m) };
    const quiet = { cwd: rust, agent: { id: "quiet-agent" }, ui };
    const other = { cwd: rust, agent: { id: "other-agent" }, ui };
    const edit = { toolName: "write", input: { path: "src/lib.rs" } };

    await commands["habit-hooks"].handler("off --agent", quiet);
    assert.match(logs[0], /this agent only/);

    // the overridden agent is not tracked; another agent still is, and the global mode is untouched
    assert.equal(await handlers["tool_result"][0](edit, quiet), undefined);
    assert.ok(await handlers["tool_result"][0](edit, other));
    assert.equal(getMode(), "turn");

    await commands["habit-hooks"].handler("status", quiet);
    assert.match(logs.at(-1)!, /never automatically.*\(this agent only\)/);

    await commands["habit-hooks"].handler("default --agent", quiet);
    assert.ok(await handlers["tool_result"][0](edit, quiet));
  } finally {
    setMode(undefined);
    rmSync(rust, { recursive: true, force: true });
  }
});

test("lifecycle: validateUnwrapWithLaya invokes Laya judge if available", async () => {
  let calledTool = "";
  const mockPi = {
    registerTool: () => {},
    callTool: async (toolName: string, _args: Record<string, unknown>) => {
      calledTool = toolName;
      return {
        answers: {
          is_safe: {
            choice: "safe",
            confidence: 0.85,
          },
        },
      };
    },
  };

  const finding = {
    smell: "unhandled-unwrap",
    issues: [{ details: { content: "if !v.is_empty() { v.unwrap(); }" } }],
  };

  const isSafe = await validateUnwrapWithLaya(finding, mockPi);
  assert.equal(calledTool, "mcp__laya_predict");
  assert.equal(isSafe, true);
});

test("compatibility: works with Pi host environment", async () => {
  const tools: Record<string, any> = {};
  const commands: Record<string, any> = {};
  const { pi, handlers } = host({
    registerTool: (tool: any) => {
      tools[tool.name] = tool;
    },
    registerCommand: (name: string, desc: string, handler: Function) => {
      commands[name] = { desc, handler };
    },
  });

  registerExtension(pi);

  // 1. Tool registration
  assert.ok(tools["habit_hooks"]);
  assert.ok(tools["habit_snooze"]);

  // 2. Pi execute calling convention
  const result = await tools["habit_hooks"].execute(
    "tool-call-1",
    { scope: "working" },
    undefined,
    undefined,
    {
      cwd: process.cwd(),
    },
  );
  assert.equal(typeof result.exitCode, "number");
  assert.equal(typeof result.passed, "boolean");
  assert.ok(Array.isArray(result.content));

  // 3. Command registration
  assert.ok(commands["habit-hooks"]);
  assert.ok(commands["snooze"]);

  // 4. Prompt injection via before_agent_start (this repo is a Rust project)
  const promptEvt = handlers["before_agent_start"][0]({
    systemPrompt: ["Base prompt"],
  });
  assert.ok(
    Array.isArray(promptEvt.systemPrompt) &&
      promptEvt.systemPrompt.some((p: string) =>
        p.includes("Habit Hooks Quality Coaching Rules"),
      ),
  );
});

test("compatibility: works with Oh My Pi (OMP) host environment", async () => {
  const tools: Record<string, any> = {};
  const commands: Record<string, any> = {};
  const logs: string[] = [];
  const { pi, handlers } = host({
    registerTool: (tool: any) => {
      tools[tool.name] = tool;
    },
    registerCommand: (cmdObj: {
      name: string;
      description: string;
      handler: Function;
    }) => {
      commands[cmdObj.name] = cmdObj;
    },
  });

  registerExtension(pi);

  // 1. Tool registration + OMP handler calling convention
  assert.ok(tools["habit_hooks"]);
  assert.ok(tools["habit_snooze"]);
  const result = await tools["habit_hooks"].handler(
    { scope: "working" },
    { cwd: process.cwd() },
  );
  assert.equal(typeof result.exitCode, "number");
  assert.equal(typeof result.passed, "boolean");
  assert.ok(Array.isArray(result.content));

  // 2. Direct command object registration, including the fire-mode controls
  assert.ok(commands["habit-hooks"]);
  assert.ok(commands["snooze"]);
  try {
    await commands["habit-hooks"].handler("off", {
      ui: { log: (m: string) => logs.push(m) },
    });
    await commands["habit-hooks"].handler("status", {
      ui: { log: (m: string) => logs.push(m) },
    });
    assert.match(logs[0], /never automatically/);
    assert.match(logs[1], /never automatically/);
    assert.match(logs[1], /HABIT_HOOKS_TIMEOUT_MS/);
  } finally {
    rmSync(modeFile(), { force: true });
    setMode(undefined);
  }

  // 3. session_stop outside a Rust project lets the session finish
  assert.deepEqual(await handlers["session_stop"][0]({}, { cwd: tmpdir() }), {
    interrupt: false,
  });
});

test("status line is labelled with the mode, shown only in Rust projects, and says off when off", async () => {
  const rust = tempProject({
    "Cargo.toml": '[package]\nname = "t"\n',
    "src/main.rs": "fn main() {}\n",
  });
  const plain = tempProject({ "README.md": "no Rust\n" });
  try {
    const { pi, handlers, statuses } = host();
    registerExtension(pi);
    const start = handlers["session_start"][0];

    start({}, { cwd: plain });
    assert.equal(statuses.at(-1), undefined);

    start({}, { cwd: rust });
    assert.equal(statuses.at(-1), "habit-hooks: turn");

    setMode("edit");
    start({}, { cwd: rust });
    assert.equal(statuses.at(-1), "habit-hooks: edit");

    setMode("off");
    start({}, { cwd: rust });
    assert.equal(statuses.at(-1), "habit-hooks: off");
  } finally {
    setMode(undefined);
    rmSync(rust, { recursive: true, force: true });
    rmSync(plain, { recursive: true, force: true });
  }
});
