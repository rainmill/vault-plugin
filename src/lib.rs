//! Rainmill Vault: the credentials a Laterite application calls other
//! services with.
//!
//! A module declares the kinds of credential it understands ([`KindReg`]) and
//! the slots it needs one for ([`SlotReg`]). An administrator stores
//! credentials on the Credentials screen, each with a label, and attaches them
//! to slots. A slot that is pooled takes several, and each [`lease`] hands out
//! the next one that is usable: in good standing, not expired, and under its
//! allowance for the period.
//!
//! ```ignore
//! let lease = rainmill_vault::lease(cx.db(), crypt, "acme.stock.api").await?;
//! let token = lease.field("token").unwrap_or_default();
//! match call(token).await {
//!     Ok(read) => lease.used(read).await?,
//!     Err(e) => lease.failed(e.status(), &e.to_string()).await?,
//! }
//! ```
//!
//! Fields are sealed at rest under the application key. The grant is
//! governance, not isolation: a module compiled into the application shares
//! its process.

pub mod action;
mod admin;
pub mod migrations;
mod registry;
mod schema;
pub mod store;

use std::collections::BTreeMap;

use laterite_core::module::BootCx;
use laterite_core::{CoreResult, Crypt, Db, MigrationSet, Module, ModuleId, Registry};

pub use action::ActionReg;
pub use registry::{Category, KindReg, ScopeReg, SlotReg, RESERVED_FIELDS};
pub use store::{Item, ItemInput, Scope, Standing};

/// The permission to see and change credentials and what they are used for.
pub const MANAGE: &str = "rainmill.vault.manage";
/// The permission to reveal a stored secret.
pub const REVEAL: &str = "rainmill.vault.reveal";

/// What can go wrong storing or leasing a credential.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// No credential is attached to the slot.
    #[error("no credential is attached to `{0}`")]
    Unbound(String),
    /// Credentials are attached, and none can be used now.
    #[error("no credential attached to `{slot}` can be used: {}", describe(.standing))]
    NoneUsable {
        /// The slot asked for.
        slot: String,
        /// Each attached credential's label and why it was passed over.
        standing: Vec<(String, Standing)>,
    },
    /// The stored fields do not open under the application key.
    #[error("the stored credential does not open under the application key")]
    Sealed,
    /// The item is gone.
    #[error("the credential no longer exists")]
    Missing,
    /// The database refused.
    #[error("the vault's store failed: {0}")]
    Store(String),
}

impl From<laterite_core::sqlx::Error> for VaultError {
    fn from(e: laterite_core::sqlx::Error) -> Self {
        Self::Store(e.to_string())
    }
}

impl From<laterite_core::CoreError> for VaultError {
    fn from(e: laterite_core::CoreError) -> Self {
        Self::Store(e.to_string())
    }
}

fn describe(standing: &[(String, Standing)]) -> String {
    standing
        .iter()
        .map(|(label, s)| {
            let why = match s {
                Standing::Ok => "usable",
                Standing::Failing => "refused by the provider",
                Standing::Spent => "at its allowance",
                Standing::Expired => "expired",
            };
            format!("{label} is {why}")
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// A credential handed out for one use. Report what came of it with
/// [`used`](Lease::used) or [`failed`](Lease::failed), so its allowance and
/// standing stay true; dropping it unreported spends nothing.
pub struct Lease {
    db: Db,
    slot: String,
    item: Item,
    fields: BTreeMap<String, String>,
}

/// Names the credential, never its fields.
impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("slot", &self.slot)
            .field("label", &self.item.label)
            .finish_non_exhaustive()
    }
}

impl Lease {
    /// A field's value, as stored. An uploaded file is the document
    /// `laterite_core::SealedFile::parse` reads.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// The label an administrator gave the credential, for a log line.
    pub fn label(&self) -> &str {
        &self.item.label
    }

    /// Id of the credential's row.
    pub fn id(&self) -> i64 {
        self.item.id
    }

    /// Units it may still spend this period; `None` when it has no allowance.
    pub fn remaining(&self) -> Option<i64> {
        self.item.remaining()
    }

    /// The use succeeded and spent `units` of the allowance.
    pub async fn used(self, units: i64) -> Result<(), VaultError> {
        store::spent(&self.db, self.item.id, &self.slot, "", units).await
    }

    /// The use succeeded and spent `units` of the allowance, counted under one
    /// of the slot's meters (see [`SlotReg::meter`]).
    pub async fn used_as(self, meter: &str, units: i64) -> Result<(), VaultError> {
        store::spent(&self.db, self.item.id, &self.slot, meter, units).await
    }

    /// The use failed. A `status` of 401, 402 or 403 means the provider would
    /// not take the credential: it leaves the cycle for a while and shows as
    /// failing. Any other failure is only counted.
    pub async fn failed(self, status: Option<u16>, message: &str) -> Result<(), VaultError> {
        let refused = matches!(status, Some(401..=403));
        store::failed(&self.db, self.item.id, &self.slot, refused, message).await
    }
}

/// Hands out the next usable credential attached to `slot`.
///
/// [`VaultError::Unbound`] when nothing is attached, and
/// [`VaultError::NoneUsable`] when everything attached is failing, spent or
/// expired.
pub async fn lease(db: &Db, crypt: &Crypt, slot: &str) -> Result<Lease, VaultError> {
    lease_in(db, crypt, slot, None).await
}

/// Like [`lease`], for one record of a scope (an app, say): its own
/// credentials are the ones used, and the global ones when it has none.
pub async fn lease_for(
    db: &Db,
    crypt: &Crypt,
    slot: &str,
    scope: &Scope,
) -> Result<Lease, VaultError> {
    lease_in(db, crypt, slot, Some(scope)).await
}

async fn lease_in(
    db: &Db,
    crypt: &Crypt,
    slot: &str,
    scope: Option<&Scope>,
) -> Result<Lease, VaultError> {
    let (item, fields) = store::take(db, crypt, slot, scope).await?;
    Ok(Lease {
        db: db.clone(),
        slot: slot.to_string(),
        item,
        fields,
    })
}

/// Checks the kinds and slots a registry holds can work together: no code
/// twice, no field of a kind under a name the form uses itself, no slot for a
/// kind nobody registered. The module runs it at boot.
pub fn check(registry: &Registry) -> CoreResult<()> {
    admin::check(registry)
}

/// This plugin's entry point. Every Laterite plugin exposes `module()`, so the
/// generated plugin manifest collects it without naming the type.
pub fn module() -> Box<dyn Module> {
    Box::new(VaultModule)
}

/// The vault as a Laterite module.
pub struct VaultModule;

#[laterite_core::strata::async_trait]
impl Module for VaultModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }

    fn migrations(&self) -> MigrationSet {
        migrations::migrations()
    }

    fn register(&self, registry: &mut Registry) {
        admin::register(registry);
    }

    async fn boot(&self, cx: &BootCx<'_>) -> CoreResult<()> {
        // On record for `lat crypt rotate`, which re-seals the column.
        laterite_core::crypt::register_column(
            cx.db,
            "vault_items",
            store::FIELDS_COLUMN,
            laterite_core::crypt::FIELD_PURPOSE,
        )
        .await?;
        admin::check(cx.registry)
    }
}
