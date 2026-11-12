//! Table and column identifiers, as `sea-query` `Iden` enums. The enum name
//! becomes the table name in snake_case.

use laterite_core::sea_query;
use sea_query::Iden;

/// A sealed item: a credential today, any sensitive item later.
#[derive(Iden)]
pub(crate) enum VaultItems {
    Table,
    Id,
    /// The registered kind's code.
    Kind,
    Label,
    /// The kind's fields as one JSON document, sealed.
    Fields,
    /// `ok` or `failing`. Spent and expired are worked out when read.
    State,
    /// Units it may spend a period; null is unlimited.
    Allowance,
    /// The day of the month its period starts, 1 to 28.
    PeriodDay,
    /// The first day of the period `used` counts, as `YYYY-MM-DD`.
    PeriodStart,
    Used,
    ExpiresAt,
    LastUsedAt,
    LastError,
    LastErrorAt,
    /// `team`: opened by the server. Personal items come later.
    Tier,
    OwnerId,
    /// The layout of `fields`, so a later one can be told apart.
    Format,
    CreatedAt,
    UpdatedAt,
}

/// An item attached to a slot. A pooled slot has several.
#[derive(Iden)]
pub(crate) enum VaultBindings {
    Table,
    Id,
    Slot,
    ItemId,
    Position,
    /// When this binding was last handed out; the cycle takes the oldest.
    LeasedAt,
    CreatedAt,
}

/// What an item spent for a slot on a day.
#[derive(Iden)]
pub(crate) enum VaultUsage {
    Table,
    Id,
    ItemId,
    Slot,
    Day,
    Calls,
    Errors,
    Units,
    /// The slot's own split of what it spends; empty when it has none.
    Meter,
}

/// The price of one unit for a slot and a meter.
#[derive(Iden)]
pub(crate) enum VaultRates {
    Table,
    Id,
    Slot,
    Meter,
    /// A decimal number, as text.
    Price,
    Currency,
    UpdatedAt,
}
