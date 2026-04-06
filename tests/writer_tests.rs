#![allow(clippy::unwrap_used, clippy::expect_used)]

use tempfile::NamedTempFile;
use tokio::sync::mpsc;

use polymarket_bot::actors::writer::WriterActor;
use polymarket_bot::types::*;

#[tokio::test]
async fn test_writer_processes_spot_price() {
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    let (tx, rx) = mpsc::channel::<DbEvent>(100);

    let handle = tokio::spawn(async move {
        let mut actor = WriterActor::new(&path, 10, 100).unwrap();
        actor.run(rx).await;
    });

    tx.send(DbEvent::SpotPrice(SpotPrice {
        asset: Asset::BTC,
        price: 85000.0,
        ts: now_micros(),
    }))
    .await
    .unwrap();

    drop(tx);
    handle.await.unwrap();

    let conn = polymarket_bot::db::init(tmp.path().to_str().unwrap()).unwrap();
    let count: i32 = conn
        .query_row("SELECT COUNT(*) FROM spot_prices", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_writer_batches_writes() {
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    let (tx, rx) = mpsc::channel::<DbEvent>(200);

    let handle = tokio::spawn(async move {
        let mut actor = WriterActor::new(&path, 50, 5000).unwrap();
        actor.run(rx).await;
    });

    for i in 0..100 {
        tx.send(DbEvent::SpotPrice(SpotPrice {
            asset: Asset::ETH,
            price: 3000.0 + i as f64,
            ts: now_micros(),
        }))
        .await
        .unwrap();
    }

    drop(tx);
    handle.await.unwrap();

    let conn = polymarket_bot::db::init(tmp.path().to_str().unwrap()).unwrap();
    let count: i32 = conn
        .query_row("SELECT COUNT(*) FROM spot_prices", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 100);
}
