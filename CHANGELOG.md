# Changelog

All notable changes to `sqltype` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-09-23

### Added
- **PostgreSQL DML mutation analysis**: Full support for `INSERT`, `UPDATE`, and `DELETE` statement analysis and `ON CONFLICT` (UPSERT) with `EXCLUDED` pseudo-relation typing.
- **RETURNING clause projections**: Inference and code generation for `RETURNING` clauses, including wildcard expansion (`RETURNING *`) and computed expressions.
- **PostgreSQL array support**: Complete array operations typing including `ScalarArrayOpExpr` (`col = ANY($1)`), array constructors (`ARRAY[...]`), overlap (`&&`), containment (`@>`, `<@`), concatenation (`||`), and table unnesting (`unnest(...)`).
- **Developer CLI workflow commands**:
  - `sqltype init`: Auto-detects driver target (`postgres.js`, `pg`, `bun:sql`) from `package.json`/lockfiles and scaffolds config and demo migrations/queries.
  - `sqltype check`: Zero-disk-write in-memory schema validation with rustc-style colored diagnostics for CI.
  - `sqltype generate`: Rayon-powered multi-threaded query compilation with atomic tempfile disk writes.
  - `sqltype watch` (and `generate --watch`): 75ms debounced incremental file watcher with selective query re-analysis and fast 64-bit content hashing.
- **Test automation & resilience**: Added end-to-end integration test suites and smoke test automation (104 passing tests, 0 warnings).

## [0.2.0] - 2026-09-22

### Added
- **Inline TypeScript template extraction**: Extract queries from `.ts`, `.tsx`, and `.js` files using `sql`...`` tagged template literals and generate sibling `.sqltype.ts` companion files.
- **Advanced PostgreSQL expression inference**: Infer types for `CASE` expressions, `COALESCE`, PostgreSQL window functions (`OVER (...)`), and scalar subqueries (`SubLink`).
- **Relational query scopes & aliasing**: Implement relational table scopes, alias hiding, column ambiguity detection, and join-induced nullability propagation for `LEFT`, `RIGHT`, and `FULL` outer joins.
- **Common Table Expressions (CTEs)**: Support for standard `WITH` clauses and recursive CTEs (`WITH RECURSIVE`) with recursive term type unification.
- **SQL set operations**: Support for nested `UNION`, `UNION ALL`, `INTERSECT`, and `EXCEPT` operations with branch width validation and type lattice widening.
- **Deep JSON / JSONB inference**: Structured typing for `json_build_object`, `jsonb_build_object`, `json_agg`, `jsonb_agg`, and extraction operators (`->`, `->>`, `#>`, `#>>`).
- **Official VS Code extension**: Language client extension under `editors/vscode` providing diagnostics and hover inspection.
- **Comprehensive E2E test suite**: End-to-end integration tests in `tests/e2e_advanced_features.rs` validating joins, CTEs, set operations, JSON typing, and template extraction.

## [0.1.0] - 2026-09-22

### Added
- Initial release of `sqltype`.
- PostgreSQL DDL schema catalog and sequential migration parser (`CREATE TABLE`, `ALTER TABLE`, `CREATE TYPE AS ENUM`).
- Zero-runtime TypeScript code generation from SQL queries (`SELECT`, `INSERT`, `UPDATE`, `DELETE`).
- Type-safe query execution wrappers for `postgres.js`, `pg` (node-postgres), and `Bun.sql`.
- Language Server Protocol (LSP) server with hover inspection and real-time schema validation.
- Multi-platform npm wrapper distribution (`@x7ssss/sqltype`).
