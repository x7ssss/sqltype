"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
exports.resolveExecutable = resolveExecutable;
exports.resolveMigrationsPath = resolveMigrationsPath;
exports.activate = activate;
exports.deactivate = deactivate;
const vscode = __importStar(require("vscode"));
const path = __importStar(require("path"));
const fs = __importStar(require("fs"));
const node_1 = require("vscode-languageclient/node");
let client;
/**
 * Resolves the sqltype executable based on configuration, local workspace installation,
 * or falls back to system PATH.
 */
function resolveExecutable(workspaceRoot) {
    const config = vscode.workspace.getConfiguration('sqltype');
    const customPath = config.get('path');
    // 1. User-configured sqltype.path (throw descriptive error if set but missing)
    if (customPath && customPath.trim().length > 0) {
        const trimmed = customPath.trim();
        const resolvedCustom = path.isAbsolute(trimmed)
            ? trimmed
            : workspaceRoot
                ? path.resolve(workspaceRoot, trimmed)
                : trimmed;
        if (fs.existsSync(resolvedCustom)) {
            return resolvedCustom;
        }
        throw new Error(`Configured SQLType binary path "${trimmed}" was not found on disk.`);
    }
    // 2. Workspace node_modules/.bin/sqltype (sqltype.cmd on Windows)
    if (workspaceRoot) {
        const binName = process.platform === 'win32' ? 'sqltype.cmd' : 'sqltype';
        const localBin = path.join(workspaceRoot, 'node_modules', '.bin', binName);
        if (fs.existsSync(localBin)) {
            return localBin;
        }
        if (process.platform === 'win32') {
            const localExe = path.join(workspaceRoot, 'node_modules', '.bin', 'sqltype.exe');
            if (fs.existsSync(localExe)) {
                return localExe;
            }
        }
    }
    // 3. Fall back to "sqltype" on system PATH
    return 'sqltype';
}
/**
 * Resolves the migrations directory relative to active workspace folder.
 */
function resolveMigrationsPath(workspaceRoot) {
    const config = vscode.workspace.getConfiguration('sqltype');
    const configuredMigrations = config.get('migrationsPath', 'migrations');
    if (path.isAbsolute(configuredMigrations)) {
        return configuredMigrations;
    }
    if (workspaceRoot) {
        return path.resolve(workspaceRoot, configuredMigrations);
    }
    return configuredMigrations;
}
async function activate(context) {
    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    const workspaceRoot = workspaceFolder?.uri.fsPath;
    let executablePath;
    try {
        executablePath = resolveExecutable(workspaceRoot);
    }
    catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        vscode.window.showErrorMessage(`SQLType: ${message}`);
        return;
    }
    const migrationsPath = resolveMigrationsPath(workspaceRoot);
    const serverOptions = {
        command: executablePath,
        args: ['lsp', '--migrations', migrationsPath],
        transport: node_1.TransportKind.stdio,
        options: workspaceRoot ? { cwd: workspaceRoot } : undefined
    };
    const clientOptions = {
        documentSelector: [{ scheme: 'file', language: 'sql' }],
        synchronize: {
            fileEvents: vscode.workspace.createFileSystemWatcher('**/*.sql')
        }
    };
    client = new node_1.LanguageClient('sqltype', 'SQLType Language Server', serverOptions, clientOptions);
    try {
        await client.start();
    }
    catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        vscode.window.showErrorMessage(`Failed to start SQLType Language Server: ${message}`);
    }
}
async function deactivate() {
    if (!client) {
        return undefined;
    }
    return client.stop();
}
//# sourceMappingURL=extension.js.map