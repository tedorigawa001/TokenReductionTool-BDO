#!/usr/bin/env bun
/**
 * Bushido Full Integration Test Suite — Multipass VM
 *
 * Usage:
 *   bun run scripts/benchmark/run.ts           # Full suite
 *   bun run scripts/benchmark/run.ts --quick   # Skip slow phases (perf, concurrency)
 *   bun run scripts/benchmark/run.ts --phase 3 # Run specific phase only
 *
 * Prerequisites:
 *   brew install multipass
 */

import { $ } from "bun";
import { vmEnsureReady, vmBuildRtk, vmExec, BDO_BIN } from "./lib/vm";
import { testCmd, testSavings, testRewrite, skipTest, getCounts } from "./lib/test";
import { saveReport } from "./lib/report";

const args = process.argv.slice(2);
const quick = args.includes("--quick");
const phaseArg = args.includes("--phase")
  ? parseInt(args[args.indexOf("--phase") + 1], 10)
  : null;
const phaseOnly = phaseArg !== null && !Number.isNaN(phaseArg) ? phaseArg : null;
if (args.includes("--phase") && phaseOnly === null) {
  console.error("Error: --phase requires a number (e.g. --phase 3)");
  process.exit(1);
}
const reportPath = args.includes("--report")
  ? args[args.indexOf("--report") + 1]
  : `${new URL("../../", import.meta.url).pathname.replace(/\/$/, "")}/benchmark-report.txt`;

const PROJECT_ROOT = new URL("../../", import.meta.url).pathname.replace(/\/$/, "");
const Bushido = BDO_BIN;

function shouldRun(phase: number): boolean {
  return phaseOnly === null || phaseOnly === phase;
}

function heading(phase: number, title: string) {
  console.log(`\n\x1b[34m[Phase ${phase}] ${title}\x1b[0m`);
}

// ══════════════════════════════════════════════════════════════
// Phase 0: VM Setup
// ══════════════════════════════════════════════════════════════

console.log("\x1b[34m[bushido-test] Bushido Full Integration Test Suite\x1b[0m");
console.log(`Project: ${PROJECT_ROOT}`);

await vmEnsureReady();

// ══════════════════════════════════════════════════════════════
// Phase 1: Transfer & Build
// ══════════════════════════════════════════════════════════════

heading(1, "Transfer & Build");
const branch = (await $`git -C ${PROJECT_ROOT} branch --show-current`.text()).trim();
const commit = (await $`git -C ${PROJECT_ROOT} log --oneline -1`.text()).trim();
const buildInfo = await vmBuildRtk(PROJECT_ROOT);

// Binary size check
// ARM Linux release binaries are ~6.5MB (vs ~4MB x86 stripped).
// CLAUDE.md target is <5MB for stripped x86 release builds.
// VM builds are ARM + not fully stripped, so we use a relaxed 8MB limit here.
const sizeLimit = 8_388_608; // 8MB (relaxed for ARM Linux VM)
if (buildInfo.binarySize < sizeLimit) {
  console.log(`  \x1b[32mPASS\x1b[0m | binary size | ${buildInfo.binarySize} bytes < 8MB`);
} else {
  console.log(`  \x1b[31mFAIL\x1b[0m | binary size | ${buildInfo.binarySize} bytes >= 8MB`);
}

// ══════════════════════════════════════════════════════════════
// Phase 2: Cargo Quality (fmt, clippy, test)
// ══════════════════════════════════════════════════════════════

if (shouldRun(2)) {
  heading(2, "Cargo Quality");

  await testCmd(
    "quality:cargo fmt",
    "export PATH=$HOME/.cargo/bin:$PATH && cd /home/ubuntu/bushido && cargo fmt --all --check 2>&1"
  );

  await testCmd(
    "quality:cargo clippy",
    "export PATH=$HOME/.cargo/bin:$PATH && cd /home/ubuntu/bushido && cargo clippy --all-targets -- -D warnings 2>&1"
  );

  await testCmd(
    "quality:cargo test",
    "export PATH=$HOME/.cargo/bin:$PATH && cd /home/ubuntu/bushido && cargo test --all 2>&1"
  );
}

