use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn test_cli_init_workflow() {
    let binary = env!("CARGO_BIN_EXE_sqltype");
    let dir = tempdir().unwrap();
    let root = dir.path();

    // 1. Run `sqltype init` with toml format and explicit driver
    let init_status = Command::new(binary)
        .arg("init")
        .arg(root)
        .arg("--format")
        .arg("toml")
        .arg("--driver")
        .arg("pg")
        .status()
        .expect("failed to execute sqltype init");

    assert!(init_status.success(), "sqltype init should succeed");
    assert!(
        root.join("sqltype.toml").exists(),
        "sqltype.toml must exist"
    );
    assert!(
        root.join("migrations/001_init.sql").exists(),
        "migrations/001_init.sql must exist"
    );
    assert!(
        root.join("queries/find_users_by_tag.sql").exists(),
        "queries/find_users_by_tag.sql must exist"
    );

    let toml_str = fs::read_to_string(root.join("sqltype.toml")).unwrap();
    assert!(toml_str.contains("name = \"pg\""));

    // 2. Running again without --force should fail
    let duplicate_status = Command::new(binary)
        .arg("init")
        .arg(root)
        .status()
        .expect("failed to execute duplicate sqltype init");
    assert!(
        !duplicate_status.success(),
        "init without force should fail"
    );

    // 3. Running with --force and json format should succeed
    let force_status = Command::new(binary)
        .arg("init")
        .arg(root)
        .arg("--format")
        .arg("json")
        .arg("--force")
        .status()
        .expect("failed to execute sqltype init --force");
    assert!(force_status.success(), "init with force should succeed");
    assert!(
        root.join("sqltype.json").exists(),
        "sqltype.json must exist"
    );
}

#[test]
fn test_cli_check_and_generate_workflows() {
    let binary = env!("CARGO_BIN_EXE_sqltype");
    let dir = tempdir().unwrap();
    let root = dir.path();

    // 1. Initialize project
    let init_status = Command::new(binary)
        .arg("init")
        .arg(root)
        .arg("--driver")
        .arg("postgres.js")
        .status()
        .expect("failed to execute init");
    assert!(init_status.success());

    // 2. Run `sqltype check` using auto-discovered sqltype.toml (no -m or -q passed)
    let check_status = Command::new(binary)
        .current_dir(root)
        .arg("check")
        .status()
        .expect("failed to execute sqltype check");
    assert!(
        check_status.success(),
        "check should succeed on scaffolded demo query"
    );

    // 3. Test check with --json flag
    let check_json_output = Command::new(binary)
        .current_dir(root)
        .arg("check")
        .arg("--json")
        .output()
        .expect("failed to execute check --json");
    assert!(check_json_output.status.success());
    let json_str = String::from_utf8(check_json_output.stdout).unwrap();
    let val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(val["success"], true);
    assert_eq!(val["error_count"], 0);

    // 4. Run `sqltype generate` using auto-discovered config with --threads
    let gen_status = Command::new(binary)
        .current_dir(root)
        .arg("generate")
        .arg("--threads")
        .arg("2")
        .status()
        .expect("failed to execute generate");
    assert!(gen_status.success(), "generate should succeed");

    // Generated companion file queries/find_users_by_tag.ts must exist
    let generated_ts = root.join("queries/find_users_by_tag.ts");
    assert!(
        generated_ts.exists(),
        "find_users_by_tag.ts must be generated"
    );
    let ts_content = fs::read_to_string(&generated_ts).unwrap();
    assert!(ts_content.contains("export interface FindUsersByTagRow"));
    assert!(ts_content.contains("tags: Array<string>;"));

    // 5. Add an invalid query to test failure and --fail-fast
    let bad_query_file = root.join("queries/invalid.sql");
    fs::write(
        &bad_query_file,
        "SELECT nonexistent_column FROM nonexistent_table;",
    )
    .unwrap();

    let fail_check_status = Command::new(binary)
        .current_dir(root)
        .arg("check")
        .arg("--fail-fast")
        .status()
        .expect("failed to execute check on invalid query");
    assert!(
        !fail_check_status.success(),
        "check should fail on invalid query"
    );

    // Verify JSON output reports error
    let fail_json_output = Command::new(binary)
        .current_dir(root)
        .arg("check")
        .arg("--json")
        .output()
        .expect("failed to execute check --json on invalid query");
    assert!(!fail_json_output.status.success());
    let fail_json_str = String::from_utf8(fail_json_output.stdout).unwrap();
    let fail_val: serde_json::Value = serde_json::from_str(&fail_json_str).unwrap();
    assert_eq!(fail_val["success"], false);
    assert!(fail_val["error_count"].as_u64().unwrap() >= 1);

    // 6. Test centralized output mode
    fs::remove_file(&bad_query_file).unwrap();
    let centralized_toml = r#"
[migrations]
directory = "migrations"
pattern = "*.sql"

[queries]
sql_files = ["queries/**/*.sql"]
inline_ts = true

[driver]
name = "postgres.js"

[output]
mode = "centralized"
file_path = "src/generated/types.ts"
declaration_only = false
"#;
    fs::write(root.join("sqltype.toml"), centralized_toml).unwrap();

    let gen_central_status = Command::new(binary)
        .current_dir(root)
        .arg("generate")
        .status()
        .expect("failed to execute generate in centralized mode");
    assert!(
        gen_central_status.success(),
        "centralized generate should succeed"
    );

    let centralized_file = root.join("src/generated/types.ts");
    assert!(
        centralized_file.exists(),
        "src/generated/types.ts must be generated"
    );
    let central_content = fs::read_to_string(&centralized_file).unwrap();
    assert!(central_content.contains("export interface FindUsersByTagRow"));
}
