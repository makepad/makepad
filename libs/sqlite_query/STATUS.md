# makepad-sqlite — status

A clean-room Rust engine for the **SQLite 3 on-disk format**. The byte layout is
not ours: header, b-tree pages, record format, overflow chains, `sqlite_master`,
the WAL and the rollback journal are implemented from
<https://www.sqlite.org/fileformat.html> so the system `sqlite3` CLI reads (and,
from P2, writes) the very same files. Only the Rust implementation — pager,
cursors, SQL front end, planner, executor — is ours.

Crate directory: `libs/sqlite_query`, package `makepad-sqlite`, binary `sqlq`.

## Phase state

| Phase | Scope | State |
|---|---|---|
| P0 | Hardened reader: header validation, page cache, bounds-checked b-tree, overflow, WAL snapshot reads, DDL parsing, integrity checker | **done** |
| P1 | Read-only SQL: tokenizer, parser, planner (rowid/index seeks, join order), executor | **done** |
| P2 | Writes: allocator/freelist, b-tree insert/split/delete, rollback journal, locking, DML/DDL, crash + concurrency + differential-DML hardening | **done** |
| P3 | Cutover: the asset store's `Db`/`Stmt` wrapper behind the `own-db` feature | **done** |

## Guarantees

- **No panics on bad bytes.** Every page-header field, cell pointer, cell
  payload offset and record offset is bounds-checked; failures surface as
  `Error::Corrupt(reason)`. A byte-flip fuzz over CLI-built databases scans
  every table of every mutant (`tests/reader.rs`).
- **Reads what SQLite writes.** Full scans are compared row for row with the
  `sqlite3` CLI at page sizes 512 / 1024 / 4096 / 65536, over b-trees deeper
  than two levels, with multi-page overflow payloads, NULLs, REALs, BLOBs and
  text.
- **Same answers as SQLite.** The asset store's hot queries, five "ask the
  library" LLM queries and 400 generated queries per run are compared
  row-for-row and value-for-value (storage class included) against the CLI.
