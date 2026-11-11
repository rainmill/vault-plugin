//! What other modules contribute to the vault: the kinds of credential they
//! understand, and the slots they need one for.

use laterite_admin::form::FormField;
use laterite_core::Text;

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
}

impl KindReg {
    /// A kind with no fields yet.
    pub fn new(code: impl Into<String>, label: impl Into<Text>) -> Self {
        Self {
            code: code.into(),
            label: label.into(),
            help: None,
            fields: Vec::new(),
        }
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
}
