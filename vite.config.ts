import { defineConfig } from "vite-plus";

// Lint and format rules live here because Vite+ mode ignores nested
// oxlint.config.ts / oxfmt.config.ts files: the workspace root block governs
// every package, with per-package deltas expressed as overrides.
//
// Oxfmt formats Markdown as well as JS/TS/JSON, so `vp fmt` is the only
// formatter for prose. TOML belongs to taplo.
export default defineConfig({
  fmt: {
    printWidth: 120,
    semi: true,
    singleQuote: false,
    trailingComma: "all",
    sortImports: true,
    sortPackageJson: true,
    ignorePatterns: [
      "*.toml",
      "**/*.toml",
      ".agents/**",
      ".claude/**",
      "reference/**",
      "node_modules/**",
      "target/**",
      ".direnv/**",
      "result",
      "result-*",
    ],
  },
  lint: {
    ignorePatterns: [
      ".agents/**",
      ".claude/**",
      "reference/**",
      "node_modules/**",
      "target/**",
      ".direnv/**",
      "result",
      "result-*",
    ],
    options: {
      typeAware: true,
      typeCheck: true,
    },
  },
  run: {
    tasks: {
      // `pnpm check` runs the whole gate. The task runner caches each step by
      // the files it reads; the Rust and prose steps are plain commands.
      "check:all": {
        command: "pnpm lint:rust && pnpm deny && pnpm lint:typos && vp lint && vp fmt --check",
        // Tasks start from a clean environment. Cargo build scripts need the
        // Nix devshell's compiler, linker and pkg-config variables (Linux
        // finds fontconfig and friends through them) and the macOS SDK path.
        cache: {
          untrackedEnv: [
            "NIX_*",
            "PKG_CONFIG*",
            "LD_LIBRARY_PATH",
            "OPENSSL_*",
            "SDKROOT",
            "CC",
            "CXX",
            "AR",
            "LD",
            "RUST_SRC_PATH",
          ],
        },
      },
    },
  },
  test: {
    exclude: ["**/node_modules/**", "**/.direnv/**", "**/.agents/**", "**/.claude/**", "reference/**", "target/**"],
  },
  staged: {
    "*": "vp check --fix",
  },
});
