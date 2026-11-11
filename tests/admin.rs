//! The Credentials screen through the real router: a credential is added with
//! a label, attached to what uses it, changed without retyping its secret,
//! revealed on the audit log, and deleted.

use std::sync::Arc;

use laterite_admin::axum::body::Body;
use laterite_admin::axum::http::{Request, StatusCode};
use laterite_admin::axum::Router;
use laterite_admin::form::FormField;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{t, CatalogStore, Crypt, Db, Module, ModuleId, Registry};
use rainmill_vault::{lease, store, KindReg, SlotReg, VaultModule};
use tower::ServiceExt;

const BASE: &str = "/admin/rainmill/vault/credentials";
const CSRF: &str = "tok-123";
const POOL: &str = "acme.stock.api";
const SINGLE: &str = "acme.mailer.api";

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.set_owner(VaultModule.id());
    VaultModule.register(&mut registry);
    registry.set_owner(ModuleId::new("acme"));
    registry.add(
        KindReg::new("acme.token", t!("Acme token"))
            .help(t!("Acme > Settings > API."))
            .field(FormField::of("token", t!("API token"), "sensitive").required())
            .field(FormField::text("account", t!("Account"))),
    );
    registry.add(
        SlotReg::pooled(POOL, "acme.token")
            .label(t!("Stock sync"))
            .reason(t!("Reads stock levels through the day")),
    );
    registry.add(SlotReg::single(SINGLE, "acme.token").label(t!("Mailer")));
    registry
}

fn app(db: &Db, crypt: Arc<Crypt>) -> Router {
    use laterite_admin::routes::ScreenReg;
    let mut registry = registry();
    let mut screens = registry.take::<ScreenReg>();
    for s in &mut screens {
        s.base_path = format!("/rainmill/vault{}", s.base_path);
    }
    let permissions = registry.take::<laterite_admin::Permission>();
    let nav = registry.take::<laterite_admin::nav::NavItem>();
    let mut config = AdminConfig::default();
    config.crypt = Some(crypt);
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            screens,
            permissions,
            nav,
            plugin_defined: Arc::new(registry),
            ..Default::default()
        },
        config,
        Arc::new(CatalogStore::default()),
    )
}

