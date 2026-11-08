import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

export function git(cwd: string, ...args: string[]): void {
  execFileSync("git", args, { cwd, stdio: "ignore" });
}

/** A temp git repo whose `files` are committed on `main`; the caller removes it. */
export function initRepo(prefix: string, files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  git(dir, "init", "-q", "-b", "main");
  git(dir, "config", "user.email", "t@example.com");
  git(dir, "config", "user.name", "t");
  for (const [name, body] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, name)), { recursive: true });
    writeFileSync(join(dir, name), body);
  }
  git(dir, "add", ".");
  git(dir, "commit", "-qm", "base");
  return dir;
}

/** Never read or write the developer's real ~/.config/habit-hooks/mode (`/habit-hooks off` persists). */
export function isolateModeFile(): void {
  process.env.HABIT_HOOKS_MODE_FILE = join(mkdtempSync(join(tmpdir(), "habit-hooks-mode-")), "mode");
  delete process.env.HABIT_HOOKS_MODE;
}