- **WAL-aware reads.** Frames are validated (salts plus the documented
  Fibonacci checksum chain, byte order from the magic's low bit) and the newest
  *committed* snapshot is served; a torn or truncated tail is ignored. Verified
  against a database whose writer was SIGKILLed mid-session and against the
  live asset-store transport DB (282 frames).
- **Read-only handles stay read-only.** `Database` has no write path at all;
  writing needs a `Connection`, and a test asserts a `Database` session leaves
  the file bytes untouched.
- **Crash safety.** Every durable write step of a commit is a crash point in the
  test suite: a child process is aborted at each of them (29 for the fixture
  transaction), and after recovery the database is always either fully before or
  fully after — checked by our integrity checker *and* by
  `sqlite3 PRAGMA integrity_check`. SQLite recovers our hot journal on its own,
  and we recover SQLite's.
- **One writer, many readers.** Byte-range locks live exactly where SQLite puts
  them, so the `sqlite3` CLI is refused (SQLITE_BUSY) while we hold a write
  transaction, and we are refused while it holds one. Connections inside one
  process coordinate through a registry, because POSIX locks are per-process.
  Locks are released between autocommit statements, like SQLite.
- **Bounded work.** Every statement runs under `Limits { max_rows, max_steps }`;
  exceeding either is `Error::Budget`, never an OOM.

## Supported SQL

| Feature | State |
|---|---|
| `SELECT [DISTINCT]`, `*`, `t.*`, `expr [[AS] alias]` | yes |
| `FROM` one table, aliases, comma joins, `INNER`/`LEFT [OUTER]`/`CROSS JOIN … ON/USING` | yes |
| Subqueries: `FROM (SELECT …)`, `IN (SELECT …)`, `EXISTS`, scalar `(SELECT …)`, correlated | yes |
| `WHERE` with `= == != <> < <= > >= IS [NOT] IS NULL IN BETWEEN LIKE GLOB AND OR NOT`, parens | yes |
| Literals (int, real, text, `x'..'` blob, NULL, TRUE/FALSE), params `?`, `?N`, `:name`, `@name`, `$name` (bare `?` numbered in text order, like SQLite) | yes |
| `GROUP BY` + `HAVING` (result aliases allowed), `ORDER BY` multi-key ASC/DESC/NULLS, `LIMIT`/`OFFSET` (incl. `LIMIT o, n`) | yes |
| Aggregates: `COUNT(*)`, `COUNT(x)`, `COUNT(DISTINCT x)`, `MIN`, `MAX`, `SUM`, `TOTAL`, `AVG`, `GROUP_CONCAT` | yes |
| Scalars: `COALESCE IFNULL NULLIF LENGTH LOWER UPPER ABS SUBSTR INSTR REPLACE TRIM LTRIM RTRIM HEX QUOTE TYPEOF ROUND MIN MAX IIF UNICODE CHAR` | yes |
| `CASE`, `CAST`, `COLLATE` (BINARY/NOCASE/RTRIM) | yes |
| `UNION [ALL]`, `INTERSECT`, `EXCEPT` | yes |
| Type affinity, SQLite sort order, three-valued logic, integral-REAL storage fix-up | yes |
| `CREATE TABLE`/`CREATE INDEX` parsing incl. automatic index reconstruction | yes |
| `INSERT` (VALUES / SELECT / DEFAULT VALUES), `OR IGNORE`, `OR REPLACE`, `REPLACE INTO` | yes |
| `INSERT … ON CONFLICT(cols) DO NOTHING / DO UPDATE SET … [WHERE …]` | yes |
| `UPDATE [OR IGNORE/REPLACE] … SET … [WHERE …]`, `DELETE FROM … [WHERE …]` | yes |
| `CREATE TABLE [IF NOT EXISTS]`, `CREATE [UNIQUE] INDEX [IF NOT EXISTS]`, `DROP TABLE/INDEX [IF EXISTS]` | yes |
| `ALTER TABLE … ADD [COLUMN] …`, `ALTER TABLE … RENAME TO …` | yes |
| `BEGIN [DEFERRED|IMMEDIATE|EXCLUSIVE]`, `COMMIT`/`END`, `ROLLBACK`, autocommit | yes |
| NOT NULL, PRIMARY KEY, UNIQUE and CHECK enforcement; `changes()` | yes |
| `PRAGMA user_version / schema_version / page_size / page_count / freelist_count / journal_mode / table_info / integrity_check`; `synchronous`, `foreign_keys`, `wal_autocheckpoint`, … accepted and ignored | yes |
| Window functions, CTEs (`WITH`), `RIGHT`/`FULL`/`NATURAL JOIN`, `printf()`, date/time functions | no |
| `EXPLAIN`, `VALUES(...)` as a statement, `ATTACH`, foreign-key enforcement | no |
| Triggers and views: not run, and a table with a trigger is refused for writes | no (refused, never ignored) |
| `SAVEPOINT` / nested transactions | no |

## Planner

Rule-based, one pass, and visible through `Statement::explain()`:

1. rowid equality → single-row seek
2. unique index full equality → seek, stops after one row
3. the index matching the most equality columns, optionally with a range on the
   next column (this is what the store's keyset pages use)
4. rowid range
5. full scan

Plus: greedy join ordering (the most constrained table drives the nested loop),
`ORDER BY` elimination when the access path already returns that order, and
column projection — a row is decoded only as far as the columns the statement
reads, so `COUNT(*)` decodes nothing.

## Storage and durability

Both of SQLite's journalling modes are implemented, and `PRAGMA journal_mode`
switches between them in either direction.

- **Rollback journal** (`journal_mode=delete`): original page images to
  `<db>-journal`, sync, write the database, sync, delete the journal — the
  deletion is the commit point.
- **WAL** (`journal_mode=wal`): commits append frames to `<db>-wal` with the
  documented checksum chain and a commit frame carrying the database size; one
  sync per commit, and the database file is only touched at a checkpoint
  (automatic every 1000 frames, or `PRAGMA wal_checkpoint`). The wal-index is
  kept in this process's memory rather than in a shared `-shm`, which is what
  SQLite itself does in exclusive locking mode. Ownership is per statement, not
  per connection: the `-shm` locking bytes are held only while a statement runs
  (shared for a reader, exclusive for a writer) and the wal-index header is
  zeroed on release — SQLite's documented signal to rebuild the index from the
  log. A `sqlite3` process therefore reads every frame we appended while we are
  idle, and we read frames it appended, both verified in
  `tests/concurrency.rs`; during our write it gets SQLITE_BUSY rather than a
  stale read.
- Durability follows SQLite's own choice: a plain `fsync()`, with the macOS full
  drive barrier (`F_FULLFSYNC`) available through `PRAGMA fullfsync=ON`. Rust's
  `File::sync_all` always asks for the barrier, which is 25x slower and not what
  a database shared with SQLite should do by default.
- Pages are rebuilt whole on modification (no free-block bookkeeping), split on
  overflow, and unlinked when they empty; freed pages go on the freelist and are
  reused before the file grows.
- The root page of a b-tree never moves, so `sqlite_master` stays valid.

## Using it from the asset store

`libs/asset/store` carries both copies of its `Db`/`Stmt` wrapper on this engine
behind a default-off feature:

```
cargo test -p makepad-asset-store --features own-db
```

The wrapper is a drop-in: same methods, same `ServerError::Db { op, code }`
mapping (5 busy, 11 corrupt, 19 constraint, …), no FFI and no `unsafe`. The
store's schema, migrations (including the `CREATE`/copy/`DROP`/`RENAME` table
rebuild), constraints, transactions and `PRAGMA user_version` all run unchanged,
and the catalog stays in WAL mode as the store requires.

**All 201 store tests pass under the feature** — the same 201 that pass against
libsqlite3, including the HTTP/e2e suite in `tests/http/` (restored by the
`[[test]]` entries in the store's `Cargo.toml`), the fault-injection tests that
poke the catalog with raw libsqlite3 while the server holds it, and the
migration tests.

## Known gaps

- `WITHOUT ROWID` tables are rejected (`TableInfo::unsupported`); their b-tree is
  keyed by the PRIMARY KEY record rather than a rowid. The asset catalog has
  none.
- Virtual tables, generated columns and `CREATE TABLE … AS SELECT` are flagged
  unsupported per table; the rest of the schema still loads.
- No covering-index scans: a query that only reads indexed columns still visits
  the table. This is most of the remaining gap on `COUNT(*)` (SQLite counts the
  smallest index instead).
- Cursors are forward-only, so `ORDER BY … DESC` sorts in memory rather than
  walking the index backwards.
- Row order for a query *without* `ORDER BY` is unspecified in SQL and does
  differ from SQLite (SQLite may scan a covering index, we scan the table).
- GROUP BY buckets compare keys with BINARY collation even when the column
  declares NOCASE.
- Bare columns in a GROUP BY follow SQLite's rule (a row of the group, and the
  min/max row when there is exactly one such aggregate), which is defined
  behaviour but not portable SQL.
- TEXT is decoded strictly (invalid UTF-8 is an error) for the SQL engine, and
  leniently for the mbtiles/GeoPackage reader, which must keep opening
  historical archives.
- Collations: BINARY, NOCASE, RTRIM. An index whose collation differs from the
  comparison's is not used for a seek (correct, but a missed optimization).
- The WAL index is rebuilt by scanning the whole log on open/refresh; large WALs
  pay for that scan each time.
- Pages are not merged when they merely become sparse — only fully empty pages
  are freed — so a heavily deleted table reclaims less space than SQLite would
  until those pages empty. `VACUUM` is not implemented.
- No `SAVEPOINT`, so a failed statement inside an explicit transaction does not
  roll back just that statement; the caller decides whether to roll the whole
  transaction back.
- Foreign keys are parsed and ignored (as SQLite does with
  `foreign_keys=OFF`).
- Triggers are not run. A table that a trigger names becomes **read-only**:
  a write is refused rather than silently skipping what the trigger would have
  done. Views are likewise not resolved.
- `-shm` is never read or written; readers reconstruct their own index, which is
  safe for reading but is why the P2 writer starts with the rollback journal.

## Perf snapshot

macOS, release build, warm OS cache, process startup subtracted (~6.7 ms for
`sqlq`, ~9 ms for `sqlite3`); the machine was busy, so sub-millisecond figures
are noisy.

| Query (3.9k-row asset catalog) | ours | `sqlite3` |
|---|---|---|
| alias resolve (unique index seek) | 0.3 ms | ~0 ms |
| keyset page of 25 (index + no sort) | 0.1 ms | ~0 ms |
| term search join + LIMIT 50 | 0.8 ms | ~0 ms |
| `COUNT(*)` (full scan) | 1.8 ms | ~0.2 ms |
| `GROUP BY kind` | 2.2 ms | ~1 ms |
| correlated `EXISTS` label filter | 15 ms | 8 ms |

| Query (1M-row table, 525 MB) | ours | `sqlite3` |
|---|---|---|
| keyset page of 25 | 0.3 ms | ~0 ms |
| `COUNT(*)` | 153 ms | 6 ms (counts an index) |
| `GROUP BY namespace` | 305 ms | 48 ms |
| `COUNT(*), SUM(LENGTH(title))` | 442 ms | 99 ms |

Index-driven queries — everything the store and the LLM route actually run in a
loop — are at parity. Full scans are 3-4x slower: we materialize a `Value` per
column where SQLite decodes lazily into registers.

Writes (release build): 1,000,000 rows with a secondary index and 20 KB overflow
payloads insert in 33 s inside one transaction (~30k rows/s), and the resulting
file passes `PRAGMA integrity_check`. SQLite is roughly an order of magnitude
faster at bulk insert; each of our inserts re-encodes and rewrites whole pages
where SQLite edits them in place.

## Tests

`cargo test -p makepad-sqlite` — 97 tests:

- `src/**` unit tests (41): varint/record round trips, sort order, affinity,
  DDL parsing incl. automatic-index numbering verified against the CLI,
  tokenizer, parser precedence and error paths, journal rollback, file locking.
- `tests/reader.rs` (11): CLI-built fixtures at four page sizes, deep b-trees,
  overflow, rowid and index seeks, WAL snapshots, torn WAL tails, byte-flip
  fuzz, header validation, integrity check vs `PRAGMA integrity_check`, the live
  catalog copy.
- `tests/query.rs` (9): the store's hot queries and LLM questions against the
  real catalog, plan assertions (index seeks, join order, no sort), semantics on
  a controlled fixture, parameter numbering, budgets, error paths, read-only
  proof.
- `tests/fuzz_sql.rs` (2): a checked-in corpus (`tests/corpus/queries.txt`) plus
  400 generated queries per run, both differentially compared with `sqlite3`.
- `tests/write.rs` (7): b-tree inserts through splits and overflow, deletes that
  free pages, freelist reuse, index maintenance, rollback — every file checked
  by `PRAGMA integrity_check` and by our own strict page accounting.
- `tests/dml.rs` (11): the SQL write layer end to end, including the store's v8
  schema round-tripping in both directions, its table-rebuild migration, and a
  WAL database staying in WAL mode across both conversions.
- `tests/fuzz_dml.rs` (2): 300 random INSERT/UPDATE/DELETE/upsert statements
  applied to our engine and to a CLI-driven copy, compared after every batch.
- `tests/crash.rs` (4): abort at every durable write step of a commit — in both
  journalling modes — recover, and require the database to be exactly the old
  or the new state, with both integrity checkers agreeing.
- `tests/search_query.rs` (1): the asset store's real search statement, which
  puts a compound subquery in FROM, a join onto it, an aggregate over a CASE,
  GROUP BY/HAVING COUNT(DISTINCT), bare columns and ORDER BY on an alias into
  one query, compared with the CLI.
- `tests/concurrency.rs` (5): in-process writer exclusion, reader isolation,
  both directions of contention with the `sqlite3` CLI, and a WAL database
  shared with the CLI while this engine stays open.
- `tests/scale.rs` (2): 20k rows by default (1M with `MAKEPAD_SQLITE_BIG=1`)
  with deep b-trees, 20 KB overflow payloads and index rebalancing.

Tests that need the `sqlite3` CLI skip themselves when it is missing; tests that
read the asset-store copy skip when it is absent.
