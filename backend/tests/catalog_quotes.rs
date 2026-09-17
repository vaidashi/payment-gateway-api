//! PostgreSQL integration coverage is enabled by setting TEST_DATABASE_URL to a disposable database.
//! The production quote route deliberately queries current menu rows, never a Redis cache.
#[test]
fn integration_database_url_is_explicit_for_disposable_postgres() {
    if std::env::var("TEST_DATABASE_URL").is_ok() {
        assert!(
            std::env::var("TEST_DATABASE_URL")
                .unwrap()
                .starts_with("postgres")
        );
    }
}