// ══════════════════════════════════════════════════════════════
// Phase 3: Rust Built-in Commands
// ══════════════════════════════════════════════════════════════

if (shouldRun(3)) {
  heading(3, "Rust Built-in Commands");

  // Git
  await testCmd("git:status", `cd /tmp/test-git && ${Bushido} git status`);
  await testCmd("git:log", `cd /tmp/test-git && ${Bushido} git log -5`);
  await testCmd("git:log --oneline", `cd /tmp/test-git && ${Bushido} git log --oneline -10`);
  await testCmd("git:diff", `cd /tmp/test-git && ${Bushido} git diff`, "any");
  await testCmd("git:branch", `cd /tmp/test-git && ${Bushido} git branch`);
  await testCmd("git:add --dry-run", `cd /tmp/test-git && ${Bushido} git add --dry-run .`, "any");

  // Files
  await testCmd("files:ls", `${Bushido} ls /home/ubuntu/bushido`);
  await testCmd("files:ls src/", `${Bushido} ls /home/ubuntu/bushido/src/`);
  await testCmd("files:ls -R", `${Bushido} ls -R /home/ubuntu/bushido/src/`);
  await testCmd("files:read", `${Bushido} read /home/ubuntu/bushido/src/main.rs`);
  await testCmd("files:read aggressive", `${Bushido} read /home/ubuntu/bushido/src/main.rs -l aggressive`);
  await testCmd("files:smart", `${Bushido} smart /home/ubuntu/bushido/src/main.rs`);
  await testCmd("files:find *.rs", `${Bushido} find '*.rs' /home/ubuntu/bushido/src/`);
  await testCmd("files:wc", `${Bushido} wc /home/ubuntu/bushido/src/main.rs`);
  await testCmd("files:diff", `${Bushido} diff /home/ubuntu/bushido/src/main.rs /home/ubuntu/bushido/src/utils.rs`);

  // Search
  await testCmd("search:grep", `${Bushido} grep 'fn main' /home/ubuntu/bushido/src/`);

  // Data
  await testCmd("data:json", `${Bushido} json /tmp/test-node/package.json`);
  await testCmd("data:deps", `cd /home/ubuntu/bushido && ${Bushido} deps`);
  await testCmd("data:env", `${Bushido} env`);

  // Runners
  await testCmd("runner:summary", `${Bushido} summary 'echo hello world'`);
  // BUG: bdo err swallows exit code — tracked in #846
  await testCmd("runner:err", `${Bushido} err false`, "any");
  await testCmd("runner:test", `${Bushido} test 'echo ok'`, "any");

  // Logs
  await testCmd("log:large", `${Bushido} log /tmp/large.log`);

  // Network
  await testCmd("net:curl", `${Bushido} curl https://httpbin.org/get`, "any");

  // GitHub
  await testCmd("gh:pr list", `cd /home/ubuntu/bushido && ${Bushido} gh pr list`, "any");

  // Cargo (test project has intentional test failure → exit 101)
  await testCmd("cargo:build", `export PATH=$HOME/.cargo/bin:$PATH && cd /tmp/test-rust && ${Bushido} cargo build`);
  await testCmd("cargo:test", `export PATH=$HOME/.cargo/bin:$PATH && cd /tmp/test-rust && ${Bushido} cargo test`, 101);
  await testCmd("cargo:clippy", `export PATH=$HOME/.cargo/bin:$PATH && cd /tmp/test-rust && ${Bushido} cargo clippy`);

  // Python (test project has intentional failures)
  await testCmd("python:pytest", `cd /tmp/test-python && ${Bushido} pytest`, 1);
  await testCmd("python:ruff check", `cd /tmp/test-python && ${Bushido} ruff check .`, 1);
  await testCmd("python:mypy", `cd /tmp/test-python && ${Bushido} mypy .`, 1);
  await testCmd("python:pip list", `${Bushido} pip list`);

  // Go (test project has intentional test failure)
  await testCmd("go:test", `export PATH=$PATH:/usr/local/go/bin && cd /tmp/test-go && ${Bushido} go test ./...`, 1);
  await testCmd("go:build", `export PATH=$PATH:/usr/local/go/bin && cd /tmp/test-go && ${Bushido} go build .`, 1);
  await testCmd("go:vet", `export PATH=$PATH:/usr/local/go/bin && cd /tmp/test-go && ${Bushido} go vet ./...`, 1);
  await testCmd("go:golangci-lint", `export PATH=$PATH:/usr/local/go/bin:$HOME/go/bin && cd /tmp/test-go && ${Bushido} golangci-lint run`, 1);

  // TypeScript
  await testCmd("ts:tsc", `cd /tmp/test-node && ${Bushido} tsc --noEmit`, "any");

  // Linters
  await testCmd("lint:eslint", `cd /tmp/test-node && ${Bushido} lint 'eslint src/'`, "any");
  await testCmd("lint:prettier", `cd /tmp/test-node && ${Bushido} prettier --check src/`, "any");

  // Docker
  await testCmd("docker:ps", `${Bushido} docker ps`, "any");
  await testCmd("docker:images", `${Bushido} docker images`, "any");

  // Kubernetes
  await testCmd("k8s:pods", `${Bushido} kubectl pods`, "any");

  // .NET
  await testCmd("dotnet:build", `export DOTNET_ROOT=/usr/local/share/dotnet && export PATH=$PATH:$DOTNET_ROOT && cd /tmp/test-dotnet/TestApp 2>/dev/null && ${Bushido} dotnet build || echo 'dotnet skip'`, "any");

  // Meta
  await testCmd("meta:gain", `${Bushido} gain`);
  await testCmd("meta:gain --history", `${Bushido} gain --history`);
  await testCmd("meta:proxy", `${Bushido} proxy echo 'proxy test'`);
  await testCmd("meta:verify", `${Bushido} verify`, "any");
}