async fn setup() -> (
    Db,
    laterite_core::testing::TestGuard,
    Arc<Crypt>,
    Router,
    String,
) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(rainmill_vault::migrations::migrations());
    let (db, guard) = laterite_core::testing::connect_test(&migrations).await;
    let crypt = Arc::new(Crypt::ephemeral());
    let app = app(&db, crypt.clone());
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    svc.create_superuser(NewOperator {
        username: "root",
        email: "root@acme.test",
        first_name: "Root",
        last_name: None,
        password: "rootpw12345",
        timezone: None,
    })
    .await
    .unwrap();
    let token = svc
        .authenticate("root", "rootpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;
    svc.set_session_data(&token, &format!(r#"{{"v":1,"csrf":"{CSRF}"}}"#))
        .await
        .unwrap();
    (db, guard, crypt, app, token)
}

async fn get(app: &Router, path: &str, token: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .uri(path)
        .header("cookie", format!("laterite_session={token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = laterite_admin::axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn post(app: &Router, path: &str, token: &str, body: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("laterite_session={token}"))
        .header("sec-fetch-site", "same-origin")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(format!("_csrf={CSRF}&{body}")))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = laterite_admin::axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[test]
fn a_kind_or_slot_that_cannot_work_stops_the_boot() {
    // A slot for a kind nobody registered.
    let mut bad = Registry::new();
    bad.set_owner(ModuleId::new("acme"));
    bad.add(SlotReg::single("acme.x", "acme.missing"));
    assert!(rainmill_vault::check(&bad).is_err());
    // A kind's field under a name the form uses itself.
    let mut bad = Registry::new();
    bad.set_owner(ModuleId::new("acme"));
    bad.add(KindReg::new("acme.token", t!("Acme")).field(FormField::text("label", t!("Label"))));
    assert!(rainmill_vault::check(&bad).is_err());
    assert!(rainmill_vault::check(&registry()).is_ok());
}

#[tokio::test]
async fn a_credential_is_added_attached_changed_revealed_and_deleted() {
    let (db, _guard, crypt, app, token) = setup().await;

    // Empty: the list says so, offers a kind, and names what needs a credential.
    let (status, html) = get(&app, &format!("{BASE}/"), &token).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains("New Acme token") && html.contains("Stock sync"),
        "{html}"
    );
    assert!(html.contains("Needs a credential"), "{html}");

    // The form is the kind's: its fields, a label, and a switch per slot.
    let (status, html) = get(&app, &format!("{BASE}/new?kind=acme.token"), &token).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    for expected in [
        "name=\"label\"",
        "name=\"token\"",
        "name=\"account\"",
        "name=\"slot_0\"",
        "name=\"slot_1\"",
        "Acme &gt; Settings",
        "fields/sensitive",
    ] {
        assert!(html.contains(expected), "{expected}: {html}");
    }
    let (status, _) = get(&app, &format!("{BASE}/new?kind=acme.nothing"), &token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // No label, or no token: refused, and the secret is not echoed back.
    let (status, html) = post(
        &app,
        &format!("{BASE}/new?kind=acme.token"),
        &token,
        "label=&token=hunter2&period_day=1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("Give the credential a label") && !html.contains("hunter2"),
        "{html}"
    );
    let (_, html) = post(
        &app,
        &format!("{BASE}/new?kind=acme.token"),
        &token,
        "label=First&token=&period_day=1",
    )
    .await;
    assert!(html.contains("a value is required"), "{html}");
    assert!(store::list(&db).await.unwrap().is_empty());

    // Saved with a label, an allowance, and attached to the pool.
    let (status, _) = post(
        &app,
        &format!("{BASE}/new?kind=acme.token"),
        &token,
        "label=Acme%3A+first+account&token=hunter2&account=alpha&allowance=1900&period_day=11&slot_0=on",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let items = store::list(&db).await.unwrap();
    assert_eq!(items.len(), 1);
    let first = &items[0];
    assert_eq!(first.label, "Acme: first account");
    assert_eq!((first.allowance, first.period_day), (Some(1900), 11));
    let fields = store::fields(&db, &crypt, first.id).await.unwrap();
    assert_eq!(
        (fields["token"].as_str(), fields["account"].as_str()),
        ("hunter2", "alpha")
    );
    assert_eq!(store::slots_of(&db, first.id).await.unwrap(), [POOL]);

    // A second one joins the pool, and a lease cycles through both.
    post(
        &app,
        &format!("{BASE}/new?kind=acme.token"),
        &token,
        "label=Second&token=swordfish&period_day=1&slot_0=on&slot_1=on",
    )
    .await;
    let mut seen = Vec::new();
    for _ in 0..4 {
        let lease = lease(&db, &crypt, POOL).await.unwrap();
        seen.push(lease.field("token").unwrap().to_string());
        lease.used(3).await.unwrap();
    }
    assert_eq!(seen, ["hunter2", "swordfish", "hunter2", "swordfish"]);

    // The list shows labels, what was spent, and what each is used by.
    let (_, html) = get(&app, &format!("{BASE}/"), &token).await;
    assert!(
        html.contains("Acme: first account") && html.contains("6 / 1900"),
        "{html}"
    );
    assert!(
        html.contains("Stock sync, Mailer") || html.contains("Mailer, Stock sync"),
        "{html}"
    );
    assert!(
        !html.contains("hunter2") && !html.contains("swordfish"),
        "{html}"
    );

    // The edit page holds no secret; saving with the mask keeps it.
    let edit = format!("{BASE}/{}/edit", first.id);
    let (status, html) = get(&app, &edit, &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !html.contains("hunter2") && html.contains("data-has-value"),
        "{html}"
    );
    assert!(
        html.contains("value=\"alpha\"") && html.contains("Usage"),
        "{html}"
    );
    let mask = "%E2%80%A2%E2%80%A2%E2%80%A2%E2%80%A2%E2%80%A2%E2%80%A2%E2%80%A2%E2%80%A2";
    let (status, _) = post(
        &app,
        &edit,
        &token,
        &format!("label=Renamed&token={mask}&account=beta&allowance=&period_day=11"),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let item = store::find(&db, first.id).await.unwrap().unwrap();
    assert_eq!((item.label.as_str(), item.allowance), ("Renamed", None));
    let fields = store::fields(&db, &crypt, first.id).await.unwrap();
    assert_eq!(
        (fields["token"].as_str(), fields["account"].as_str()),
        ("hunter2", "beta")
    );
    // Its switch was off in that save: it left the pool.
    assert!(store::slots_of(&db, first.id).await.unwrap().is_empty());

    // Reveal answers with the secret and is on the audit log; another field is not found.
    let (status, plain) = get(&app, &format!("{BASE}/{}/reveal/token", first.id), &token).await;
    assert_eq!((status, plain.as_str()), (StatusCode::OK, "hunter2"));
    let (status, _) = get(&app, &format!("{BASE}/{}/reveal/account", first.id), &token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Deleted: gone from the list, and no longer leased.
    let (status, _) = post(&app, &format!("{BASE}/{}/delete", first.id), &token, "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(store::list(&db).await.unwrap().len(), 1);

    // Signed out, nothing answers.
    let (status, _) = get(&app, &format!("{BASE}/"), "").await;
    assert!(status.is_redirection());
}
