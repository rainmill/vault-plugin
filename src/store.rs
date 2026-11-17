//! The vault's rows: items, their bindings to slots, and what they spent.
//!
//! An item's fields are one JSON object of plain values, sealed whole under
//! the application key (the `field` purpose, the column's name as the AAD), so
//! `lat crypt rotate` re-seals the column like any other.

use laterite_core::serde_json;
use std::collections::{BTreeMap, HashMap};

use chrono::{Datelike, NaiveDate, SecondsFormat, Utc};
use laterite_core::crypt::FIELD_PURPOSE;
use laterite_core::query::{bind_values, build, insert_returning_id, on_conflict_ignore};
use laterite_core::sea_query;
use laterite_core::sqlx::{self, Row};
use laterite_core::{AnyRowExt, Crypt, Db};
use sea_query::{Expr, Iden, Order, Query};

use crate::schema::{
    VaultBindings as Bindings, VaultItemScopes as Scopes, VaultItems as Items, VaultRates as Rates,
    VaultUsage as Usage,
};
use crate::VaultError;

/// The column the fields are sealed in, which is also the AAD.
pub(crate) const FIELDS_COLUMN: &str = "fields";

/// How long a credential a provider refused is left alone before it is tried
/// again.
pub(crate) const COOL_OFF_MINUTES: i64 = 60;

/// An item as a screen lists it: everything but its fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Id of the item's row.
    pub id: i64,
    /// The registered kind's code.
    pub kind: String,
    /// The name an administrator gave it.
    pub label: String,
    /// Whether a provider refused it last time it was used.
    pub failing: bool,
    /// Units it may spend a period; `None` is unlimited.
    pub allowance: Option<i64>,
    /// The day of the month its period starts.
    pub period_day: u32,
    /// The first day of the current period, `YYYY-MM-DD`.
    pub period_start: String,
    /// Units spent this period.
    pub used: i64,
    /// When it stops being valid, if it does.
    pub expires_at: Option<String>,
    /// When it was last used.
    pub last_used_at: Option<String>,
    /// What the provider said when it last refused.
    pub last_error: Option<String>,
    /// When it last refused.
    pub last_error_at: Option<String>,
}

/// Where a credential stands, for a screen and for the cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Usable.
    Ok,
    /// A provider refused it; it is left alone for a while, then tried again.
    Failing,
    /// At its allowance until its period turns.
    Spent,
    /// Past its expiry date.
    Expired,
}

impl Item {
    /// Where the item stands now.
    pub fn standing(&self) -> Standing {
        self.standing_at(&now())
    }

    fn standing_at(&self, now: &str) -> Standing {
        if self.expires_at.as_deref().is_some_and(|at| at < now) {
            Standing::Expired
        } else if self.failing {
            Standing::Failing
        } else if self.remaining() == Some(0) {
            Standing::Spent
        } else {
            Standing::Ok
        }
    }

    /// Units left this period; `None` when it has no allowance.
    pub fn remaining(&self) -> Option<i64> {
        self.allowance.map(|a| (a - self.used).max(0))
    }

    /// Whether the cycle may hand it out: usable, or failing for long enough
    /// to be worth another try.
    fn leasable(&self, now: &str) -> bool {
        match self.standing_at(now) {
            Standing::Ok => true,
            Standing::Failing => {
                let again = (Utc::now() - chrono::Duration::minutes(COOL_OFF_MINUTES))
                    .to_rfc3339_opts(SecondsFormat::Micros, true);
                self.last_error_at
                    .as_deref()
                    .is_none_or(|at| at < again.as_str())
                    && self.remaining() != Some(0)
            }
            Standing::Spent | Standing::Expired => false,
        }
    }
}

