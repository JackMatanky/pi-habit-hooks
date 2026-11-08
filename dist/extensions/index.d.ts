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
    sessionManager?: {
        getBranch?: () => unknown[];
    };
    ui?: {
        setStatus?: (key: string, status: string | {
            text: string;
        } | undefined) => void;
        log?: (msg: string) => void;
    };
}
export interface PiAgentHost {
    registerTool: (tool: Record<string, unknown>) => void;
    registerCommand?: (...args: unknown[]) => void;
    injectSystemPrompt?: (prompt: string) => void;
    callTool?: (toolName: string, args: Record<string, unknown>) => Promise<unknown>;
    ui?: {
        setStatus?: (key: string, status: string | {
            text: string;
        } | undefined) => void;
        log?: (msg: string) => void;
    };
    on?: (event: string, handler: (data: HostEvent, ctx?: ExtensionContext) => unknown) => void;
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
export declare function validateUnwrapWithLaya(finding: {
    smell: string;
    issues?: Array<{
        details?: {
            content?: string;
        };
    }>;
}, pi: PiAgentHost): Promise<boolean | undefined>;
export { classifyCall, isReadOnlyCommand } from "./tracking.js";
export default function (pi: PiAgentHost): void;
