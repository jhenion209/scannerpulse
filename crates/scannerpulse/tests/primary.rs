//! A database on more than one node: the hosts are read from the settings,
//! the primary is the node that answers and is not a standby, and a pool
//! moves to it.

use std::time::Duration;

use scannerpulse::primary::{self, Node};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

fn node(host: &str, port: u16) -> Node {
    Node {
        host: host.into(),
        port,
    }
}

#[test]
fn the_nodes_are_hosts_with_their_ports() {
    assert_eq!(
        primary::nodes("db1.example.org:5433 [2001:db8::40]:5432  db2.example.org").unwrap(),
        [
            node("db1.example.org", 5433),
            node("2001:db8::40", 5432),
            node("db2.example.org", 5432),
        ]
    );
    assert!(primary::nodes("").unwrap().is_empty());
    for bad in ["db1.example.org:0", "db1.example.org:port", "[2001:db8::40"] {
        let error = primary::nodes(bad).unwrap_err();
        assert!(
            error.contains("SCANNERPULSE_DATABASE_HOSTS"),
            "{bad}: {error}"
        );
    }
}

/// The test database's options on another host and port.
fn moved(options: &PgConnectOptions, to: &Node) -> PgConnectOptions {
    options.clone().host(&to.host).port(to.port)
}

#[sqlx::test(migrations = "./migrations")]
async fn the_primary_is_the_node_that_answers_and_is_not_a_standby(pool: PgPool) {
    let options = (*pool.connect_options()).clone();
    let here = node(options.get_host(), options.get_port());
    // Nothing listens on port 1.
    let nodes = vec![node("127.0.0.1", 1), here];
    assert_eq!(primary::find(&options, &nodes).await, Some(1));
    assert_eq!(primary::find(&options, &nodes[..1]).await, None);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_pool_moves_to_the_primary(pool: PgPool) {
    let options = (*pool.connect_options()).clone();
    let here = node(options.get_host(), options.get_port());
    // Built on a node that does not answer, the pool finds the one that does.
    let on = primary::pool(
        &moved(&options, &node("127.0.0.1", 1)),
        &[node("127.0.0.1", 1), here],
        PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(2)),
    );
    let mut answered = false;
    for _ in 0..40 {
        if sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&on)
            .await
            .is_ok()
        {
            answered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(answered, "the pool reached the node that answers");
    assert_eq!(on.connect_options().get_port(), options.get_port());
}

/// A relay to the database that can go silent: it then passes nothing
/// either way but keeps both connections open, as a machine that lost its
/// power does.
async fn relay(host: String, port: u16) -> (u16, std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap().port();
    let silent = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let quiet = silent.clone();
    tokio::spawn(async move {
        while let Ok((inbound, _)) = listener.accept().await {
            let Ok(outbound) = tokio::net::TcpStream::connect((host.as_str(), port)).await else {
                continue;
            };
            let (ri, wi) = inbound.into_split();
            let (ro, wo) = outbound.into_split();
            for (mut from, mut to) in [(ri, wo), (ro, wi)].map(|(r, w)| (r, w)) {
                let quiet = quiet.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    while let Ok(n @ 1..) = from.read(&mut buf).await {
                        if quiet.load(Ordering::SeqCst) {
                            std::future::pending::<()>().await;
                        }
                        if to.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        }
    });
    (local, silent)
}

#[sqlx::test(migrations = "./migrations")]
async fn a_listener_notices_a_database_that_went_silent(pool: PgPool) {
    use std::sync::atomic::Ordering;
    let options = (*pool.connect_options()).clone();
    let (port, silent) = relay(options.get_host().to_string(), options.get_port()).await;
    let through = PgPoolOptions::new()
        .max_connections(2)
        .connect_lazy_with(options.clone().host("127.0.0.1").port(port));
    let mut listener = sqlx::postgres::PgListener::connect_with(&through)
        .await
        .unwrap();
    listener.listen("scannerpulse_test").await.unwrap();
    silent.store(true, Ordering::SeqCst);
    // Without a bound, it would wait on the silent connection for ever.
    let heard = tokio::time::timeout(
        Duration::from_secs(10),
        primary::listened(&mut listener, Duration::from_millis(500)),
    )
    .await;
    assert!(matches!(heard, Ok(Err(_))), "{heard:?}");
}