/// What a new or changed item holds.
#[derive(Debug, Clone, Default)]
pub struct ItemInput {
    /// The name an administrator gives it.
    pub label: String,
    /// The kind's fields, as plain values.
    pub fields: BTreeMap<String, String>,
    /// Units it may spend a period.
    pub allowance: Option<i64>,
    /// The day of the month its period starts, 1 to 28.
    pub period_day: u32,
    /// When it stops being valid.
    pub expires_at: Option<String>,
}

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// The first day of the period that holds `today`, for a period starting on
/// `day` of each month.
pub(crate) fn period_start(today: NaiveDate, day: u32) -> NaiveDate {
    let day = day.clamp(1, 28);
    let (year, month) = if today.day() >= day {
        (today.year(), today.month())
    } else if today.month() == 1 {
        (today.year() - 1, 12)
    } else {
        (today.year(), today.month() - 1)
    };
    NaiveDate::from_ymd_opt(year, month, day).unwrap_or(today)
}

fn seal(crypt: &Crypt, fields: &BTreeMap<String, String>) -> Result<String, VaultError> {
    let plain = serde_json::to_string(fields).map_err(|e| VaultError::Store(e.to_string()))?;
    crypt
        .encrypt(FIELD_PURPOSE, FIELDS_COLUMN.as_bytes(), plain.as_bytes())
        .map_err(|_| VaultError::Sealed)
}

fn open(crypt: &Crypt, sealed: &str) -> Result<BTreeMap<String, String>, VaultError> {
    let plain = crypt
        .decrypt(FIELD_PURPOSE, FIELDS_COLUMN.as_bytes(), sealed)
        .map_err(|_| VaultError::Sealed)?;
    serde_json::from_slice(&plain).map_err(|_| VaultError::Sealed)
}

const ITEM_COLUMNS: [Items; 12] = [
    Items::Id,
    Items::Kind,
    Items::Label,
    Items::State,
    Items::Allowance,
    Items::PeriodDay,
    Items::PeriodStart,
    Items::Used,
    Items::ExpiresAt,
    Items::LastUsedAt,
    Items::LastError,
    Items::LastErrorAt,
];

fn item_from(row: &sqlx::any::AnyRow) -> Result<Item, sqlx::Error> {
    Ok(Item {
        id: row.try_get::<i64, _>("id")?,
        kind: row.get_text("kind")?,
        label: row.get_text("label")?,
        failing: row.get_text("state")? == "failing",
        allowance: row.try_get::<Option<i64>, _>("allowance")?,
        period_day: u32::try_from(row.try_get::<i64, _>("period_day").unwrap_or(1)).unwrap_or(1),
        period_start: row.get_text("period_start")?,
        used: row.try_get::<i64, _>("used")?,
        expires_at: row.get_text_opt("expires_at")?.filter(|s| !s.is_empty()),
        last_used_at: row.get_text_opt("last_used_at")?,
        last_error: row.get_text_opt("last_error")?,
        last_error_at: row.get_text_opt("last_error_at")?,
    })
}

/// Stores a new item of `kind`, returning its id.
pub async fn create(
    db: &Db,
    crypt: &Crypt,
    kind: &str,
    input: &ItemInput,
) -> Result<i64, VaultError> {
    let at = now();
    let day = input.period_day.clamp(1, 28);
    let stmt = Query::insert()
        .into_table(Items::Table)
        .columns([
            Items::Kind,
            Items::Label,
            Items::Fields,
            Items::Allowance,
            Items::PeriodDay,
            Items::PeriodStart,
            Items::ExpiresAt,
            Items::CreatedAt,
            Items::UpdatedAt,
        ])
        .values_panic([
            kind.into(),
            input.label.trim().into(),
            seal(crypt, &input.fields)?.into(),
            input.allowance.into(),
            i64::from(day).into(),
            period_start(Utc::now().date_naive(), day)
                .to_string()
                .into(),
            input.expires_at.clone().into(),
            at.clone().into(),
            at.into(),
        ])
        .to_owned();
    Ok(insert_returning_id(db, stmt, Items::Id).await?)
}