// ══════════════════════════════════════════════════════════════
// Phase 4: TOML Filter Commands
// ══════════════════════════════════════════════════════════════

if (shouldRun(4)) {
  heading(4, "TOML Filter Commands");

  // System
  await testCmd("toml:df", `${Bushido} df -h`);
  await testCmd("toml:du", `${Bushido} du -sh /tmp`, "any");
  await testCmd("toml:ps", `${Bushido} ps aux`);
  await testCmd("toml:ping", `${Bushido} ping -c 2 127.0.0.1`);

  // Build tools
  await testCmd("toml:make", `cd /tmp && ${Bushido} make -f Makefile`, "any");
  await testCmd("toml:rsync", `${Bushido} rsync --version`);

  // Linters
  await testCmd("toml:shellcheck", `${Bushido} shellcheck /tmp/test.sh`, "any");
  await testCmd("toml:hadolint", `${Bushido} hadolint /tmp/Dockerfile.bad`, "any");
  await testCmd("toml:yamllint", `${Bushido} yamllint /tmp/test.yaml`, "any");
  await testCmd("toml:markdownlint", `${Bushido} markdownlint /tmp/test.md`, "any");

  // Cloud/Infra
  await testCmd("toml:terraform", `${Bushido} terraform --version`, "any");
  await testCmd("toml:helm", `${Bushido} helm version`, "any");
  await testCmd("toml:ansible", `${Bushido} ansible-playbook --version`, "any");

  // Mocked tools
  await testCmd("toml:gcloud", `${Bushido} gcloud version`);
  await testCmd("toml:shopify", `${Bushido} shopify theme check`, "any");
  await testCmd("toml:pio", `${Bushido} pio run`, "any");
  await testCmd("toml:quarto", `${Bushido} quarto render`, "any");
  await testCmd("toml:sops", `${Bushido} sops --version`);
  // Swift ecosystem
  await testCmd("toml:swift build", `${Bushido} swift build`, "any");
  await testCmd("toml:swift test", `${Bushido} swift test`, "any");
  await testCmd("toml:swift run", `${Bushido} swift run`, "any");
  await testCmd("toml:swift package", `${Bushido} swift package resolve`, "any");
  await testCmd("toml:swiftlint", `${Bushido} swiftlint`, "any");
  await testCmd("toml:swiftformat", `${Bushido} swiftformat`, "any");
  await testCmd("toml:kubectl", `${Bushido} kubectl version --client`, "any");
}

