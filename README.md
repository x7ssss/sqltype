# sqltype

> **Ultra-fast, local-first SQL-to-TypeScript compiler CLI and Language Server in Rust.**  
> Write raw PostgreSQL queries with compile-time type safety. **Zero** Docker containers. **Zero** WASM overhead. **Sub-10ms** codegen & LSP.

[![Release](https://img.shields.io/badge/npm-v0.1.0-blue.svg)](https://www.npmjs.com/package/sqltype)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-33%20passed-brightgreen.svg)]()

---

## Why sqltype?

Modern TypeScript database tools often force an undesirable trade-off:
- **ORMs (Prisma, Drizzle)** introduce proprietary query DSLs, runtime overhead, and complex migration engines.
- **PgTyped** requires an active PostgreSQL connection or Docker instance running during compilation to infer types.
- **sqlc-gen-typescript** runs heavy 69MB WASM blobs in Node.js, introducing significant compile-time latency.

**`sqltype`** eliminates these compromises. By embedding PostgreSQL's native C-parser (`libpg_query`) into a standalone Rust binary, `sqltype` parses raw migration DDL files offline to build an exact in-memory schema catalog and analyzes SQL query ASTs in **sub-10ms**, emitting clean, zero-dependency TypeScript types and SQL constants.

---

## Comparison

| Feature | `sqltype` | PgTyped | sqlc (TS) | Prisma |
| :--- | :--- | :--- | :--- | :--- |
| **Running Database Required** | **No** (Offline AST) | Yes (Docker / Postgres) | No | No (Dev schema engine) |
| **Codegen Speed** | **< 10ms** (Native Rust) | 200–800ms (Network roundtrip) | 2–3s (69MB WASM) | 1–3s (Node / WASM) |
| **Runtime Overhead** | **0 KB** (Pure types & strings) | Runtime helper | 0 KB | Heavy client library & engine |
| **Outer Join Nullability** | **Automatic** (AST traversal) | Manual overrides / Flaky | Manual casts | Handled |
| **CTE / Subquery Support** | **Automatic** (Scope overlay) | Partial | Partial | Proprietary syntax |
| **Driver Profiles** | **Postgres.js, pg, Bun.sql** | Single driver | Static | Proprietary engine |
| **Dynamic / Optional Filters**| **AST detection (`col?: T \| null`)** | Manual casts | Partial | Built-in |
| **DML Support** | **INSERT, UPDATE, DELETE (±RETURNING)**| Partial | Partial | Yes |
| **Sub-5ms Watch Mode** | **Yes** (Incremental) | No | No | Partial |
| **Language Server (LSP)** | **Yes** (Sub-10ms, stdio) | No | No | Yes (Prisma Schema only) |

---

## Comprehensive Feature Breakdown

### 1. Offline AST Analysis (Zero Docker, Zero Postgres)
Uses PostgreSQL's native grammar parser via `libpg_query` directly compiled into Rust. Builds an exact schema catalog entirely from your migration `.sql` files without needing Docker or a live database.

### 2. Strict Driver Target Profiles (`--driver postgres | pg | bun`)
Fine-tune generated TypeScript primitives to match your database driver runtime:
- **`postgres`** (default, `postgres.js`):
  - `int8` / `bigint` -> `string`
  - `bytea` -> `Buffer`
  - `date` -> `string`
  - `timestamp` / `timestamptz` -> `Date`
- **`pg`** (`node-postgres`):
  - `int8` / `bigint` -> `string`
  - `bytea` -> `Buffer`
  - `date` -> `string`
  - `timestamp` / `timestamptz` -> `Date`
- **`bun`** (`Bun.sql`):
  - `int8` / `bigint` -> native JavaScript `bigint`
  - `bytea` -> `Uint8Array`
  - `date` -> `string`
  - `timestamp` / `timestamptz` -> `Date`

### 3. AST Detection for Optional / Dynamic Filters
`sqltype` statically detects nullable parameter bypass patterns in `WHERE` clauses:
```sql
WHERE ($1::text IS NULL OR name = $1) AND id = $2;
```
Recognizes that `$1` is optional and emits:
```typescript
export interface FindUserParams {
  name?: string | null;
  id: string;
}
```

### 4. Full DML Mutation Support (with and without `RETURNING`)
Full static typing for mutations alongside queries:
- **`INSERT`**: Correlates insert columns with parameter values; columns with database defaults or nullable definitions are treated as optional parameters where appropriate.
- **`UPDATE`**: Analyzes `SET` target assignments and `WHERE` filter conditions.
- **`DELETE`**: Maps filter parameters and validates table relations.
- **`RETURNING` Clause**:
  - **With `RETURNING`**: Generates both `<Query>Params` and `<Query>Row` interfaces.
  - **Without `RETURNING`**: Emits pure mutation query types (`Params` only, omitting unnecessary empty row types).

### 5. Static Join Nullability & CTE Scoping
- **`LEFT JOIN`**: Right-hand table projections are automatically marked `| null`.
- **`RIGHT JOIN`**: Left-hand table projections are automatically marked `| null`.
- **`FULL JOIN`**: Projections from both sides are marked `| null`.
- **`WITH` (Common Table Expressions)**: Recursively parses CTE queries, registering temporary projected schemas into a query-scoped catalog overlay.

### 6. Expression & Function Resolution
- Arithmetic (`+`, `-`, `*`, `/`) resolves to `number`.
- `COALESCE(a, b)`: If any operand is statically non-nullable, the result is marked non-nullable.
- Aggregate nullability: `COUNT(...)` is strictly `number`, while `SUM(...)` / `AVG(...)` evaluate to `number | null`.
- PostgreSQL arrays (`text[]`, `int4[]`) correctly emit TypeScript array types (`string[]`, `number[]`).

### 7. Sub-5ms Incremental Watch Mode (`-w, --watch`)
Watches your query and migration files. Query changes re-analyze only the changed file against the cached catalog in `< 5ms`. Migration edits re-apply DDL and refresh all queries automatically.

### 8. Sub-10ms Language Server Protocol (`sqltype lsp`)
An ultra-fast Language Server Protocol server operating over stdio:
- **Real-Time Syntax Diagnostics**: Maps PostgreSQL C-parser syntax errors directly to document line and column ranges via `cursorpos`.
- **Schema Validation**: Traverses AST `RangeVar` relations against the schema catalog, flagging nonexistent tables with red squiggly underlines.
- **Lock-Free Hot-Reloading**: Migration edits on disk reload the schema catalog atomically via `ArcSwap`, re-triggering instant validation across all open editor buffers.
- **Rich Markdown Hover**:
  - Hovering over `$N` displays parameter name, inferred PostgreSQL type, TypeScript type, and optional status.
  - Hovering over tables shows table name and column schemas with nullability.
  - Hovering over column references shows inferred PostgreSQL and TypeScript types.

---

## Quickstart

### 1. Installation

Install globally or as a project devDependency via npm:

```bash
# Using npm
npm install -D sqltype

# Using pnpm
pnpm add -D sqltype

# Using cargo
cargo install --path .
```

### 2. Directory Structure

```
my-project/
├── migrations/
│   ├── 001_create_users.sql
│   └── 002_create_posts.sql
├── queries/
│   ├── get_user_with_posts.sql
│   ├── find_posts_by_status.sql
│   ├── create_user.sql
│   └── delete_post.sql
└── src/
    └── types/
        └── (generated .ts files emitted here)
```

### 3. Example Queries

#### SELECT with CTE & Outer Joins:
```sql
-- queries/get_user_with_posts.sql
-- name: GetUserWithPosts
WITH active_users AS (
  SELECT id, email FROM users WHERE active = true
)
SELECT 
  au.id, 
  au.email, 
  p.title AS post_title,
  COUNT(c.id) AS comment_count
FROM active_users au
LEFT JOIN posts p ON p.user_id = au.id
LEFT JOIN comments c ON c.post_id = p.id
WHERE au.id = $1
GROUP BY au.id, au.email, p.title;
```

#### INSERT with RETURNING:
```sql
-- queries/create_user.sql
-- name: CreateUser
INSERT INTO users (email, first_name) 
VALUES ($1, $2) 
RETURNING id, created_at;
```

#### DELETE Mutation:
```sql
-- queries/delete_post.sql
-- name: DeletePost
DELETE FROM posts WHERE id = $1;
```

---

## Generated TypeScript Output

Zero runtime overhead. Pure TypeScript types and SQL strings:

```typescript
// Autogenerated by sqltype. DO NOT EDIT.

export interface GetUserWithPostsParams {
  id: string;
}

export interface GetUserWithPostsRow {
  id: string;
  email: string;
  post_title: string | null;
  comment_count: number;
}

export const getUserWithPostsSql = `
  WITH active_users AS (
    SELECT id, email FROM users WHERE active = true
  )
  SELECT 
    au.id, 
    au.email, 
    p.title AS post_title,
    COUNT(c.id) AS comment_count
  FROM active_users au
  LEFT JOIN posts p ON p.user_id = au.id
  LEFT JOIN comments c ON c.post_id = p.id
  WHERE au.id = $1
  GROUP BY au.id, au.email, p.title;
`;

export type GetUserWithPostsQuery = {
  sql: string;
  params: GetUserWithPostsParams;
  row: GetUserWithPostsRow;
};
```

For pure mutations without `RETURNING`:

```typescript
// Autogenerated by sqltype. DO NOT EDIT.

export interface DeletePostParams {
  id: string;
}

export const deletePostSql = `
  DELETE FROM posts WHERE id = $1;
`;

export type DeletePostQuery = {
  sql: string;
  params: DeletePostParams;
};
```

---

## CLI Reference

```
Usage: sqltype <COMMAND>

Commands:
  check     Validates all queries against the migration schema and exits with code 1 on type/column mismatch
  generate  Emits .ts files for all valid queries
  lsp       Starts the Language Server Protocol (LSP) server for real-time diagnostics and hover inspection
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

### `sqltype check`
Validate queries in CI/CD without generating files:
```bash
sqltype check --migrations ./migrations --queries ./queries --driver postgres
```

### `sqltype generate`
Generate TypeScript definitions:
```bash
# Standard generation
sqltype generate --migrations ./migrations --queries ./queries --out ./src/types

# With Bun driver profile
sqltype generate -m ./migrations -q ./queries -o ./src/types --driver bun

# With sub-5ms incremental watch mode
sqltype generate -m ./migrations -q ./queries -o ./src/types --watch
```

### `sqltype lsp`
Launch the Language Server Protocol server over stdio:
```bash
# Default migrations path (./migrations)
sqltype lsp

# Custom migrations directory
sqltype lsp --migrations ./custom/migrations
```

---

## Editor Configuration (LSP)

### VS Code
Configure using the generic LSP client or add to your `.vscode/settings.json`:
```json
{
  "sql.languageServer": {
    "command": "sqltype",
    "args": ["lsp", "--migrations", "./migrations"]
  }
}
```

### Neovim (`nvim-lspconfig`)
```lua
local lspconfig = require('lspconfig')
local configs = require('lspconfig.configs')

if not configs.sqltype then
  configs.sqltype = {
    default_config = {
      cmd = { 'sqltype', 'lsp', '--migrations', './migrations' },
      filetypes = { 'sql' },
      root_dir = lspconfig.util.root_pattern('migrations', '.git'),
      settings = {},
    },
  }
end

lspconfig.sqltype.setup({})
```

### Helix (`languages.toml`)
```toml
[language-server.sqltype]
command = "sqltype"
args = ["lsp", "--migrations", "./migrations"]

[[language]]
name = "sql"
language-servers = ["sqltype"]
```

---

## About the Author

**x7ssss** is an independent systems and compiler developer specializing in high-performance developer tooling, type inference systems, and low-latency database engines in Rust.

- **GitHub**: [@x7ssss](https://github.com/x7ssss)
- **Repository**: [x7ssss/sqltype](https://github.com/x7ssss/sqltype)
- **Email**: babadookmariqn@gmail.com

---

## License

MIT © [x7ssss](https://github.com/x7ssss)
