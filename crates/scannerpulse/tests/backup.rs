use std::time::Duration;

use scannerpulse::backup;
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[sqlx::test(migrations = "./migrations")]
async fn each_kind_keeps_the_time_of_its_newest_success(pool: PgPool) -> sqlx::Result<()> {
    assert!(backup::newest(&pool).await?.is_empty());
    backup::record(&pool, "dump").await?;
    backup::record(&pool, "pgbackrest").await?;
    let first = backup::newest(&pool).await?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    backup::record(&pool, "dump").await?;
    let newest = backup::newest(&pool).await?;
    let kinds: Vec<&str> = newest.iter().map(|(kind, _)| kind.as_str()).collect();
    assert_eq!(kinds, ["dump", "pgbackrest"]);
    assert!(newest[0].1 > first[0].1, "the dump's time moved on");
    assert_eq!(newest[1].1, first[1].1);
    Ok(())
}

#[tokio::test]
async fn a_push_url_is_called_after_a_success() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0u8; 1024];
        let n = stream.read(&mut request).await.unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
            .await
            .unwrap();
        String::from_utf8_lossy(&request[..n]).into_owned()
    });
    backup::push(&format!(
        "http://{address}/api/push/a1b2c3?status=up&msg=OK"
    ))
    .await
    .unwrap();
    let request = server.await.unwrap();
    assert!(
        request.starts_with("GET /api/push/a1b2c3?status=up&msg=OK HTTP/1.1"),
        "{request}"
    );
}

#[tokio::test]
async fn a_push_that_is_not_answered_says_so() {
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = free.local_addr().unwrap();
    drop(free);
    let error = backup::push(&format!("http://{address}/api/push/x"))
        .await
        .unwrap_err();
    assert!(error.contains(&address.to_string()), "{error}");
}