// ══════════════════════════════════════════════════════════════
// Phase 5: Hook Rewrite Engine
// ══════════════════════════════════════════════════════════════

if (shouldRun(5)) {
  heading(5, "Hook Rewrite Engine");

  // Basic rewrites
  await testRewrite("git status", "bdo git status");
  await testRewrite("git log --oneline -10", "bdo git log --oneline -10");
  await testRewrite("cargo test", "bdo cargo test");
  await testRewrite("cargo build --release", "bdo cargo build --release");
  await testRewrite("docker ps", "bdo docker ps");
  // NOTE: bdo rewrites "kubectl get pods" to "bdo kubectl get pods" (preserves get)
  await testRewrite("kubectl get pods", "bdo kubectl get pods");
  await testRewrite("ruff check", "bdo ruff check");
  await testRewrite("pytest", "bdo pytest");
  await testRewrite("go test", "bdo go test");
  await testRewrite("pnpm list", "bdo pnpm list");
  await testRewrite("gh pr list", "bdo gh pr list");
  await testRewrite("df -h", "bdo df -h");
  await testRewrite("ps aux", "bdo ps aux");

  // Compound
  await testRewrite("cargo test && git status", "bdo cargo test && bdo git status");
  // NOTE: shell strips single quotes in vmExec, so 'msg' becomes msg
  await testRewrite("git add . && git commit -m msg", "bdo git add . && bdo git commit -m msg");

  // No rewrite (shell builtins) — bdo rewrite returns empty string + exit 1
  // We test via testCmd since testRewrite expects non-empty output
  await testCmd("rewrite:cd (no rewrite)", `${Bushido} rewrite 'cd /tmp'`, 1);
  await testCmd("rewrite:export (no rewrite)", `${Bushido} rewrite 'export FOO=bar'`, 1);
}

// ══════════════════════════════════════════════════════════════
// Phase 6: Exit Code Preservation
// ══════════════════════════════════════════════════════════════

if (shouldRun(6)) {
  heading(6, "Exit Code Preservation");

  // Success
  await testCmd("exit:git status=0", `cd /tmp/test-git && ${Bushido} git status`, 0);
  await testCmd("exit:ls=0", `${Bushido} ls /tmp`, 0);
  await testCmd("exit:gain=0", `${Bushido} gain`, 0);

  // Failures
  // rg returns exit 1 (no match) or 2 (error) — accept both
  await testCmd("exit:grep NOTFOUND", `${Bushido} grep NOTFOUND_XYZ_123 /tmp`, "any");
}

// ══════════════════════════════════════════════════════════════
// Phase 7: Token Savings
// ══════════════════════════════════════════════════════════════

if (shouldRun(7)) {
  heading(7, "Token Savings");

  await testSavings(
    "savings:git log",
    "cd /tmp/test-git && git log -20",
    `cd /tmp/test-git && ${Bushido} git log -20`,
    60
  );
  await testSavings(
    "savings:ls",
    "ls -la /home/ubuntu/bushido/src/",
    `${Bushido} ls /home/ubuntu/bushido/src/`,
    60
  );
  await testSavings(
    "savings:log dedup",
    "cat /tmp/large.log",
    `${Bushido} log /tmp/large.log`,
    80
  );
  await testSavings(
    "savings:read aggressive",
    "cat /home/ubuntu/bushido/src/main.rs",
    `${Bushido} read /home/ubuntu/bushido/src/main.rs -l aggressive`,
    50
  );
  await testSavings(
    "savings:swift test",
    "swift test",
    `${Bushido} swift test`,
    60
  );
  await testSavings(
    "savings:swiftlint",
    "swiftlint",
    `${Bushido} swiftlint`,
    20
  );
}

// ══════════════════════════════════════════════════════════════
// Phase 8: Pipe Compatibility
// ══════════════════════════════════════════════════════════════

