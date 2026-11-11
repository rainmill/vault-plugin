//! Storing credentials, attaching them to slots, and what a lease hands out.

use std::collections::BTreeMap;

use laterite_core::query::{bind_values, build};
use laterite_core::sea_query::{Alias, Expr, Query};
use laterite_core::testing::connect_test;
use laterite_core::{sqlx, Crypt, Db};
use rainmill_vault::{lease, store, ItemInput, Standing, VaultError};

const SLOT: &str = "acme.stock.api";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    connect_test(&[rainmill_vault::migrations::migrations()]).await
}

fn token(label: &str, value: &str, allowance: Option<i64>) -> ItemInput {
    ItemInput {
        label: label.to_string(),
        fields: BTreeMap::from([("token".to_string(), value.to_string())]),
        allowance,
        period_day: 1,
        expires_at: None,
    }
}

/// Sets columns on an item directly, to stand in for time passing.
async fn set(db: &Db, id: i64, values: &[(&str, &str)]) {
    let mut update = Query::update();
    update
        .table(Alias::new("vault_items"))
        .and_where(Expr::col(Alias::new("id")).eq(id));
    for (column, value) in values {
        update.value(Alias::new(*column), *value);
    }
    let (sql, bound) = build(db.backend, update);
    bind_values(sqlx::query(&sql), bound)
        .execute(&db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_credential_is_sealed_and_opens_with_its_fields() {
    let (db, _guard) = test_db().await;
    let crypt = Crypt::ephemeral();
    let id = store::create(&db, &crypt, "acme.token", &token("First", "hunter2", None))
        .await
        .unwrap();

    let item = store::find(&db, id).await.unwrap().unwrap();
    assert_eq!(
        (item.label.as_str(), item.kind.as_str()),
        ("First", "acme.token")
    );
    assert_eq!(item.standing(), Standing::Ok);
    assert_eq!(
        store::fields(&db, &crypt, id).await.unwrap()["token"],
        "hunter2"
    );
    // Another key opens nothing.
    assert!(matches!(
        store::fields(&db, &Crypt::ephemeral(), id).await,
        Err(VaultError::Sealed)
    ));

    store::update(&db, &crypt, id, &token("Renamed", "rotated", Some(10)))
        .await
        .unwrap();
    let item = store::find(&db, id).await.unwrap().unwrap();
    assert_eq!((item.label.as_str(), item.allowance), ("Renamed", Some(10)));
    assert_eq!(
        store::fields(&db, &crypt, id).await.unwrap()["token"],
        "rotated"
    );

    store::bind(&db, SLOT, id, true).await.unwrap();
    store::delete(&db, id).await.unwrap();
    assert!(store::find(&db, id).await.unwrap().is_none());
    assert!(matches!(
        lease(&db, &crypt, SLOT).await,
        Err(VaultError::Unbound(_))
    ));
}

#[tokio::test]
async fn a_pool_is_cycled_and_a_spent_credential_is_passed_over() {
    let (db, _guard) = test_db().await;
    let crypt = Crypt::ephemeral();
    let mut ids = Vec::new();
    for (label, value, allowance) in [("A", "ta", Some(5)), ("B", "tb", None), ("C", "tc", None)] {
        let id = store::create(&db, &crypt, "acme.token", &token(label, value, allowance))
            .await
            .unwrap();
        store::bind(&db, SLOT, id, true).await.unwrap();
        ids.push(id);
    }
    // Attaching twice changes nothing.
    store::bind(&db, SLOT, ids[0], true).await.unwrap();
    assert_eq!(store::items_of(&db, SLOT).await.unwrap().len(), 3);

    // Each lease takes the next credential, round and round.
    let mut seen = Vec::new();
    for _ in 0..6 {
        let lease = lease(&db, &crypt, SLOT).await.unwrap();
        seen.push(format!(
            "{}={}",
            lease.label(),
            lease.field("token").unwrap()
        ));
        lease.used(2).await.unwrap();
    }
    assert_eq!(seen, ["A=ta", "B=tb", "C=tc", "A=ta", "B=tb", "C=tc"]);

    // A has spent 4 of 5: one more use takes it to its allowance, and the
    // cycle then runs on B and C alone.
    let a = lease(&db, &crypt, SLOT).await.unwrap();
    assert_eq!((a.label(), a.remaining()), ("A", Some(1)));
    a.used(3).await.unwrap();
    let item = store::find(&db, ids[0]).await.unwrap().unwrap();
    assert_eq!((item.used, item.standing()), (7, Standing::Spent));
    let mut seen = Vec::new();
    for _ in 0..4 {
        let lease = lease(&db, &crypt, SLOT).await.unwrap();
        seen.push(lease.label().to_string());
        lease.used(1).await.unwrap();
    }
    assert_eq!(seen, ["B", "C", "B", "C"]);

    // A new period gives A its allowance back.
    set(&db, ids[0], &[("period_start", "2000-01-01")]).await;
    let item = store::find(&db, ids[0]).await.unwrap().unwrap();
    assert_eq!((item.used, item.standing()), (0, Standing::Ok));

    // What each spent is counted by day.
    let usage = store::usage(&db, ids[1], 7).await.unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!((usage[0].1, usage[0].2, usage[0].3), (4, 0, 6));
}

#[tokio::test]
async fn a_refused_credential_leaves_the_cycle_until_it_cools_off() {
    let (db, _guard) = test_db().await;
    let crypt = Crypt::ephemeral();
    let a = store::create(&db, &crypt, "acme.token", &token("A", "ta", None))
        .await
        .unwrap();
    let b = store::create(&db, &crypt, "acme.token", &token("B", "tb", None))
        .await
        .unwrap();
    store::bind(&db, SLOT, a, true).await.unwrap();
    store::bind(&db, SLOT, b, true).await.unwrap();

    // A failure that is not a refusal is counted and changes nothing else.
    let first = lease(&db, &crypt, SLOT).await.unwrap();
    assert_eq!(first.label(), "A");
    first
        .failed(Some(500), "the provider is down")
        .await
        .unwrap();
    assert_eq!(
        store::find(&db, a).await.unwrap().unwrap().standing(),
        Standing::Ok
    );

    // B is refused: it is failing, and only A is handed out.
    let second = lease(&db, &crypt, SLOT).await.unwrap();
    assert_eq!(second.label(), "B");
    second.failed(Some(401), "token revoked").await.unwrap();
    let item = store::find(&db, b).await.unwrap().unwrap();
    assert_eq!(item.standing(), Standing::Failing);
    assert_eq!(item.last_error.as_deref(), Some("token revoked"));
    for _ in 0..3 {
        let lease = lease(&db, &crypt, SLOT).await.unwrap();
        assert_eq!(lease.label(), "A");
        lease.used(1).await.unwrap();
    }

    // With A expired too, nothing is usable, and the error says why.
    set(&db, a, &[("expires_at", "2000-01-01T00:00:00Z")]).await;
    match lease(&db, &crypt, SLOT).await {
        Err(VaultError::NoneUsable { standing, .. }) => assert_eq!(
            standing,
            [
                ("B".to_string(), Standing::Failing),
                ("A".to_string(), Standing::Expired)
            ]
        ),
        other => panic!("{other:?}"),
    }

    // After the cool-off B is tried again, and a success clears it.
    set(&db, b, &[("last_error_at", "2000-01-01T00:00:00.000000Z")]).await;
    let again = lease(&db, &crypt, SLOT).await.unwrap();
    assert_eq!(again.label(), "B");
    again.used(1).await.unwrap();
    assert_eq!(
        store::find(&db, b).await.unwrap().unwrap().standing(),
        Standing::Ok
    );
}

#[tokio::test]
async fn a_slot_for_one_credential_keeps_the_last_attached() {
    let (db, _guard) = test_db().await;
    let crypt = Crypt::ephemeral();
    let a = store::create(&db, &crypt, "acme.token", &token("A", "ta", None))
        .await
        .unwrap();
    let b = store::create(&db, &crypt, "acme.token", &token("B", "tb", None))
        .await
        .unwrap();
    store::bind(&db, "acme.single", a, false).await.unwrap();
    store::bind(&db, "acme.single", b, false).await.unwrap();
    let attached = store::items_of(&db, "acme.single").await.unwrap();
    assert_eq!(attached.len(), 1);
    assert_eq!(attached[0].label, "B");
    assert_eq!(store::slots_of(&db, b).await.unwrap(), ["acme.single"]);
    store::unbind(&db, "acme.single", b).await.unwrap();
    assert!(store::slots_of(&db, b).await.unwrap().is_empty());
}
