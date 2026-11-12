//! Count usage per meter (a slot's own split of what it spends, such as a
//! message category), and store a price for each slot and meter.

use laterite_core::strata::*;

use crate::schema::{VaultRates as Rates, VaultUsage as Usage};

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0002_add_meters_and_rates"
    }

    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(Usage::Table)
                .add_column(key_col(Usage::Meter).not_null().default(""))
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::drop()
                .name("vault_usage_item_slot_day_idx")
                .table(Usage::Table)
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("vault_usage_item_slot_day_meter_idx")
                .table(Usage::Table)
                .col(Usage::ItemId)
                .col(Usage::Slot)
                .col(Usage::Day)
                .col(Usage::Meter)
                .unique()
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Rates::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(Rates::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(Rates::Slot).not_null())
                .col(key_col(Rates::Meter).not_null().default(""))
                .col(ColumnDef::new(Rates::Price).text().not_null())
                .col(key_col(Rates::Currency).not_null())
                .col(ColumnDef::new(Rates::UpdatedAt).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("vault_rates_slot_meter_idx")
                .table(Rates::Table)
                .col(Rates::Slot)
                .col(Rates::Meter)
                .unique()
                .to_owned(),
        )
        .await
    }
}
