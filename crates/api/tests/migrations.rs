use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn inbox_list_has_a_recency_index(pool: PgPool) {
    let def: String = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes WHERE indexname = 'tickets_project_updated_idx'",
    )
    .fetch_one(&pool)
    .await
    .expect("index exists");
    assert!(
        def.contains("project_id, updated_at DESC, id DESC"),
        "{def}"
    );
}