/// Replaces an item's label, fields and limits. A new period day starts a new
/// period; what was spent is kept.
pub async fn update(db: &Db, crypt: &Crypt, id: i64, input: &ItemInput) -> Result<(), VaultError> {
    let day = input.period_day.clamp(1, 28);
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Items::Table)
            .value(Items::Label, input.label.trim())
            .value(Items::Fields, seal(crypt, &input.fields)?)
            .value(Items::Allowance, input.allowance)
            .value(Items::PeriodDay, i64::from(day))
            .value(
                Items::PeriodStart,
                period_start(Utc::now().date_naive(), day).to_string(),
            )
            .value(Items::ExpiresAt, input.expires_at.clone())
            // New values deserve a fresh try.
            .value(Items::State, "ok")
            .value(Items::UpdatedAt, now())
            .and_where(Expr::col(Items::Id).eq(id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Removes an item, with its bindings and usage.
pub async fn delete(db: &Db, id: i64) -> Result<(), VaultError> {
    // Built first: a statement builder must not live across an await.
    let statements = [
        build(
            db.backend,
            Query::delete()
                .from_table(Scopes::Table)
                .and_where(Expr::col(Scopes::ItemId).eq(id))
                .to_owned(),
        ),
        build(
            db.backend,
            Query::delete()
                .from_table(Bindings::Table)
                .and_where(Expr::col(Bindings::ItemId).eq(id))
                .to_owned(),
        ),
        build(
            db.backend,
            Query::delete()
                .from_table(Usage::Table)
                .and_where(Expr::col(Usage::ItemId).eq(id))
                .to_owned(),
        ),
        build(
            db.backend,
            Query::delete()
                .from_table(Items::Table)
                .and_where(Expr::col(Items::Id).eq(id))
                .to_owned(),
        ),
    ];
    for (sql, values) in statements {
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await?;
    }
    Ok(())
}

/// Every item, by label, each with its period brought up to date.
pub async fn list(db: &Db) -> Result<Vec<Item>, VaultError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(ITEM_COLUMNS)
            .from(Items::Table)
            .order_by(Items::Label, Order::Asc)
            .order_by(Items::Id, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(turn_period(db, item_from(row)?).await?);
    }
    Ok(items)
}

/// One item, with its period brought up to date.
pub async fn find(db: &Db, id: i64) -> Result<Option<Item>, VaultError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(ITEM_COLUMNS)
            .from(Items::Table)
            .and_where(Expr::col(Items::Id).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    match row {
        Some(row) => Ok(Some(turn_period(db, item_from(&row)?).await?)),
        None => Ok(None),
    }
}

/// An item's fields, opened.
pub async fn fields(
    db: &Db,
    crypt: &Crypt,
    id: i64,
) -> Result<BTreeMap<String, String>, VaultError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(Items::Fields)
            .from(Items::Table)
            .and_where(Expr::col(Items::Id).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?
        .ok_or(VaultError::Missing)?;
    open(crypt, &row.get_text("fields")?)
}

/// Starts a new period for an item whose stored one has passed: what it spent
/// goes back to nothing.
async fn turn_period(db: &Db, mut item: Item) -> Result<Item, VaultError> {
    let current = period_start(Utc::now().date_naive(), item.period_day).to_string();
    if item.period_start == current {
        return Ok(item);
    }
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Items::Table)
            .value(Items::PeriodStart, current.clone())
            .value(Items::Used, 0)
            .and_where(Expr::col(Items::Id).eq(item.id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    item.period_start = current;
    item.used = 0;
    Ok(item)
}

/// The slots an item is attached to.
pub async fn slots_of(db: &Db, item_id: i64) -> Result<Vec<String>, VaultError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(Bindings::Slot)
            .from(Bindings::Table)
            .and_where(Expr::col(Bindings::ItemId).eq(item_id))
            .order_by(Bindings::Slot, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter().map(|r| Ok(r.get_text("slot")?)).collect()
}

/// The items attached to a slot, in their order.
pub async fn items_of(db: &Db, slot: &str) -> Result<Vec<Item>, VaultError> {
    Ok(bound(db, slot, Within::Any)
        .await?
        .into_iter()
        .map(|(_, i)| i)
        .collect())
}

/// What an item belongs to: a record of a registered scope. An item with none
/// is global.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Scope {
    /// The registered scope's code.
    pub kind: String,
    /// The id of the record.
    pub id: i64,
}

impl Scope {
    /// The record `id` of the scope `kind`.
    pub fn new(kind: impl Into<String>, id: i64) -> Self {
        Self {
            kind: kind.into(),
            id,
        }
    }

    /// A scope from its `kind:id` form.
    pub fn parse(text: &str) -> Option<Self> {
        let (kind, id) = text.rsplit_once(':')?;
        (!kind.is_empty()).then_some(())?;
        Some(Self::new(kind, id.parse().ok()?))
    }

    /// The `kind:id` form.
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind, self.id)
    }
}

