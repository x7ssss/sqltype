#!/usr/bin/env node

const { spawnSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const PLATFORMS = {
  "linux-x64": { pkg: "@x7ssss/sqltype-linux-x64", bin: "sqltype" },
  "linux-arm64": { pkg: "@x7ssss/sqltype-linux-arm64", bin: "sqltype" },
  "darwin-x64": { pkg: "@x7ssss/sqltype-darwin-x64", bin: "sqltype" },
  "darwin-arm64": { pkg: "@x7ssss/sqltype-darwin-arm64", bin: "sqltype" },
  "win32-x64": { pkg: "@x7ssss/sqltype-win32-x64", bin: "sqltype.exe" },
};

function resolveBinary() {
  const platformKey = `${process.platform}-${process.arch}`;
  const target = PLATFORMS[platformKey];

  if (!target) {
    console.error(
      `[sqltype] Unsupported platform or architecture: ${process.platform} (${process.arch})`
    );
    console.error(`Supported platforms: ${Object.keys(PLATFORMS).join(", ")}`);
    process.exit(1);
  }

  // 1. Try resolving via require.resolve (handles hoisted, nested, or symlinked node_modules)
  try {
    const pkgJsonPath = require.resolve(`${target.pkg}/package.json`);
    const pkgDir = path.dirname(pkgJsonPath);
    const binPath = path.join(pkgDir, "bin", target.bin);
    if (fs.existsSync(binPath)) {
      return binPath;
    }
    const fallbackPath = path.join(pkgDir, target.bin);
    if (fs.existsSync(fallbackPath)) {
      return fallbackPath;
    }
  } catch {
    // Optional dependency could not be resolved via require.resolve
  }

  // 2. Direct node_modules relative resolution
  const candidateDirs = [
    path.join(__dirname, "..", "..", target.pkg),
    path.join(__dirname, "..", "node_modules", target.pkg),
    path.join(process.cwd(), "node_modules", target.pkg),
  ];
  for (const dir of candidateDirs) {
    const binInPkg = path.join(dir, "bin", target.bin);
    if (fs.existsSync(binInPkg)) return binInPkg;
    const directBin = path.join(dir, target.bin);
    if (fs.existsSync(directBin)) return directBin;
  }

  // 3. Monorepo / repository layout (npm/platforms/<target>/bin/...)
  const localPlatformDirs = [
    path.join(__dirname, "..", "..", "platforms", platformKey, "bin", target.bin),
    path.join(__dirname, "..", "..", "platforms", platformKey, target.bin),
  ];
  for (const p of localPlatformDirs) {
    if (fs.existsSync(p)) return p;
  }

  // 4. Local cargo build directories (target/release and target/debug)
  const cargoReleasePath = path.join(
    __dirname,
    "..",
    "..",
    "..",
    "target",
    "release",
    target.bin
  );
  if (fs.existsSync(cargoReleasePath)) {
    return cargoReleasePath;
  }

  const cargoDebugPath = path.join(
    __dirname,
    "..",
    "..",
    "..",
    "target",
    "debug",
    target.bin
  );
  if (fs.existsSync(cargoDebugPath)) {
    return cargoDebugPath;
  }

  console.error(`[sqltype] Could not find native binary for ${platformKey}.`);
  console.error(
    `Make sure the optional dependency "${target.pkg}" was installed during npm/pnpm/yarn install.`
  );
  console.error(
    `If you are on an unsupported platform, you can compile from source with: cargo install sqltype`
  );
  process.exit(1);
}

const binaryPath = resolveBinary();
const args = process.argv.slice(2);

const result = spawnSync(binaryPath, args, {
  stdio: [process.stdin, process.stdout, process.stderr],
  windowsHide: false,
});

if (result.error) {
  console.error(`[sqltype] Failed to execute ${binaryPath}:`, result.error.message);
  process.exit(1);
}

if (result.signal) {
  process.kill(process.pid, result.signal);
}

process.exit(result.status ?? 0);
