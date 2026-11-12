//! The Credentials screen: what is stored, where each credential stands, and
//! the slots it is attached to.
//!
//! The form is built per kind, so it is a screen of this plugin's own over the
//! framework's value form, not a descriptor file.

use laterite_core::serde_json;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use laterite_admin::axum::extract::{Path, Query};
use laterite_admin::axum::http::{header, StatusCode};
use laterite_admin::axum::response::{IntoResponse, Redirect, Response};
use laterite_admin::axum::routing::{get, post};
use laterite_admin::axum::{Extension, Form, Router};
use laterite_admin::form::{FormField, Span};
use laterite_admin::html::Markup;
use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};
use laterite_admin::ui::{Badge, Button};
use laterite_admin::{
    AdminRegistry, AuthenticatedUser, FlashLevel, PageHead, Permission, SessionHandle, Shell,
};
use laterite_core::{t, CoreError, CoreResult, Registry, SealedFile, Text};

use crate::registry::{KindReg, SlotReg, RESERVED_FIELDS};
use crate::store::{self, Item, ItemInput, Standing};
use crate::{MANAGE, REVEAL};

const SECTION: &str = "rainmill.vault";

/// What a stored secret is shown as on the form, so the page never holds it.
const PRESENT: &str = "stored";

pub(crate) fn register(registry: &mut Registry) {
    let group = || t!("Vault");
    registry.add_permission(
        Permission::new(MANAGE, t!("Manage credentials"), group())
            .roles([laterite_admin::ROLE_ADMIN]),
    );
    registry.add_permission(
        Permission::new(REVEAL, t!("Reveal stored secrets"), group())
            .roles([laterite_admin::ROLE_ADMIN]),
    );
    registry.add_nav(
        laterite_admin::nav::NavItem::section(SECTION, t!("Credentials"))
            .icon("key")
            .description(t!(
                "Store API keys and key files, and choose what uses them."
            ))
            .slot(laterite_core::arrange::slot::SYSTEM)
            .order(18)
            .in_settings(t!("System")),
    );
    registry.add_screen(
        ScreenReg::new("/credentials", MANAGE, Arc::new(Credentials))
            .in_menu(t!("Credentials"))
            .menu(
                laterite_admin::nav::Placement::new()
                    .under(SECTION)
                    .icon("key"),
            ),
    );
}

/// Refuses a boot where a kind or a slot cannot work: two with one code, a
/// kind's field under a reserved name, a slot for a kind nobody registered.
pub(crate) fn check(registry: &Registry) -> CoreResult<()> {
    let kinds = registry.items::<KindReg>();
    let slots = registry.items::<SlotReg>();
    let fail = |message: String| Err(CoreError::Config(message));
    for (i, kind) in kinds.iter().enumerate() {
        if kinds[..i].iter().any(|k| k.code == kind.code) {
            return fail(format!(
                "the credential kind `{}` is registered twice",
                kind.code
            ));
        }
        if let Some(field) = kind
            .fields
            .iter()
            .find(|f| RESERVED_FIELDS.contains(&f.name.as_str()) || f.name.starts_with("slot_"))
        {
            return fail(format!(
                "the credential kind `{}` has a field named `{}`, which the form uses itself",
                kind.code, field.name
            ));
        }
    }
    for (i, slot) in slots.iter().enumerate() {
        if slots[..i].iter().any(|s| s.code == slot.code) {
            return fail(format!(
                "the credential slot `{}` is registered twice",
                slot.code
            ));
        }
        if !kinds.iter().any(|k| k.code == slot.kind) {
            return fail(format!(
                "the credential slot `{}` asks for the kind `{}`, which no module registered",
                slot.code, slot.kind
            ));
        }
    }
    Ok(())
}

mod usage;

struct Credentials;

#[derive(Clone)]
struct Cx {
    ctx: RouteCtx,
    kinds: Arc<Vec<KindReg>>,
    slots: Arc<Vec<SlotReg>>,
}

