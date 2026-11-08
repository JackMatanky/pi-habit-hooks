import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import registerExtension from "../extensions/index.js";
import { FirePolicy } from "../extensions/policy.js";
// Never read the developer's real ~/.config/habit-hooks/mode.
process.env.HABIT_HOOKS_MODE_FILE = join(mkdtempSync(join(tmpdir(), "habit-hooks-mode-")), "mode");
delete process.env.HABIT_HOOKS_MODE;
function git(cwd, ...args) {
    execFileSync("git", args, { cwd, stdio: "ignore" });
}
async function withRepo(body) {
    const dir = mkdtempSync(join(tmpdir(), "habit-hooks-iso-"));
    try {
        git(dir, "init", "-q", "-b", "main");
        git(dir, "config", "user.email", "t@example.com");
        git(dir, "config", "user.name", "t");
        writeFileSync(join(dir, "Cargo.toml"), '[package]\nname = "t"\n');
        mkdirSync(join(dir, "src"));
        git(dir, "add", ".");
        git(dir, "commit", "-qm", "base");
        await body(dir);
    }
    finally {
        rmSync(dir, { recursive: true, force: true });
    }
}
function extension() {
    const handlers = {};
    registerExtension({
        registerTool: () => { },
        on: (event, handler) => {
            (handlers[event] ??= []).push(handler);
        },
    });
    /** One edit tool call by `agent`, from tool_call through tool_result, exactly as the host fires them. */
    const edit = async (dir, agent, file, callId) => {
        const ctx = { cwd: dir, agent: { kind: "main", id: agent } };
        const event = { toolName: "edit", toolCallId: callId, input: { path: file } };
        await handlers["tool_call"][0](event, ctx);
        writeFileSync(join(dir, file), `pub fn ${agent}() {}\n// ${callId}\n`);
        return handlers["tool_result"][0](event, ctx);
    };
    return { handlers, edit };
}
/** The CLI stand-in: a smell for anything involving src/a.rs, clean otherwise; records what each run was scoped to. */
function fakeCli() {
    const scopes = [];
    const run = async (options) => {
        scopes.push((options.files ?? ["<whole tree>"]).join());
        return options.files?.includes("src/a.rs")
            ? { exitCode: 1, passed: false, output: "### too-many-parameters\n- src/a.rs:1: too many parameters\n" }
            : { exitCode: 0, passed: true, output: "" };
    };
    return { run, scopes };
}
test("two agents editing one repo: each stop gate sees only its own file, and only the guilty agent is refused", () => withRepo(async (dir) => {
    const { edit } = extension();
    const cli = fakeCli();
    const policy = new FirePolicy(cli.run);
    const stampA = await edit(dir, "agent-a", "src/a.rs", "call-a");
    const stampB = await edit(dir, "agent-b", "src/b.rs", "call-b");
    assert.deepEqual(stampA, { details: { habitHooks: { hard: ["src/a.rs"], soft: [] } } });
    assert.deepEqual(stampB, { details: { habitHooks: { hard: ["src/b.rs"], soft: [] } } });
    const [b, a] = await Promise.all([policy.gate(dir, undefined, "agent-b"), policy.gate(dir, undefined, "agent-a")]);
    assert.equal(b.block, false);
    assert.equal(b.status, "✅ clean");
    assert.equal(a.block, true);
    assert.match(a.feedback ?? "", /src\/a\.rs/);
    assert.deepEqual(cli.scopes.slice().sort(), ["src/a.rs", "src/b.rs"]);
}));
test("overlapping tool calls make the attribution soft, so neither agent is refused for the other's smell", () => withRepo(async (dir) => {
    const { handlers } = extension();
    const cli = fakeCli();
    const policy = new FirePolicy(cli.run);
    const call = (agent, callId) => ({
        event: { toolName: "bash", toolCallId: callId, input: { command: "cargo fmt" } },
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
