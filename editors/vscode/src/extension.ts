import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;

/**
 * Resolves the sqltype executable based on configuration, local workspace installation,
 * or falls back to system PATH.
 */
export function resolveExecutable(workspaceRoot?: string): string {
  const config = vscode.workspace.getConfiguration('sqltype');
  const customPath = config.get<string>('path');

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
    throw new Error(
      `Configured SQLType binary path "${trimmed}" was not found on disk.`
    );
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
export function resolveMigrationsPath(workspaceRoot?: string): string {
  const config = vscode.workspace.getConfiguration('sqltype');
  const configuredMigrations = config.get<string>('migrationsPath', 'migrations');
  if (path.isAbsolute(configuredMigrations)) {
    return configuredMigrations;
  }
  if (workspaceRoot) {
    return path.resolve(workspaceRoot, configuredMigrations);
  }
  return configuredMigrations;
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
  const workspaceRoot = workspaceFolder?.uri.fsPath;

  let executablePath: string;
  try {
    executablePath = resolveExecutable(workspaceRoot);
  } catch (err: unknown) {
    const message = err instanceof Error ? err.message : String(err);
    vscode.window.showErrorMessage(`SQLType: ${message}`);
    return;
  }

  const migrationsPath = resolveMigrationsPath(workspaceRoot);

  const serverOptions: ServerOptions = {
    command: executablePath,
    args: ['lsp', '--migrations', migrationsPath],
    transport: TransportKind.stdio,
    options: workspaceRoot ? { cwd: workspaceRoot } : undefined
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: 'file', language: 'sql' }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher('**/*.sql')
    }
  };

  client = new LanguageClient(
    'sqltype',
    'SQLType Language Server',
    serverOptions,
    clientOptions
  );

  try {
    await client.start();
  } catch (err: unknown) {
    const message = err instanceof Error ? err.message : String(err);
    vscode.window.showErrorMessage(
      `Failed to start SQLType Language Server: ${message}`
    );
  }
}

export async function deactivate(): Promise<void> {
  if (!client) {
    return undefined;
  }
  return client.stop();
}
