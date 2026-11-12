//! The vault's schema, as portable one-file migrations. Append new entries at
//! the end; never reorder or rename a shipped one.

laterite_core::migration_set! {
    module_id: "rainmill.vault",
    m0001_create_vault,
    m0002_add_meters_and_rates,
}