impl Cx {
    fn kind(&self, code: &str) -> Option<&KindReg> {
        self.kinds.iter().find(|k| k.code == code)
    }

    /// The slots that take credentials of `kind`, in registration order. A
    /// slot's place in this list names its switch on the form.
    fn slots_for(&self, kind: &str) -> Vec<&SlotReg> {
        self.slots.iter().filter(|s| s.kind == kind).collect()
    }

    fn index(&self) -> String {
        self.ctx.url("")
    }

    fn edit(&self, id: i64) -> String {
        self.ctx.url(&format!("{id}/edit"))
    }
}

impl Screen for Credentials {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let registry = ctx.plugin_defined();
        let cx = Cx {
            ctx: ctx.clone(),
            kinds: Arc::new(registry.items::<KindReg>().into_iter().cloned().collect()),
            slots: Arc::new(registry.items::<SlotReg>().into_iter().cloned().collect()),
        };
        let c = |cx: &Cx| cx.clone();
        let (index_cx, new_cx, create_cx, edit_cx, update_cx, delete_cx, reveal_cx) =
            (c(&cx), c(&cx), c(&cx), c(&cx), c(&cx), c(&cx), c(&cx));
        let (usage_home_cx, usage_cx, rates_cx) = (c(&cx), c(&cx), c(&cx));
        Router::new()
            .route(
                "/usage",
                get(move || {
                    let cx = usage_home_cx.clone();
                    async move {
                        match cx.slots.first() {
                            Some(slot) => {
                                Redirect::to(&cx.ctx.url(&format!("usage/{}", slot.code)))
                                    .into_response()
                            }
                            None => Redirect::to(&cx.index()).into_response(),
                        }
                    }
                }),
            )
            .route(
                "/usage/{slot}",
                get(
                    move |Path(slot): Path<String>,
                          Query(query): Query<laterite_admin::chart::ChartQuery>,
                          Extension(shell): Extension<Shell>| {
                        let cx = usage_cx.clone();
                        async move { usage::page(&cx, shell, &slot, query).await }
                    },
                ),
            )
            .route(
                "/usage/{slot}/rates",
                post(
                    move |Path(slot): Path<String>,
                          Extension(user): Extension<AuthenticatedUser>,
                          Extension(session): Extension<SessionHandle>,
                          Form(data): Form<HashMap<String, String>>| {
                        let cx = rates_cx.clone();
                        async move { usage::save_rates(&cx, user, session, &slot, data).await }
                    },
                ),
            )
            .route(
                "/",
                get(move |Extension(shell): Extension<Shell>| {
                    let cx = index_cx.clone();
                    async move { index(&cx, shell).await }
                }),
            )
            .route(
                "/new",
                get(
                    move |Query(q): Query<HashMap<String, String>>,
                          Extension(shell): Extension<Shell>| {
                        let cx = new_cx.clone();
                        async move {
                            let kind = q.get("kind").cloned().unwrap_or_default();
                            form_page(&cx, shell, &kind, None, None, None).await
                        }
                    },
                )
                .post(
                    move |Query(q): Query<HashMap<String, String>>,
                          Extension(shell): Extension<Shell>,
                          Extension(user): Extension<AuthenticatedUser>,
                          Extension(session): Extension<SessionHandle>,
                          Form(data): Form<HashMap<String, String>>| {
                        let cx = create_cx.clone();
                        async move {
                            let kind = q.get("kind").cloned().unwrap_or_default();
                            save(&cx, shell, user, session, &kind, None, data).await
                        }
                    },
                ),
            )
            .route(
                "/{id}/edit",
                get(
                    move |Path(id): Path<i64>, Extension(shell): Extension<Shell>| {
                        let cx = edit_cx.clone();
                        async move {
                            match store::find(cx.ctx.db(), id).await {
                                Ok(Some(item)) => {
                                    let kind = item.kind.clone();
                                    form_page(&cx, shell, &kind, Some(item), None, None).await
                                }
                                Ok(None) => StatusCode::NOT_FOUND.into_response(),
                                Err(e) => failure(e),
                            }
                        }
                    },
                )
                .post(
                    move |Path(id): Path<i64>,
                          Extension(shell): Extension<Shell>,
                          Extension(user): Extension<AuthenticatedUser>,
                          Extension(session): Extension<SessionHandle>,
                          Form(data): Form<HashMap<String, String>>| {
                        let cx = update_cx.clone();
                        async move {
                            match store::find(cx.ctx.db(), id).await {
                                Ok(Some(item)) => {
                                    let kind = item.kind.clone();
                                    save(&cx, shell, user, session, &kind, Some(item), data).await
                                }
                                Ok(None) => StatusCode::NOT_FOUND.into_response(),
                                Err(e) => failure(e),
                            }
                        }
                    },
                ),
            )
            .route(
                "/{id}/delete",
                post(
                    move |Path(id): Path<i64>,
                          Extension(user): Extension<AuthenticatedUser>,
                          Extension(session): Extension<SessionHandle>| {
                        let cx = delete_cx.clone();
                        async move { delete(&cx, user, session, id).await }
                    },
                ),
            )
            .route(
                "/{id}/reveal/{field}",
                get(
                    move |Path((id, field)): Path<(i64, String)>,
                          Extension(shell): Extension<Shell>,
                          Extension(user): Extension<AuthenticatedUser>| {
                        let cx = reveal_cx.clone();
                        async move { reveal(&cx, shell, user, id, &field).await }
                    },
                ),
            )
    }
}