/// Which of a slot's items a read takes.
#[derive(Clone, Copy)]
enum Within<'a> {
    /// Every item, whatever it belongs to.
    Any,
    /// Global items alone.
    Global,
    /// The items of one scope.
    Of(&'a Scope),
}

/// What an item belongs to, or `None` for a global item.
pub async fn scope_of(db: &Db, item_id: i64) -> Result<Option<Scope>, VaultError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([Scopes::ScopeType, Scopes::ScopeId])
            .from(Scopes::Table)
            .and_where(Expr::col(Scopes::ItemId).eq(item_id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    Ok(match row {
        Some(row) => Some(Scope::new(
            row.get_text("scope_type")?,
            row.get_int("scope_id")?,
        )),
        None => None,
    })
}

/// Every scoped item's scope, by item id. An item missing here is global.
pub async fn scopes(db: &Db) -> Result<HashMap<i64, Scope>, VaultError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([Scopes::ItemId, Scopes::ScopeType, Scopes::ScopeId])
            .from(Scopes::Table)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    let mut out = HashMap::with_capacity(rows.len());
    for row in &rows {
        out.insert(
            row.get_int("item_id")?,
            Scope::new(row.get_text("scope_type")?, row.get_int("scope_id")?),
        );
    }
    Ok(out)
}

