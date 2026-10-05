#!/usr/bin/env node

const { spawnSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const SUPPORTED_TARGETS = {
  "win32-x64": { bin: "sqltype.exe", label: "Windows x64" },
  "linux-x64": { bin: "sqltype", label: "Linux x64" },
  "linux-arm64": { bin: "sqltype", label: "Linux ARM64" },
  "darwin-x64": { bin: "sqltype", label: "macOS x64 (Intel)" },
  "darwin-arm64": { bin: "sqltype", label: "macOS ARM64 (Apple Silicon)" },
};

function ensureExecutable(filePath) {
  if (process.platform !== "win32") {
    try {
      const stats = fs.statSync(filePath);
      if ((stats.mode & 0o111) === 0) {
        fs.chmodSync(filePath, stats.mode | 0o755);
      }
    } catch {
      // Ignore errors if the file system is read-only
    }
  }
}

function resolveBinary() {
  const platformKey = `${process.platform}-${process.arch}`;
  const target = SUPPORTED_TARGETS[platformKey];

  if (!target) {
    console.error(
      `[sqltype] Unsupported platform or architecture: ${process.platform} (${process.arch})`
    );
    console.error(`Supported platforms: ${Object.keys(SUPPORTED_TARGETS).join(", ")}`);
    process.exit(1);
  }

  const binaryName = target.bin;

  // 1. Check for bundled binary in bin/ (architecture-specific subfolder or direct)
  const bundledCandidates = [
    path.join(__dirname, platformKey, binaryName),
    path.join(__dirname, binaryName),
  ];

  for (const candidate of bundledCandidates) {
    if (fs.existsSync(candidate)) {
      ensureExecutable(candidate);
      return candidate;
    }
  }

  // 2. Check optional dependency platform packages if present in node_modules
  const optionalPkg = `@x7ssss/sqltype-${platformKey}`;
  try {
    const pkgJsonPath = require.resolve(`${optionalPkg}/package.json`);
    const pkgDir = path.dirname(pkgJsonPath);
    const pkgCandidates = [
      path.join(pkgDir, "bin", binaryName),
      path.join(pkgDir, binaryName),
    ];
    for (const p of pkgCandidates) {
      if (fs.existsSync(p)) {
        ensureExecutable(p);
        return p;
      }
    }
  } catch {
    // Optional dependency not installed or resolve failed
  }

  // Monorepo / repository fallback (npm/platforms/<target>/bin/...)
  const repoPlatformCandidates = [
    path.join(__dirname, "..", "npm", "platforms", platformKey, "bin", binaryName),
    path.join(__dirname, "..", "npm", "platforms", platformKey, binaryName),
  ];
  for (const p of repoPlatformCandidates) {
    if (fs.existsSync(p)) {
      ensureExecutable(p);
      return p;
    }
  }

  // 3. Fallback to target/release/ or target/debug/ for local development
  const devCandidates = [
    path.join(__dirname, "..", "target", "release", binaryName),
    path.join(process.cwd(), "target", "release", binaryName),
    path.join(__dirname, "..", "target", "debug", binaryName),
    path.join(process.cwd(), "target", "debug", binaryName),
  ];

  for (const candidate of devCandidates) {
    if (fs.existsSync(candidate)) {
      ensureExecutable(candidate);
      return candidate;
    }
  }

  console.error(`[sqltype] Could not locate native executable for ${platformKey}.`);
  console.error(`Expected binary locations:`);
  for (const candidate of [...bundledCandidates, ...devCandidates]) {
    console.error(`  - ${candidate}`);
  }
  console.error(`\nTo compile locally, run: cargo build --release`);
  process.exit(1);
}

const binaryPath = resolveBinary();
const args = process.argv.slice(2);

// Handle SIGINT/SIGTERM gracefully: keep parent alive to allow child process to terminate and report status
const ignoreSignal = () => {};
process.on("SIGINT", ignoreSignal);
process.on("SIGTERM", ignoreSignal);

const result = spawnSync(binaryPath, args, {
  stdio: "inherit",
  windowsHide: false,
});

process.removeListener("SIGINT", ignoreSignal);
process.removeListener("SIGTERM", ignoreSignal);

if (result.error) {
  console.error(`[sqltype] Failed to execute ${binaryPath}:`, result.error.message);
  process.exit(1);
}

if (result.signal) {
  if (result.signal === "SIGINT") {
    process.exit(130);
  } else if (result.signal === "SIGTERM") {
    process.exit(143);
  } else {
    try {
      process.kill(process.pid, result.signal);
    } catch {
      process.exit(1);
    }
  }
}

process.exit(result.status !== null ? result.status : 0);
