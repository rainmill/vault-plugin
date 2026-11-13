//! What other modules contribute to the vault: the kinds of credential they
//! understand, and the slots they need one for.

use laterite_admin::form::FormField;
use laterite_core::{t, Text};

/// A kind of credential: a code, a name, and the fields one holds.
///
/// ```ignore
/// registry.add(
///     KindReg::new("acme.token", t!("Acme API token"))
///         .field(FormField::of("token", t!("API token"), "sensitive").required()),
/// );
/// ```
#[derive(Clone)]
pub struct KindReg {
    /// The kind's dotted code (`acme.token`).
    pub code: String,
    /// The kind's name, as an administrator reads it.
    pub label: Text,
    /// A sentence under the name on the form: where to find the value.
    pub help: Option<Text>,
    /// The fields a credential of this kind holds. A `sensitive` field is a
    /// secret; any other is shown as it is.
    pub fields: Vec<FormField>,
    /// What the credential is for; kinds are offered under it.
    pub category: Category,
}

/// What a kind of credential is for. The Credentials screen offers the kinds
/// grouped under these, in this order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Category {
    /// WhatsApp, SMS, push and chat.
    Messaging,
    /// Mail delivery.
    Email,
    /// Payment processors and payouts.
    Payments,
    /// Language and vision models.
    Ai,
    /// Scrapers, feeds and third-party data.
    Data,
    /// Maps, geocoding and places.
    Maps,
    /// Object storage and file hosts.
    Storage,
    /// Sign-in providers and identity checks.
    Identity,
    /// Anything else.
    #[default]
    Other,
}

impl Category {
    /// The category's name, as an administrator reads it.
    pub fn label(self) -> Text {
        match self {
            Self::Messaging => t!("Messaging"),
            Self::Email => t!("Email"),
            Self::Payments => t!("Payments"),
            Self::Ai => t!("AI"),
            Self::Data => t!("Data"),
            Self::Maps => t!("Maps"),
            Self::Storage => t!("Storage"),
            Self::Identity => t!("Sign-in and identity"),
            Self::Other => t!("Other"),
        }
    }
}

impl KindReg {
    /// A kind with no fields yet, under [`Category::Other`].
    pub fn new(code: impl Into<String>, label: impl Into<Text>) -> Self {
        Self {
            code: code.into(),
            label: label.into(),
            help: None,
            fields: Vec::new(),
            category: Category::Other,
        }
    }

    /// Files the kind under `category`.
    pub fn category(mut self, category: Category) -> Self {
        self.category = category;
        self
    }

    /// Adds a field. Its name is a plain identifier and none of
    /// [`RESERVED_FIELDS`].
    pub fn field(mut self, field: FormField) -> Self {
        self.fields.push(field);
        self
    }

    /// A sentence shown with the kind's fields.
    pub fn help(mut self, help: impl Into<Text>) -> Self {
        self.help = Some(help.into());
        self
    }
}

/// Names the credential form uses itself; a kind's field cannot take one.
pub const RESERVED_FIELDS: &[&str] = &["label", "allowance", "period_day", "expires_at"];

/// A module's need for a credential of a kind.
///
/// ```ignore
/// registry.add(
///     SlotReg::pooled("acme.stock.api", "acme.token")
///         .label(t!("Stock sync"))
///         .reason(t!("Reads stock levels through the day")),
/// );
/// ```
#[derive(Clone)]
pub struct SlotReg {
    /// The slot's dotted code, which the module leases by.
    pub code: String,
    /// The kind of credential it takes.
    pub kind: String,
    /// The slot's name, as an administrator reads it.
    pub label: Text,
    /// What the module does with the credential.
    pub reason: Option<Text>,
    /// Whether several credentials may be attached and cycled through.
    pub pooled: bool,
    /// What one unit is, in the plural, as the usage screen names it.
    pub unit: Option<Text>,
    /// The meters its use is counted under, each with its own price.
    pub meters: Vec<(String, Text)>,
}

impl SlotReg {
    /// A slot that takes one credential: attaching another replaces it.
    pub fn single(code: impl Into<String>, kind: impl Into<String>) -> Self {
        let code = code.into();
        Self {
            label: Text::dynamic(code.clone()),
            code,
            kind: kind.into(),
            reason: None,
            pooled: false,
            unit: None,
            meters: Vec::new(),
        }
    }

    /// A slot that takes any number of credentials and cycles through them.
    pub fn pooled(code: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            pooled: true,
            ..Self::single(code, kind)
        }
    }

    /// The slot's name.
    pub fn label(mut self, label: impl Into<Text>) -> Self {
        self.label = label.into();
        self
    }

    /// What the module does with the credential, shown where one is attached.
    pub fn reason(mut self, reason: impl Into<Text>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// What one unit is, in the plural: "profiles", "messages".
    pub fn unit(mut self, unit: impl Into<Text>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// Adds a meter: a split of the slot's use that is counted and priced on
    /// its own. The module reports a use under it with
    /// [`Lease::used_as`](crate::Lease::used_as).
    pub fn meter(mut self, code: impl Into<String>, label: impl Into<Text>) -> Self {
        self.meters.push((code.into(), label.into()));
        self
    }
}
