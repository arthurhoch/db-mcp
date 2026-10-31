#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;

use serde_json::json;

#[tokio::test]
async fn returning_rows_does_not_turn_a_write_into_a_read() {
    let fixture = common::Fixture::sqlite();
    let service = fixture.service();
    common::succeeded(
        &common::execute(&service, "ddl", "CREATE TABLE returning_test(id INTEGER)").await,
    );
    let result = common::execute(
        &service,
        "rw",
        "INSERT INTO returning_test VALUES(7) RETURNING id",
    )
    .await;
    common::succeeded(&result);
    assert_eq!(result["effect"], "committed");
    assert_eq!(result["rows"], json!([[7]]));
    let denied = common::execute(
        &service,
        "ro",
        "INSERT INTO returning_test VALUES(8) RETURNING id",
    )
    .await;
    assert_eq!(denied["error"]["code"], "READ_ONLY_VIOLATION");
}
