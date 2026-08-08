use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_postgres::{Config, NoTls};

use super::{CONNECT_TIMEOUT, ConnectError, Open, Pool};

/// A DSN nothing listens on; these tests never open a connection.
const UNUSED: &str = "host=/nonexistent dbname=kronika";

fn pool() -> Pool {
    Pool::new(UNUSED).expect("the DSN parses")
}

#[test]
fn a_fresh_pool_holds_no_connection_generation() {
    let mut pool = pool();
    assert_eq!(pool.generation(), None);
    assert!(pool.session_for_generation(1).is_none());
    assert!(pool.open.is_none(), "the generation guard must not connect");
}

#[test]
fn the_connect_deadline_is_small_and_finite() {
    assert!(CONNECT_TIMEOUT > Duration::ZERO);
    assert!(CONNECT_TIMEOUT <= Duration::from_secs(10));
}

#[test]
fn timeout_classification_is_explicit() {
    assert!(ConnectError::Timeout.is_timeout());
}

#[test]
fn closing_a_pool_that_never_opened_is_not_an_error() {
    let mut pool = pool();
    pool.close();
    assert_eq!(pool.generation(), None);
}

#[test]
fn a_dsn_that_is_not_a_connection_string_is_rejected() {
    assert!(Pool::new("host=").is_err());
}

#[test]
fn a_url_dsn_is_accepted_as_well_as_keywords() {
    let pool = Pool::new("postgres://reader@example:5433/appdb").expect("the URL parses");
    assert_eq!(pool.config.get_dbname(), Some("appdb"));
    assert_eq!(pool.database_label(), "appdb");
}

#[test]
fn another_database_keeps_server_configuration_without_an_open_session() {
    let mut primary = pool();
    primary.remember_resolved_user("actual_role");
    let other = primary.on_database("payments");
    assert_eq!(other.config.get_dbname(), Some("payments"));
    assert_eq!(other.generation(), None);
    assert!(other.connection_label(0).starts_with("actual_role@"));
}

#[test]
fn labels_use_neutral_defaults_until_the_generation_probe_resolves_them() {
    let mut pool = Pool::new("host=db.example").expect("the DSN parses");
    assert_eq!(pool.database_label(), "server-default");
    assert_eq!(pool.connection_label(3), "server-default@db.example:5432");
    pool.remember_resolved_user("monitor");
    assert_eq!(pool.connection_label(3), "monitor@db.example:5432");
    assert!(
        pool.config
            .get_application_name()
            .is_some_and(|name| name.starts_with("kronika-collector-"))
    );
}

#[tokio::test]
async fn a_dead_client_is_not_reported_as_a_reusable_generation() {
    let (client_io, mut server_io) = tokio::io::duplex(4_096);
    let (close_server, wait_to_close) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let startup_len = server_io.read_u32().await.expect("read startup length");
        let mut startup = vec![0; usize::try_from(startup_len - 4).expect("startup fits")];
        server_io
            .read_exact(&mut startup)
            .await
            .expect("read startup body");
        server_io.write_u8(b'R').await.expect("write auth tag");
        server_io.write_u32(8).await.expect("write auth length");
        server_io.write_i32(0).await.expect("write auth-ok code");
        server_io.write_u8(b'Z').await.expect("write ready tag");
        server_io.write_u32(5).await.expect("write ready length");
        server_io.write_u8(b'I').await.expect("write idle status");
        server_io.flush().await.expect("flush startup response");
        let _closed = wait_to_close.await;
    });

    let mut config = Config::new();
    config.user("kronika-dead-client-test");
    let (client, connection) = config
        .connect_raw(client_io, NoTls)
        .await
        .expect("the probe performs a valid startup handshake");
    let driver = tokio::spawn(async move {
        let _ended = connection.await;
    });
    let mut pool = pool();
    pool.open = Some(Open {
        client,
        driver,
        generation: 7,
    });
    pool.next_generation = 8;
    assert_eq!(pool.generation(), Some(7));
    assert_eq!(
        pool.session_for_generation(7)
            .map(crate::Session::generation),
        Some(7)
    );
    assert!(pool.session_for_generation(8).is_none());
    assert_eq!(
        pool.generation(),
        Some(7),
        "a healthy generation mismatch must not churn the connection"
    );

    close_server.send(()).expect("the server is waiting");
    server.await.expect("the server closes cleanly");
    tokio::time::timeout(Duration::from_secs(1), async {
        while pool.generation().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the driver notices EOF");
    assert_eq!(pool.generation(), None);
    assert!(pool.session_for_generation(7).is_none());
    assert!(pool.open.is_none(), "the closed session must be discarded");

    assert!(pool.session().await.is_err());
    assert!(pool.open.is_none(), "a dead client must be discarded");
}
