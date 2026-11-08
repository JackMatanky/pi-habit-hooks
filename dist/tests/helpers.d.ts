export declare function git(cwd: string, ...args: string[]): void;
/** A temp git repo whose `files` are committed on `main`; the caller removes it. */
export declare function initRepo(prefix: string, files: Record<string, string>): string;
/** Never read or write the developer's real ~/.config/habit-hooks/mode (`/habit-hooks off` persists). */
export declare function isolateModeFile(): void;
