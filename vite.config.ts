import { defineConfig } from "vite-plus";

// Shared by `vp test`, `vp fmt` and `vp lint`; see mise.toml for the tasks.
const ignorePatterns = [
  ".agents/**",
  ".habit-hooks/**",
  ".mise/**",
  ".pi/**",
  "crates/**",
  "dist/**",
  "plugins/**",
  "target/**",
  // Markdown belongs to the docs tooling and TOML to tombi (see hk.pkl).
  "**/*.md",
  "**/*.toml",
];
export default defineConfig({
  test: {
    include: ["tests/**/*.test.ts"],
    // Rust-backed tests start real cargo/git processes.
    testTimeout: 60_000,
  },
  fmt: {
    printWidth: 80,
    sortImports: {
      groups: [
        "type",
        "builtin",
        "external",
        ["internal", "subpath"],
        ["parent", "sibling", "index"],
        "unknown",
      ],
    },
    sortPackageJson: { sortScripts: true },
    ignorePatterns: [...ignorePatterns, "package-lock.json"],
  },
  lint: {
    plugins: ["typescript", "unicorn", "oxc", "node"],
    categories: { correctness: "error" },
    ignorePatterns,
    rules: {
      "oxc/no-accumulating-spread": "error",
    },
  },
});
