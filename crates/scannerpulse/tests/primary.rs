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