fn failure(e: crate::VaultError) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn standing_text(standing: Standing) -> Text {
    match standing {
        Standing::Ok => t!("Ready"),
        Standing::Failing => t!("Refused"),
        Standing::Spent => t!("Allowance used"),
        Standing::Expired => t!("Expired"),
    }
}

/// What a credential has spent this period, as text.
fn spent_text(item: &Item, shell: &Shell) -> String {
    match item.allowance {
        Some(allowance) => format!("{} / {}", item.used, allowance),
        None if item.used > 0 => item.used.to_string(),
        None => shell.t("No limit"),
    }
}

/// The list of credentials, then every slot and what is attached to it.
async fn index(cx: &Cx, shell: Shell) -> Response {
    let db = cx.ctx.db();
    let items = match store::list(db).await {
        Ok(items) => items,
        Err(e) => return failure(e),
    };
    let mut body = String::new();

    // Credentials.
    if items.is_empty() {
        body.push_str(&format!(
            "<p class=\"lat-note\">{}</p>",
            escape(&shell.t("No credentials yet. Add one, then attach it to what uses it."))
        ));
    } else {
        let mut rows = String::new();
        for item in &items {
            let kind = cx
                .kind(&item.kind)
                .map(|k| shell.tt(&k.label))
                .unwrap_or_else(|| item.kind.clone());
            let used_by = store::slots_of(db, item.id).await.unwrap_or_default();
            let used_by: Vec<String> = used_by
                .iter()
                .map(|code| {
                    cx.slots
                        .iter()
                        .find(|s| &s.code == code)
                        .map(|s| shell.tt(&s.label))
                        .unwrap_or_else(|| code.clone())
                })
                .collect();
            let standing = item.standing();
            let mut state = Badge::new(standing_text(standing))
                .render(&shell)
                .into_string();
            if standing == Standing::Failing {
                if let Some(error) = &item.last_error {
                    state.push_str(&format!(
                        "<br><span class=\"lat-muted\">{}</span>",
                        escape(error)
                    ));
                }
            }
            rows.push_str(&format!(
                "<tr><td><a href=\"{}\"><strong>{}</strong></a></td><td>{}</td><td>{state}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape(&cx.edit(item.id)),
                escape(&item.label),
                escape(&kind),
                escape(&spent_text(item, &shell)),
                item.last_used_at
                    .as_deref()
                    .map(|at| escape(&shell.datetime(at)))
                    .unwrap_or_else(|| escape(&shell.t("Never"))),
                if used_by.is_empty() {
                    format!("<span class=\"lat-muted\">{}</span>", escape(&shell.t("Nothing")))
                } else {
                    escape(&used_by.join(", "))
                },
            ));
        }
        body.push_str(&format!(
            "<div class=\"lat-tablewrap\"><table class=\"lat-table\"><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>{rows}</tbody></table></div>",
            escape(&shell.t("Label")),
            escape(&shell.t("Kind")),
            escape(&shell.t("State")),
            escape(&shell.t("Used this period")),
            escape(&shell.t("Last used")),
            escape(&shell.t("Used by")),
        ));
    }

    // What uses them.
    body.push_str(&format!(
        "<h3 class=\"lat-field__heading\" style=\"margin-top:28px\">{}</h3>",
        escape(&shell.t("What uses credentials"))
    ));
    if cx.slots.is_empty() {
        body.push_str(&format!(
            "<p class=\"lat-note\">{}</p>",
            escape(&shell.t("Nothing installed asks for a credential."))
        ));
    } else {
        let mut rows = String::new();
        for slot in cx.slots.iter() {
            let attached = store::items_of(db, &slot.code).await.unwrap_or_default();
            let kind = cx
                .kind(&slot.kind)
                .map(|k| shell.tt(&k.label))
                .unwrap_or_else(|| slot.kind.clone());
            let list = if attached.is_empty() {
                format!(
                    "<span class=\"lat-muted\">{}</span>",
                    escape(&shell.t("Needs a credential"))
                )
            } else {
                attached
                    .iter()
                    .map(|item| {
                        format!(
                            "<a href=\"{}\">{}</a> <span class=\"lat-muted\">({})</span>",
                            escape(&cx.edit(item.id)),
                            escape(&item.label),
                            escape(&shell.tt(&standing_text(item.standing())))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("<br>")
            };
            rows.push_str(&format!(
                "<tr><td><strong>{}</strong>{}</td><td>{}</td><td>{}</td><td>{list}</td><td><a href=\"{usage}\">{usage_label}</a></td></tr>",
                escape(&shell.tt(&slot.label)),
                slot.reason
                    .as_ref()
                    .map(|r| format!(
                        "<br><span class=\"lat-muted\">{}</span>",
                        escape(&shell.tt(r))
                    ))
                    .unwrap_or_default(),
                escape(&kind),
                escape(&if slot.pooled {
                    shell.t("Several, used in turn")
                } else {
                    shell.t("One")
                }),
                usage = escape(&cx.ctx.url(&format!("usage/{}", slot.code))),
                usage_label = escape(&shell.t("Usage")),
            ));
        }
        body.push_str(&format!(
            "<div class=\"lat-tablewrap\"><table class=\"lat-table\"><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th></th></tr></thead><tbody>{rows}</tbody></table></div>",
            escape(&shell.t("Use")),
            escape(&shell.t("Kind")),
            escape(&shell.t("Takes")),
            escape(&shell.t("Credentials")),
        ));
    }

    // One New button a kind.
    let mut actions = String::new();
    for (i, kind) in cx.kinds.iter().enumerate() {
        let label = Text::dynamic(format!("{} {}", shell.t("New"), shell.tt(&kind.label)));
        let href = cx.ctx.url(&format!("new?kind={}", kind.code));
        let button = Button::link(label, href);
        let button = if i == 0 { button.primary() } else { button };
        actions.push_str(&button.render(&shell).into_string());
    }
    shell.page_with(
        PageHead::new(t!("Credentials")).actions(Markup::raw_unchecked(actions)),
        Markup::raw_unchecked(body),
    )
}

/// The form's own fields around a kind's, with a switch per slot of the kind.
fn form_fields(cx: &Cx, kind: &KindReg) -> Vec<FormField> {
    let mut fields = vec![FormField::text("label", t!("Label"))
        .required()
        .help(t!(
            "A name to tell this credential from the others. Shown in lists and logs; never put the secret here."
        ))];
    fields.extend(kind.fields.iter().cloned());
    fields.push(
        FormField::of("allowance", t!("Allowance a period"), "text")
            .options(serde_json::json!({ "input": "number", "min": 0 }))
            .span(Span::columns(6))
            .help(t!(
                "How many units it may spend before it is passed over until its next period. Blank for no limit."
            )),
    );
    fields.push(
        FormField::of("period_day", t!("Period starts on day"), "text")
            .options(serde_json::json!({ "input": "number", "min": 1, "max": 28 }))
            .span(Span::columns(6))
            .help(t!(
                "The day of the month the allowance starts again, 1 to 28."
            )),
    );
    if !cx.slots_for(&kind.code).is_empty() {
        fields.push(
            FormField::of("slot_heading", t!("Use it for"), "section").help(t!(
                "What may use this credential. Something that takes several uses each in turn."
            )),
        );
    }
    for (i, slot) in cx.slots_for(&kind.code).into_iter().enumerate() {
        let mut field = FormField::switch(&format!("slot_{i}"), slot.label.clone());
        if let Some(reason) = &slot.reason {
            field = field.help(reason.clone());
        }
        fields.push(field);
    }
    fields
}

/// Whether a kind's field holds a secret, and whether that secret is a file.
fn secret(field: &FormField) -> (bool, bool) {
    let sensitive = field.field_type == "sensitive";
    let file = sensitive
        && field
            .options
            .get("file")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
    (sensitive, file)
}

/// The New or Edit page. `submitted` is a refused submission to show again.
async fn form_page(
    cx: &Cx,
    shell: Shell,
    kind_code: &str,
    item: Option<Item>,
    submitted: Option<serde_json::Value>,
    error: Option<Text>,
) -> Response {
    let Some(kind) = cx.kind(kind_code) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let fields = form_fields(cx, kind);
    let form = match cx.ctx.value_form(&fields, "rainmill.vault") {
        Ok(form) => form,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, shell.tt(&e)).into_response(),
    };
    let stored = match (&submitted, &item) {
        (Some(values), _) => values.clone(),
        (None, Some(item)) => match stored_view(cx, kind, item).await {
            Ok(values) => values,
            Err(e) => return failure(e),
        },
        (None, None) => serde_json::json!({ "period_day": "1" }),
    };
    let action = match &item {
        Some(item) => cx.edit(item.id),
        None => cx.ctx.url(&format!("new?kind={}", kind.code)),
    };
    let mut body = String::new();
    if let Some(error) = &error {
        body.push_str(&format!(
            "<div class=\"lat-alert\">{}</div>",
            escape(&shell.tt(error))
        ));
    }
    if let Some(item) = &item {
        if item.standing() == Standing::Failing {
            body.push_str(&format!(
                "<div class=\"lat-alert\">{} {}</div>",
                escape(&shell.t("The provider refused this credential the last time it was used.")),
                escape(item.last_error.as_deref().unwrap_or_default())
            ));
        }
    }
    body.push_str(&format!(
        "<form method=\"post\" action=\"{}\" class=\"lat-card\" style=\"padding:20px 24px\" data-lat-widget=\"form\"><input type=\"hidden\" name=\"_csrf\" value=\"{}\">",
        escape(&action),
        escape(shell.csrf_token())
    ));
    if let Some(help) = &kind.help {
        body.push_str(&format!(
            "<p class=\"lat-note\">{}</p>",
            escape(&shell.tt(help))
        ));
    }
    body.push_str(form.render(&stored, &shell).as_str());
    body.push_str(&format!(
        "<div class=\"lat-actions\"><button class=\"lat-btn lat-btn--primary\" type=\"submit\">{}</button><a class=\"lat-btn lat-btn--ghost\" href=\"{}\">{}</a></div></form>",
        escape(&shell.t("Save")),
        escape(&cx.index()),
        escape(&shell.t("Cancel"))
    ));
    if let Some(item) = &item {
        body.push_str(&usage_table(cx, &shell, item).await);
    }
    let title = match &item {
        Some(item) => Text::dynamic(item.label.clone()),
        None => Text::dynamic(format!("{} {}", shell.t("New"), shell.tt(&kind.label))),
    };
    let mut head = PageHead::new(title)
        .back(t!("Credentials"), cx.index())
        .assets(&form.assets());
    if let Some(item) = &item {
        head = head.actions(
            Button::post(t!("Delete"), cx.ctx.url(&format!("{}/delete", item.id)))
                .danger()
                .confirm(t!(
                    "Delete this credential? Whatever uses it stops being able to."
                ))
                .render(&shell),
        );
    }
    shell.page_with(head, Markup::raw_unchecked(body))
}

/// What the form shows for a stored item. A secret is never sent to the page:
/// a text secret shows as its mask, and a file as its name and fingerprint.
async fn stored_view(
    cx: &Cx,
    kind: &KindReg,
    item: &Item,
) -> Result<serde_json::Value, crate::VaultError> {
    let db = cx.ctx.db();
    let crypt = cx.ctx.crypt();
    let fields = store::fields(db, crypt, item.id).await?;
    let mut view = serde_json::Map::new();
    view.insert("label".into(), item.label.clone().into());
    view.insert(
        "allowance".into(),
        item.allowance
            .map(|a| a.to_string())
            .unwrap_or_default()
            .into(),
    );
    view.insert("period_day".into(), item.period_day.to_string().into());
    for field in &kind.fields {
        let value = fields.get(&field.name).cloned().unwrap_or_default();
        let shown = match secret(field) {
            _ if value.is_empty() => String::new(),
            // Sealed again for the form's own field, which describes a file
            // by opening it.
            (true, true) => SealedFile::parse(&value)
                .and_then(|file| file.seal(crypt, &field.name).ok())
                .unwrap_or_default(),
            (true, false) => PRESENT.to_string(),
            (false, _) => value,
        };
        view.insert(field.name.clone(), shown.into());
    }
    let attached = store::slots_of(db, item.id).await?;
    for (i, slot) in cx.slots_for(&kind.code).into_iter().enumerate() {
        view.insert(format!("slot_{i}"), attached.contains(&slot.code).into());
    }
    Ok(serde_json::Value::Object(view))
}

/// The item's last fortnight of use, as a table under the form.
async fn usage_table(cx: &Cx, shell: &Shell, item: &Item) -> String {
    let usage = store::usage(cx.ctx.db(), item.id, 14)
        .await
        .unwrap_or_default();
    let mut out = format!(
        "<h3 class=\"lat-field__heading\" style=\"margin-top:28px\">{}</h3><p class=\"lat-note\">{}: {} &middot; {}: {}</p>",
        escape(&shell.t("Usage")),
        escape(&shell.t("This period")),
        escape(&spent_text(item, shell)),
        escape(&shell.t("since")),
        escape(&item.period_start),
    );
    if usage.is_empty() {
        out.push_str(&format!(
            "<p class=\"lat-note\">{}</p>",
            escape(&shell.t("Not used yet."))
        ));
        return out;
    }
    let rows: String = usage
        .iter()
        .map(|(day, calls, errors, units)| {
            format!(
                "<tr><td>{}</td><td>{calls}</td><td>{errors}</td><td>{units}</td></tr>",
                escape(day)
            )
        })
        .collect();
    out.push_str(&format!(
        "<div class=\"lat-tablewrap\"><table class=\"lat-table\"><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>{rows}</tbody></table></div>",
        escape(&shell.t("Day")),
        escape(&shell.t("Calls")),
        escape(&shell.t("Failures")),
        escape(&shell.t("Units")),
    ));
    out
}

/// Creates or changes an item from a submission, then attaches it as ticked.
async fn save(
    cx: &Cx,
    shell: Shell,
    user: AuthenticatedUser,
    session: SessionHandle,
    kind_code: &str,
    item: Option<Item>,
    data: HashMap<String, String>,
) -> Response {
    let Some(kind) = cx.kind(kind_code) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let db = cx.ctx.db();
    let crypt = cx.ctx.crypt();
    let fields = form_fields(cx, kind);
    let form = match cx.ctx.value_form(&fields, "rainmill.vault") {
        Ok(form) => form,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, shell.tt(&e)).into_response(),
    };
    // What to show again when the submission is refused: what was typed, with
    // no secret in it.
    let again = |data: &HashMap<String, String>| {
        let mut view = serde_json::Map::new();
        for field in &fields {
            let value = data.get(&field.name).cloned().unwrap_or_default();
            if field.name.starts_with("slot_") {
                view.insert(field.name.clone(), (!value.is_empty()).into());
            } else if !secret(field).0 {
                view.insert(field.name.clone(), value.into());
            }
        }
        serde_json::Value::Object(view)
    };
    let collected = match form.collect(&data) {
        Ok(serde_json::Value::Object(map)) => map,
        Ok(_) => serde_json::Map::new(),
        Err(e) => return form_page(cx, shell, kind_code, item, Some(again(&data)), Some(e)).await,
    };
    let text = |key: &str| {
        collected
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    let refuse = |message: Text| (again(&data), message);

    // The kind's fields: a secret left as its mask keeps what is stored.
    let previous = match &item {
        Some(item) => match store::fields(db, crypt, item.id).await {
            Ok(fields) => fields,
            Err(e) => return failure(e),
        },
        None => BTreeMap::new(),
    };
    let mut refused = None;
    let mut values = BTreeMap::new();
    for field in &kind.fields {
        let (sensitive, _) = secret(field);
        let value = match collected
            .get(&field.name)
            .and_then(serde_json::Value::as_str)
        {
            // The form's field sealed it for itself; the vault seals the
            // whole document, so it is opened here.
            Some(sealed) if sensitive && !sealed.is_empty() => crypt
                .decrypt(
                    laterite_core::crypt::FIELD_PURPOSE,
                    field.name.as_bytes(),
                    sealed,
                )
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_default(),
            Some(plain) if !sensitive => plain.trim().to_string(),
            _ => previous.get(&field.name).cloned().unwrap_or_default(),
        };
        let required = field
            .rules
            .iter()
            .any(|r| matches!(r, laterite_core::validation::Rule::Required));
        if required && value.is_empty() {
            refused = Some(refuse(Text::dynamic(format!(
                "{}: {}",
                shell.tt(&field.label),
                shell.t("a value is required.")
            ))));
        }
        values.insert(field.name.clone(), value);
    }
    let label = text("label");
    if label.is_empty() {
        refused = Some(refuse(t!("Give the credential a label.")));
    }
    let allowance = match text("allowance").as_str() {
        "" => None,
        number => match number.parse::<i64>() {
            Ok(n) if n >= 0 => Some(n),
            _ => {
                refused = Some(refuse(t!("The allowance is a whole number, or blank.")));
                None
            }
        },
    };
    let period_day = match text("period_day").as_str() {
        "" => 1,
        number => match number.parse::<u32>() {
            Ok(n) if (1..=28).contains(&n) => n,
            _ => {
                refused = Some(refuse(t!("The period starts on a day from 1 to 28.")));
                1
            }
        },
    };
    if let Some((view, message)) = refused {
        return form_page(cx, shell, kind_code, item, Some(view), Some(message)).await;
    }

    let input = ItemInput {
        label: label.clone(),
        fields: values,
        allowance,
        period_day,
        expires_at: None,
    };
    let (id, action) = match &item {
        Some(item) => match store::update(db, crypt, item.id, &input).await {
            Ok(()) => (item.id, "rainmill.vault.credential.update"),
            Err(e) => return failure(e),
        },
        None => match store::create(db, crypt, &kind.code, &input).await {
            Ok(id) => (id, "rainmill.vault.credential.create"),
            Err(e) => return failure(e),
        },
    };

    // Attach and detach as the switches say; each change is on the audit log.
    let attached = store::slots_of(db, id).await.unwrap_or_default();
    for (i, slot) in cx.slots_for(&kind.code).into_iter().enumerate() {
        let wanted = collected.get(&format!("slot_{i}")).is_some_and(|v| {
            v.as_bool() == Some(true)
                || v.as_str()
                    .is_some_and(|s| !s.is_empty() && s != "0" && s != "false")
        });
        let has = attached.contains(&slot.code);
        let change = if wanted && !has {
            store::bind(db, &slot.code, id, slot.pooled)
                .await
                .map(|()| "attach")
        } else if !wanted && has {
            store::unbind(db, &slot.code, id).await.map(|()| "detach")
        } else {
            continue;
        };
        match change {
            Ok(what) => {
                cx.ctx
                    .audit(
                        &user,
                        &format!("rainmill.vault.credential.{what}"),
                        Some("vault_items"),
                        Some(&id.to_string()),
                        Some(&label),
                        Some(&slot.code),
                    )
                    .await;
            }
            Err(e) => return failure(e),
        }
    }
    cx.ctx
        .audit(
            &user,
            action,
            Some("vault_items"),
            Some(&id.to_string()),
            Some(&label),
            Some(&kind.code),
        )
        .await;
    session.push_flash(FlashLevel::Success, t!("The credential was saved."));
    Redirect::to(&cx.index()).into_response()
}

async fn delete(cx: &Cx, user: AuthenticatedUser, session: SessionHandle, id: i64) -> Response {
    let db = cx.ctx.db();
    let item = match store::find(db, id).await {
        Ok(Some(item)) => item,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return failure(e),
    };
    if let Err(e) = store::delete(db, id).await {
        return failure(e);
    }
    cx.ctx
        .audit(
            &user,
            "rainmill.vault.credential.delete",
            Some("vault_items"),
            Some(&id.to_string()),
            Some(&item.label),
            Some(&item.kind),
        )
        .await;
    session.push_flash(FlashLevel::Success, t!("The credential was deleted."));
    Redirect::to(&cx.index()).into_response()
}

/// `GET <id>/reveal/<field>`: a stored text secret, as plain text for the
/// field's eye button. Asks for the reveal permission and is recorded on the
/// audit log. A file, or a field that is not a secret, is not found.
async fn reveal(cx: &Cx, shell: Shell, user: AuthenticatedUser, id: i64, field: &str) -> Response {
    if !shell.allows(REVEAL) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let db = cx.ctx.db();
    let item = match store::find(db, id).await {
        Ok(Some(item)) => item,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return failure(e),
    };
    let is_text_secret = cx
        .kind(&item.kind)
        .and_then(|kind| kind.fields.iter().find(|f| f.name == field))
        .is_some_and(|f| secret(f) == (true, false));
    if !is_text_secret {
        return StatusCode::NOT_FOUND.into_response();
    }
    let value = match store::fields(db, cx.ctx.crypt(), id).await {
        Ok(fields) => fields.get(field).cloned().unwrap_or_default(),
        Err(e) => return failure(e),
    };
    cx.ctx
        .audit(
            &user,
            "rainmill.vault.credential.reveal",
            Some("vault_items"),
            Some(&id.to_string()),
            Some(&item.label),
            Some(field),
        )
        .await;
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        value,
    )
        .into_response()
}
