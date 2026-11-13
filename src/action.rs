//! What another module adds to a credential's page: a button that acts on the
//! credential, and what the page shows when it has run.

use std::collections::BTreeMap;
use std::sync::Arc;

use laterite_core::strata::async_trait;
use laterite_core::{Crypt, Db, Text};

use crate::store::Item;

/// A button on the page of every credential of a kind.
///
/// ```ignore
/// registry.add(
///     ActionReg::new("acme.token", "test", t!("Test connection"), Arc::new(TestToken))
///         .icon("plug"),
/// );
/// ```
#[derive(Clone)]
pub struct ActionReg {
    /// The kind of credential the button is for.
    pub kind: String,
    /// The action's code, unique within the kind: letters, digits, `-`, `_`.
    pub code: String,
    /// The button's label.
    pub label: Text,
    /// The button's icon, by name.
    pub icon: Option<String>,
    /// Whether the credential's page shows the button. An unlisted action is
    /// reached only when a [`Report`] offers it.
    pub listed: bool,
    pub(crate) action: Arc<dyn Action>,
}

impl ActionReg {
    /// A listed button for credentials of `kind`.
    pub fn new(
        kind: impl Into<String>,
        code: impl Into<String>,
        label: impl Into<Text>,
        action: Arc<dyn Action>,
    ) -> Self {
        Self {
            kind: kind.into(),
            code: code.into(),
            label: label.into(),
            icon: None,
            listed: true,
            action,
        }
    }

    /// The button's icon, by name.
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Keeps the button off the credential's page: a [`Report`] offers it.
    pub fn unlisted(mut self) -> Self {
        self.listed = false;
        self
    }
}

/// What a button does.
#[async_trait]
pub trait Action: Send + Sync {
    /// Acts on the credential and says what came of it.
    async fn run(&self, cx: &ActionCx<'_>) -> Outcome;
}

/// The credential an action runs on, whatever its standing.
pub struct ActionCx<'a> {
    /// The database.
    pub db: &'a Db,
    /// The application's key service.
    pub crypt: &'a Crypt,
    pub(crate) item: &'a Item,
    pub(crate) fields: &'a BTreeMap<String, String>,
}

impl ActionCx<'_> {
    /// A field's value, as stored.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// The label an administrator gave the credential.
    pub fn label(&self) -> &str {
        &self.item.label
    }

    /// Id of the credential's row.
    pub fn id(&self) -> i64 {
        self.item.id
    }
}

/// What came of an action.
pub enum Outcome {
    /// It worked: said on the credential's page.
    Done(Text),
    /// It did not: said on the credential's page.
    Failed(Text),
    /// A page of its own: one line a part.
    Report(Report),
}

/// Several results, shown as a table.
#[derive(Default)]
pub struct Report {
    /// A sentence above the table.
    pub note: Option<Text>,
    /// The results, in order.
    pub lines: Vec<Line>,
    /// Whether the provider took the credential: `Some(true)` puts a refused
    /// credential back in good standing. Nothing here ever takes one out.
    pub accepted: Option<bool>,
    /// Codes of the kind's actions to offer as buttons above the table.
    pub offer: Vec<String>,
}

/// One result.
pub struct Line {
    /// What was tried.
    pub label: Text,
    /// How it fared.
    pub state: State,
    /// What the provider said.
    pub detail: String,
}

/// How a line fared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// It works.
    Works,
    /// It does not.
    Failed,
    /// It is stored, and only a later event proves it.
    NotTried,
}
