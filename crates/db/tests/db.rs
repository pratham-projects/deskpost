use sd_db::Pool;

#[tokio::test]
async fn connect_works_and_migrate_is_idempotent() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let pool = sd_db::connect(&url, 2).await.unwrap();
    let one: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(one, 1);
    assert!(sd_db::connect("not a url", 1).await.is_err());
}

#[sqlx::test(migrations = "./migrations")]
async fn migrate_twice_is_a_no_op(pool: Pool) {
    sd_db::migrate(&pool).await.unwrap();
    sd_db::migrate(&pool).await.unwrap();
    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(applied, 6);
}

#[sqlx::test(migrations = "./migrations")]
async fn schema_has_no_aws_urls_or_plaintext_secrets(pool: Pool) {
    let cols: Vec<(String, String)> = sqlx::query_as(
        "SELECT table_name::text, column_name::text FROM information_schema.columns WHERE table_schema = 'public'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for (table, col) in &cols {
        let c = col.to_lowercase();
        assert!(
            !c.contains("ses_")
                && !c.contains("s3")
                && c != "storage_url"
                && c != "storage_provider",
            "{table}.{col}"
        );
        assert!(
            !(c.ends_with("password") || c == "api_key"),
            "plaintext secret column {table}.{col}"
        );
    }
    let has = |t: &str, c: &str| cols.iter().any(|(tt, cc)| tt == t && cc == c);
    for c in [
        "smtp_host",
        "smtp_port",
        "smtp_username",
        "smtp_password_encrypted",
        "imap_host",
        "imap_port",
        "imap_username",
        "imap_password_encrypted",
        "imap_folder",
        "imap_last_uid",
    ] {
        assert!(has("projects", c), "projects.{c}");
    }
    assert!(
        has("attachments", "storage_key")
            && has("refresh_tokens", "rotated_at")
            && has("refresh_families", "revoked_reason")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn every_timestamp_is_timestamptz_and_every_id_a_uuid(pool: Pool) {
    let bad: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT table_name::text, column_name::text, data_type::text FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name <> '_sqlx_migrations'
           AND ((data_type LIKE 'timestamp%' AND data_type <> 'timestamp with time zone')
                OR (column_name = 'id' AND data_type <> 'uuid')
                OR (column_name LIKE '%\\_id' AND column_name NOT IN ('storage_key', 'email_message_id', 'smtp_message_id', 'api_key_hash')
                    AND data_type NOT IN ('uuid') AND column_name NOT LIKE 'email\\_%' AND column_name NOT LIKE 'smtp\\_%'))",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(bad.is_empty(), "{bad:?}");
}
