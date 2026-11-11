//! Create the vault tables: items, the bindings of items to slots, and the
//! daily usage counts. Portable SQL only: auto-increment integer ids and text
//! timestamps.

use laterite_core::strata::*;

use crate::schema::{VaultBindings as Bindings, VaultItems as Items, VaultUsage as Usage};

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_vault"
    }

    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Items::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(Items::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(Items::Kind).not_null())
                .col(ColumnDef::new(Items::Label).text().not_null())
                .col(ColumnDef::new(Items::Fields).text().not_null())
                .col(key_col(Items::State).not_null().default("ok"))
                .col(ColumnDef::new(Items::Allowance).big_integer())
                .col(ColumnDef::new(Items::PeriodDay).integer().not_null().default(1))
                .col(ColumnDef::new(Items::PeriodStart).text().not_null())
                .col(ColumnDef::new(Items::Used).big_integer().not_null().default(0))
                .col(ColumnDef::new(Items::ExpiresAt).text())
                .col(ColumnDef::new(Items::LastUsedAt).text())
                .col(ColumnDef::new(Items::LastError).text())
                .col(ColumnDef::new(Items::LastErrorAt).text())
                .col(key_col(Items::Tier).not_null().default("team"))
                .col(ColumnDef::new(Items::OwnerId).big_integer())
                .col(ColumnDef::new(Items::Format).integer().not_null().default(1))
                .col(ColumnDef::new(Items::CreatedAt).text().not_null())
                .col(ColumnDef::new(Items::UpdatedAt).text().not_null())
                .to_owned(),
        )
        .await?;

        s.exec(
            Table::create()
                .table(Bindings::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(Bindings::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(Bindings::Slot).not_null())
                .col(ColumnDef::new(Bindings::ItemId).big_integer().not_null())
                .col(ColumnDef::new(Bindings::Position).integer().not_null().default(0))
                .col(ColumnDef::new(Bindings::LeasedAt).text().not_null().default(""))
                .col(ColumnDef::new(Bindings::CreatedAt).text().not_null())
                .foreign_key(
                    ForeignKey::create()
                        .from(Bindings::Table, Bindings::ItemId)
                        .to(Items::Table, Items::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
        // An index, never the primary key, so it can be widened later by
        // replacing it.
        s.exec(
            Index::create()
                .name("vault_bindings_slot_item_idx")
                .table(Bindings::Table)
                .col(Bindings::Slot)
                .col(Bindings::ItemId)
                .unique()
                .to_owned(),
        )
        .await?;

        s.exec(
            Table::create()
                .table(Usage::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(Usage::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Usage::ItemId).big_integer().not_null())
                .col(key_col(Usage::Slot).not_null())
                .col(key_col(Usage::Day).not_null())
                .col(ColumnDef::new(Usage::Calls).big_integer().not_null().default(0))
                .col(ColumnDef::new(Usage::Errors).big_integer().not_null().default(0))
                .col(ColumnDef::new(Usage::Units).big_integer().not_null().default(0))
                .foreign_key(
                    ForeignKey::create()
                        .from(Usage::Table, Usage::ItemId)
                        .to(Items::Table, Items::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("vault_usage_item_slot_day_idx")
                .table(Usage::Table)
                .col(Usage::ItemId)
                .col(Usage::Slot)
                .col(Usage::Day)
                .unique()
                .to_owned(),
        )
        .await
    }

    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(Usage::Table).to_owned()).await?;
        s.exec(Table::drop().table(Bindings::Table).to_owned()).await?;
        s.exec(Table::drop().table(Items::Table).to_owned()).await
    }
}
