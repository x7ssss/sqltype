use sqltype::cli::GenerateArgs;
use sqltype::commands::watch::run_watch_loop;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[test]
fn test_watch_incremental_and_debouncing_workflow() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Setup initial project structure
    let migrations_dir = root.join("migrations");
    let queries_dir = root.join("queries");
    fs::create_dir_all(&migrations_dir).unwrap();
    fs::create_dir_all(&queries_dir).unwrap();

    let config_toml = r#"
[migrations]
directory = "migrations"
pattern = "*.sql"

[queries]
sql_files = ["queries/**/*.sql"]
inline_ts = true

[driver]
name = "postgres.js"
"#;
    fs::write(root.join("sqltype.toml"), config_toml).unwrap();

    let init_sql = "CREATE TABLE users (id UUID PRIMARY KEY, name TEXT NOT NULL);";
    fs::write(migrations_dir.join("001_init.sql"), init_sql).unwrap();

    let query_sql = "-- name: FindUser\nSELECT id, name FROM users WHERE id = $1;";
    let find_user_path = queries_dir.join("find_user.sql");
    fs::write(&find_user_path, query_sql).unwrap();

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = Arc::clone(&running);
    let root_buf = root.to_path_buf();

    let handle = thread::spawn(move || {
        let args = GenerateArgs {
            migrations: None,
            queries: None,
            out: None,
            watch: true,
            driver: None,
            wrappers: false,
            threads: None,
            declaration_only: false,
        };
        run_watch_loop(&args, &root_buf, None, running_clone)
    });

    // 1. Initial compilation should generate companion find_user.sql.ts
    let companion = queries_dir.join("find_user.sql.ts");
    let start = Instant::now();
    while !companion.exists() && start.elapsed() < Duration::from_secs(3) {
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        companion.exists(),
        "Initial companion file must be generated"
    );
    let content = fs::read_to_string(&companion).unwrap();
    assert!(content.contains("export interface FindUserRow"));

    // 2. Incremental compilation: add a new query file
    let list_users_path = queries_dir.join("list_users.sql");
    let list_companion = queries_dir.join("list_users.sql.ts");
    fs::write(
        &list_users_path,
        "-- name: ListUsers\nSELECT id, name FROM users;",
    )
    .unwrap();

    let start = Instant::now();
    while !list_companion.exists() && start.elapsed() < Duration::from_secs(3) {
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        list_companion.exists(),
        "New query companion file must be generated incrementally"
    );
    let list_content = fs::read_to_string(&list_companion).unwrap();
    assert!(list_content.contains("export interface ListUsersRow"));

    // 3. Graceful syntax error handling: write invalid query
    let invalid_path = queries_dir.join("invalid_temp.sql");
    fs::write(&invalid_path, "SELECT invalid_col FROM non_existent_table;").unwrap();

    // Sleep to allow debounce to process invalid query
    thread::sleep(Duration::from_millis(200));

    // Watcher thread must still be alive and running
    assert!(
        !handle.is_finished(),
        "Watcher must remain active even after query validation errors"
    );

    // 4. Migration schema rebuild: add posts table
    let posts_sql =
        "CREATE TABLE posts (id UUID PRIMARY KEY, user_id UUID NOT NULL, title TEXT NOT NULL);";
    fs::write(migrations_dir.join("002_posts.sql"), posts_sql).unwrap();

    // Write a query referencing the newly added table
    let posts_query_path = queries_dir.join("get_posts.sql");
    let posts_companion = queries_dir.join("get_posts.sql.ts");
    fs::write(
        &posts_query_path,
        "-- name: GetPosts\nSELECT id, title FROM posts;",
    )
    .unwrap();

    let start = Instant::now();
    while !posts_companion.exists() && start.elapsed() < Duration::from_secs(3) {
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        posts_companion.exists(),
        "Posts query companion must be generated after migration rebuild"
    );
    let posts_content = fs::read_to_string(&posts_companion).unwrap();
    assert!(posts_content.contains("export interface GetPostsRow"));
    assert!(posts_content.contains("title: string;"));

    // 5. Clean shutdown
    running.store(false, Ordering::SeqCst);
    let res = handle.join().expect("watch thread panicked");
    assert!(res.is_ok(), "watch loop should exit cleanly");
}