if (shouldRun(8)) {
  heading(8, "Pipe Compatibility");

  await testCmd("pipe:git status|wc", `cd /tmp/test-git && ${Bushido} git status | wc -l`);
  await testCmd("pipe:ls|wc", `${Bushido} ls /home/ubuntu/bushido/src/ | wc -l`);
  await testCmd("pipe:grep|head", `${Bushido} grep 'fn' /home/ubuntu/bushido/src/ | head -5`);
}

// ══════════════════════════════════════════════════════════════
// Phase 9: Edge Cases
// ══════════════════════════════════════════════════════════════

if (shouldRun(9)) {
  heading(9, "Edge Cases");

  await testCmd("edge:summary true", `${Bushido} summary 'true'`, "any");
  await testCmd("edge:grep NOTFOUND", `${Bushido} grep NOTFOUND_XYZ /home/ubuntu/bushido/src/`, 1);
  await testCmd("edge:unicode", `echo 'hello world' > /tmp/uni.txt && ${Bushido} grep 'hello' /tmp`, "any");
}

// ══════════════════════════════════════════════════════════════
// Phase 10: Performance (skip with --quick)
// ══════════════════════════════════════════════════════════════

if (shouldRun(10) && !quick) {
  heading(10, "Performance");

  // hyperfine
  const { exitCode: hfExist } = await vmExec("command -v hyperfine");
  if (hfExist === 0) {
    const { stdout: hfOut } = await vmExec(
      `cd /tmp/test-git && hyperfine --warmup 3 --min-runs 5 '${Bushido} git status' 'git status' --export-json /dev/stdout 2>/dev/null`
    );
    try {
      const hf = JSON.parse(hfOut);
      const rtkMean = (hf.results?.[0]?.mean * 1000).toFixed(1);
      const rawMean = (hf.results?.[1]?.mean * 1000).toFixed(1);
      console.log(`  Startup: bdo=${rtkMean}ms raw=${rawMean}ms`);
    } catch {
      console.log("  hyperfine output parse failed");
    }
  } else {
    skipTest("perf:hyperfine", "not installed");
  }

  // Memory
  const { stdout: memOut } = await vmExec(
    `cd /tmp/test-git && /usr/bin/time -v ${Bushido} git status 2>&1 | grep 'Maximum resident'`
  );
  const memKb = parseInt(memOut.match(/(\d+)/)?.[1] ?? "0", 10);
  if (memKb > 0 && memKb < 20000) {
    await testCmd("perf:memory", `echo '${memKb} KB < 20MB'`);
  } else if (memKb > 0) {
    await testCmd("perf:memory", `echo '${memKb} KB >= 20MB' && exit 1`, 0);
  }
} else if (quick && shouldRun(10)) {
  skipTest("perf:hyperfine", "--quick mode");
  skipTest("perf:memory", "--quick mode");
}

// ══════════════════════════════════════════════════════════════
// Phase 11: Concurrency (skip with --quick)
// ══════════════════════════════════════════════════════════════

if (shouldRun(11) && !quick) {
  heading(11, "Concurrency");

  await testCmd(
    "concurrency:10x git status",
    `cd /tmp/test-git && for i in $(seq 1 10); do ${Bushido} git status >/dev/null & done; wait`
  );
} else if (quick && shouldRun(11)) {
  skipTest("concurrency:10x", "--quick mode");
}

// ══════════════════════════════════════════════════════════════
// Report
// ══════════════════════════════════════════════════════════════

const report = await saveReport(
  { ...buildInfo, branch, commit },
  reportPath
);

console.log("\n" + report);

const { total, passed, failed, skipped } = getCounts();
const passRate = total > 0 ? Math.round((passed * 100) / total) : 0;

if (failed === 0) {
  console.log(`\n\x1b[32m  READY FOR RELEASE — ${passed}/${total} (${passRate}%)\x1b[0m\n`);
  process.exit(0);
} else {
  console.log(`\n\x1b[31m  NOT READY — ${failed} failures — ${passed}/${total} (${passRate}%)\x1b[0m\n`);
  process.exit(1);
}
