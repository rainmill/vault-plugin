//! What an item belongs to: a record of a registered scope, such as an app.
//! An item with no row here is global.

use laterite_core::strata::*;

use crate::schema::{VaultItemScopes as Scopes, VaultItems as Items};

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0003_add_item_scopes"
    }

    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Scopes::Table)
                .col(
                    ColumnDef::new(Scopes::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Scopes::ItemId).big_integer().not_null())
                .col(key_col(Scopes::ScopeType).not_null())
                .col(ColumnDef::new(Scopes::ScopeId).big_integer().not_null())
                .foreign_key(
                    ForeignKey::create()
                        .from(Scopes::Table, Scopes::ItemId)
                        .to(Items::Table, Items::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
        // An index, never the primary key: one scope an item today, widened
        // later by replacing it.
        s.exec(
            Index::create()
                .name("vault_item_scopes_item_idx")
                .table(Scopes::Table)
                .col(Scopes::ItemId)
                .unique()
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("vault_item_scopes_scope_idx")
                .table(Scopes::Table)
                .col(Scopes::ScopeType)
                .col(Scopes::ScopeId)
                .to_owned(),
        )
        .await
    }
}