/// The records of a scope as `(id, name)`, by name.
pub async fn scope_records(
    db: &Db,
    table: &str,
    name_column: &str,
) -> Result<Vec<(i64, String)>, VaultError> {
    use sea_query::Alias;
    let (sql, values) = build(
        db.backend,
        Query::select()
            .expr_as(Expr::col(Alias::new("id")), Alias::new("id"))
            .expr_as(Expr::col(Alias::new(name_column)), Alias::new("name"))
            .from(Alias::new(table))
            .order_by(Alias::new(name_column), Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter()
        .map(|row| Ok((row.get_int("id")?, row.get_text("name")?)))
        .collect()
}

/// Says what an item belongs to; `None` makes it global.
pub async fn set_scope(db: &Db, item_id: i64, scope: Option<&Scope>) -> Result<(), VaultError> {
    // Built first: a statement builder must not live across an await.
    let clear = build(
        db.backend,
        Query::delete()
            .from_table(Scopes::Table)
            .and_where(Expr::col(Scopes::ItemId).eq(item_id))
            .to_owned(),
    );
    let set = scope.map(|scope| {
        build(
            db.backend,
            Query::insert()
                .into_table(Scopes::Table)
                .columns([Scopes::ItemId, Scopes::ScopeType, Scopes::ScopeId])
                .values_panic([item_id.into(), scope.kind.as_str().into(), scope.id.into()])
                .to_owned(),
        )
    });
    for (sql, values) in std::iter::once(clear).chain(set) {
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await?;
    }
    Ok(())
}

/// A slot's bindings as `(binding id, item)`, the one handed out longest ago
/// first, then in their order.
async fn bound(db: &Db, slot: &str, within: Within<'_>) -> Result<Vec<(i64, Item)>, VaultError> {
    // Scoped: a statement builder must not live across an await.
    let (sql, values) = {
        let mut select = Query::select();
        select
            .expr_as(Expr::col((Bindings::Table, Bindings::Id)), BindingId)
            .from(Bindings::Table)
            .inner_join(
                Items::Table,
                Expr::col((Items::Table, Items::Id)).equals((Bindings::Table, Bindings::ItemId)),
            )
            .and_where(Expr::col((Bindings::Table, Bindings::Slot)).eq(slot))
            .order_by((Bindings::Table, Bindings::LeasedAt), Order::Asc);
        if !matches!(within, Within::Any) {
            select.left_join(
                Scopes::Table,
                Expr::col((Scopes::Table, Scopes::ItemId)).equals((Items::Table, Items::Id)),
            );
        }
        match within {
            Within::Any => {}
            Within::Global => {
                select.and_where(Expr::col((Scopes::Table, Scopes::Id)).is_null());
            }
            Within::Of(scope) => {
                select
                    .and_where(
                        Expr::col((Scopes::Table, Scopes::ScopeType)).eq(scope.kind.as_str()),
                    )
                    .and_where(Expr::col((Scopes::Table, Scopes::ScopeId)).eq(scope.id));
            }
        }
        select
            .order_by((Bindings::Table, Bindings::Position), Order::Asc)
            .order_by((Bindings::Table, Bindings::Id), Order::Asc);
        for column in ITEM_COLUMNS {
            select.column((Items::Table, column));
        }
        build(db.backend, select)
    };
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let binding = row.try_get::<i64, _>("binding_id")?;
        out.push((binding, turn_period(db, item_from(row)?).await?));
    }
    Ok(out)
}

#[derive(Iden)]
struct BindingId;

/// Attaches an item to a slot, last in its order. In a slot that takes one
/// credential (`pooled` false) it replaces whatever was attached.
pub async fn bind(db: &Db, slot: &str, item_id: i64, pooled: bool) -> Result<(), VaultError> {
    if !pooled {
        // One credential for the slot in each scope: the app's own beside the
        // global one.
        let scope = scope_of(db, item_id).await?;
        let within = scope.as_ref().map_or(Within::Global, Within::Of);
        for (_, other) in bound(db, slot, within).await? {
            if other.id != item_id {
                unbind(db, slot, other.id).await?;
            }
        }
    }
    let position = i64::try_from(bound(db, slot, Within::Any).await?.len()).unwrap_or(0);
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(Bindings::Table)
            .columns([
                Bindings::Slot,
                Bindings::ItemId,
                Bindings::Position,
                Bindings::CreatedAt,
            ])
            .values_panic([slot.into(), item_id.into(), position.into(), now().into()])
            .on_conflict(on_conflict_ignore([Bindings::Slot, Bindings::ItemId]))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Detaches an item from a slot.
pub async fn unbind(db: &Db, slot: &str, item_id: i64) -> Result<(), VaultError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(Bindings::Table)
            .and_where(Expr::col(Bindings::Slot).eq(slot))
            .and_where(Expr::col(Bindings::ItemId).eq(item_id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// The next credential of `slot` the cycle may hand out, with its fields
/// opened, and marks it handed out now.
///
/// With a `scope`, the credentials that belong to it are the ones cycled, and
/// the global ones when it has none. Without, the global ones alone.
pub(crate) async fn take(
    db: &Db,
    crypt: &Crypt,
    slot: &str,
    scope: Option<&Scope>,
) -> Result<(Item, BTreeMap<String, String>), VaultError> {
    let own = match scope {
        Some(scope) => bound(db, slot, Within::Of(scope)).await?,
        None => Vec::new(),
    };
    let bound = if own.is_empty() {
        bound(db, slot, Within::Global).await?
    } else {
        own
    };
    if bound.is_empty() {
        return Err(VaultError::Unbound(slot.to_string()));
    }
    let at = now();
    let Some((binding, item)) = bound.iter().find(|(_, item)| item.leasable(&at)) else {
        return Err(VaultError::NoneUsable {
            slot: slot.to_string(),
            standing: bound
                .iter()
                .map(|(_, item)| (item.label.clone(), item.standing_at(&at)))
                .collect(),
        });
    };
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Bindings::Table)
            .value(Bindings::LeasedAt, at)
            .and_where(Expr::col(Bindings::Id).eq(*binding))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    let fields = fields(db, crypt, item.id).await?;
    Ok((item.clone(), fields))
}

/// Records a use of an item for a slot: `units` against its allowance, and
/// the item back in good standing.
pub(crate) async fn spent(
    db: &Db,
    item_id: i64,
    slot: &str,
    meter: &str,
    units: i64,
) -> Result<(), VaultError> {
    let units = units.max(0);
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Items::Table)
            .value(Items::Used, Expr::col(Items::Used).add(units))
            .value(Items::LastUsedAt, now())
            .value(Items::State, "ok")
            .and_where(Expr::col(Items::Id).eq(item_id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    count(db, item_id, slot, meter, 1, 0, units).await
}

/// Puts an item a provider refused back in good standing.
pub(crate) async fn reinstate(db: &Db, item_id: i64) -> Result<(), VaultError> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Items::Table)
            .value(Items::State, "ok")
            .and_where(Expr::col(Items::Id).eq(item_id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Records a failed use. A refusal (the provider would not take the
/// credential) puts the item out of the cycle for a while.
pub(crate) async fn failed(
    db: &Db,
    item_id: i64,
    slot: &str,
    refused: bool,
    message: &str,
) -> Result<(), VaultError> {
    let (sql, values) = {
        let mut update = Query::update();
        update
            .table(Items::Table)
            .value(
                Items::LastError,
                message.chars().take(500).collect::<String>(),
            )
            .value(Items::LastErrorAt, now())
            .and_where(Expr::col(Items::Id).eq(item_id));
        if refused {
            update.value(Items::State, "failing");
        }
        build(db.backend, update)
    };
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    count(db, item_id, slot, "", 0, 1, 0).await
}

/// Adds to the day's counts for an item, a slot and a meter.
async fn count(
    db: &Db,
    item_id: i64,
    slot: &str,
    meter: &str,
    calls: i64,
    errors: i64,
    units: i64,
) -> Result<(), VaultError> {
    let day = Utc::now().date_naive().to_string();
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(Usage::Table)
            .columns([Usage::ItemId, Usage::Slot, Usage::Day, Usage::Meter])
            .values_panic([
                item_id.into(),
                slot.into(),
                day.clone().into(),
                meter.into(),
            ])
            .on_conflict(on_conflict_ignore([
                Usage::ItemId,
                Usage::Slot,
                Usage::Day,
                Usage::Meter,
            ]))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Usage::Table)
            .value(Usage::Calls, Expr::col(Usage::Calls).add(calls))
            .value(Usage::Errors, Expr::col(Usage::Errors).add(errors))
            .value(Usage::Units, Expr::col(Usage::Units).add(units))
            .and_where(Expr::col(Usage::ItemId).eq(item_id))
            .and_where(Expr::col(Usage::Slot).eq(slot))
            .and_where(Expr::col(Usage::Day).eq(day))
            .and_where(Expr::col(Usage::Meter).eq(meter))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// What an item spent on each of its last `days` days with any use, newest
/// first, as `(day, calls, errors, units)`.
///
/// Added up here across the item's slots: a database's own sum comes back as
/// a type that differs from one database to the next.
pub async fn usage(
    db: &Db,
    item_id: i64,
    days: usize,
) -> Result<Vec<(String, i64, i64, i64)>, VaultError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([Usage::Day, Usage::Calls, Usage::Errors, Usage::Units])
            .from(Usage::Table)
            .and_where(Expr::col(Usage::ItemId).eq(item_id))
            .order_by(Usage::Day, Order::Desc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    let mut out: Vec<(String, i64, i64, i64)> = Vec::new();
    for row in &rows {
        let day = row.get_text("day")?;
        let (calls, errors, units) = (
            row.try_get::<i64, _>("calls")?,
            row.try_get::<i64, _>("errors")?,
            row.try_get::<i64, _>("units")?,
        );
        if let Some(last) = out.last_mut().filter(|last| last.0 == day) {
            last.1 += calls;
            last.2 += errors;
            last.3 += units;
        } else if out.len() == days {
            break;
        } else {
            out.push((day, calls, errors, units));
        }
    }
    Ok(out)
}

/// What one credential spent for a slot and a meter on a day.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageRow {
    /// The day, in UTC.
    pub day: NaiveDate,
    /// The credential's row id.
    pub item_id: i64,
    /// The slot it was leased through.
    pub slot: String,
    /// The slot's meter, empty when the use named none.
    pub meter: String,
    /// Uses that succeeded.
    pub calls: i64,
    /// Uses that failed.
    pub errors: i64,
    /// Units spent.
    pub units: i64,
}

/// Every count from `from` to `to`, both days included, for one slot or for
/// all of them, oldest first.
pub async fn usage_between(
    db: &Db,
    slot: Option<&str>,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<UsageRow>, VaultError> {
    let (sql, values) = {
        let mut select = Query::select();
        select
            .columns([
                Usage::Day,
                Usage::ItemId,
                Usage::Slot,
                Usage::Meter,
                Usage::Calls,
                Usage::Errors,
                Usage::Units,
            ])
            .from(Usage::Table)
            .and_where(Expr::col(Usage::Day).gte(from.to_string()))
            .and_where(Expr::col(Usage::Day).lte(to.to_string()))
            .order_by(Usage::Day, Order::Asc);
        if let Some(slot) = slot {
            select.and_where(Expr::col(Usage::Slot).eq(slot));
        }
        build(db.backend, select)
    };
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let Ok(day) = NaiveDate::parse_from_str(&row.get_text("day")?, "%Y-%m-%d") else {
            continue;
        };
        out.push(UsageRow {
            day,
            item_id: row.try_get::<i64, _>("item_id")?,
            slot: row.get_text("slot")?,
            meter: row.get_text("meter")?,
            calls: row.try_get::<i64, _>("calls")?,
            errors: row.try_get::<i64, _>("errors")?,
            units: row.try_get::<i64, _>("units")?,
        });
    }
    Ok(out)
}

/// The price of one unit for a slot and a meter.
#[derive(Clone, Debug, PartialEq)]
pub struct Rate {
    /// The slot's code.
    pub slot: String,
    /// The slot's meter, empty for a slot without meters.
    pub meter: String,
    /// The price of one unit.
    pub price: f64,
    /// The price as it was entered.
    pub entered: String,
    /// The ISO currency code.
    pub currency: String,
}

/// Every stored rate.
pub async fn rates(db: &Db) -> Result<Vec<Rate>, VaultError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([Rates::Slot, Rates::Meter, Rates::Price, Rates::Currency])
            .from(Rates::Table)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let entered = row.get_text("price")?;
        out.push(Rate {
            slot: row.get_text("slot")?,
            meter: row.get_text("meter")?,
            price: entered.parse().unwrap_or(0.0),
            entered,
            currency: row.get_text("currency")?,
        });
    }
    Ok(out)
}

/// Stores the price of one unit for a slot and a meter, or removes it when
/// `price` is `None`.
pub async fn set_rate(
    db: &Db,
    slot: &str,
    meter: &str,
    price: Option<&str>,
    currency: &str,
) -> Result<(), VaultError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(Rates::Table)
            .and_where(Expr::col(Rates::Slot).eq(slot))
            .and_where(Expr::col(Rates::Meter).eq(meter))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    let Some(price) = price else {
        return Ok(());
    };
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(Rates::Table)
            .columns([
                Rates::Slot,
                Rates::Meter,
                Rates::Price,
                Rates::Currency,
                Rates::UpdatedAt,
            ])
            .values_panic([
                slot.into(),
                meter.into(),
                price.into(),
                currency.into(),
                now().into(),
            ])
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_period_starts_on_its_day_this_month_or_last() {
        let date = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(period_start(date(2026, 11, 12), 11), date(2026, 11, 11));
        assert_eq!(period_start(date(2026, 11, 10), 11), date(2026, 10, 11));
        assert_eq!(period_start(date(2026, 1, 3), 11), date(2025, 12, 11));
        assert_eq!(period_start(date(2026, 3, 31), 1), date(2026, 3, 1));
        // A day past 28 is held to 28, so every month has it.
        assert_eq!(period_start(date(2026, 2, 28), 31), date(2026, 2, 28));
    }
}
