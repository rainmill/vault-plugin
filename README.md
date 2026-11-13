# Rainmill Vault

A first-party [Laterite](https://github.com/lateritecmf/laterite) plugin: the
**credentials your application calls other services with**, kept in one place.

- A module declares the **kinds** of credential it understands (an API token,
  a key file) and the **slots** it needs one for.
- An administrator stores credentials under **Settings > System >
  Credentials**, each with a label, and attaches them to what uses them.
- A slot may take **several** credentials and use each in turn. One that a
  provider refused, that has expired, or that has spent its **allowance** for
  the period is passed over.
- What each credential spent is counted by day.

Fields are sealed at rest under the application key and never sent to the
page; revealing one asks for its own permission and is written to the audit
log.

The grant is governance, not isolation: a module compiled into the application
shares its process.

## Requirements

- Laterite 0.11 or later.
- Database: **any** (PostgreSQL, MySQL, or SQLite).

## Installation

```sh
lat plugin add https://github.com/rainmill/vault-plugin
```

## Declare a kind and a slot

```rust
use laterite_admin::form::FormField;
use laterite_core::t;
use rainmill_vault::{KindReg, SlotReg};

registry.add(
    KindReg::new("acme.token", t!("Acme API token"))
        .help(t!("Acme > Settings > API."))
        .field(FormField::of("token", t!("API token"), "sensitive").required()),
);
registry.add(
    SlotReg::pooled("acme.stock.api", "acme.token")
        .label(t!("Stock sync"))
        .reason(t!("Reads stock levels through the day")),
);
```

| Call | Description |
|---|---|
| `KindReg::new(code, label)` | A kind of credential. |
| `.field(FormField)` | A field it holds; a `sensitive` field is a secret, with `file: true` an uploaded key. |
| `.help(text)` | A sentence shown with its fields. |
| `SlotReg::single(code, kind)` | A slot for one credential; attaching another replaces it. |
| `SlotReg::pooled(code, kind)` | A slot for several, used in turn. |
| `.label(text)`, `.reason(text)` | What an administrator reads where a credential is attached. |

A kind or slot that cannot work stops the boot: a code registered twice, a
kind's field named `label`, `allowance`, `period_day` or `expires_at`, a slot
for a kind nobody registered.

## Use a credential

```rust
let lease = rainmill_vault::lease(cx.db(), crypt, "acme.stock.api").await?;
let token = lease.field("token").unwrap_or_default();
match call(token).await {
    Ok(read) => lease.used(read).await?,
    Err(e) => lease.failed(e.status(), &e.to_string()).await?,
}
```

| Call | Description |
|---|---|
| `lease(db, crypt, slot)` | The next usable credential attached to the slot. |
| `lease.field(name)` | A field's value. An uploaded file is the document `laterite_core::SealedFile::parse` reads. |
| `lease.label()` | The label an administrator gave it, for a log line. |
| `lease.remaining()` | Units left this period, when it has an allowance. |
| `lease.used(units)` | It worked and spent `units`. |
| `lease.used_as(meter, units)` | It worked and spent `units`, counted under one of the slot's meters. |
| `lease.failed(status, message)` | It failed. A 401, 402 or 403 takes it out of the cycle for an hour and shows it as refused. |

| Error | Meaning |
|---|---|
| `VaultError::Unbound` | Nothing is attached to the slot. |
| `VaultError::NoneUsable` | Everything attached is refused, expired or at its allowance; the error names each. |

## Add a button to a credential's page

```rust
use std::sync::Arc;
use rainmill_vault::action::{Action, ActionCx, Line, Outcome, Report, State};
use rainmill_vault::ActionReg;

struct TestToken;

#[async_trait]
impl Action for TestToken {
    async fn run(&self, cx: &ActionCx<'_>) -> Outcome {
        let token = cx.field("token").unwrap_or_default();
        match acme::ping(token).await {
            Ok(account) => Outcome::Report(Report {
                lines: vec![Line {
                    label: t!("API token"),
                    state: State::Works,
                    detail: format!("Signed in as {account}."),
                }],
                accepted: Some(true),
                ..Report::default()
            }),
            Err(e) => Outcome::Failed(Text::dynamic(e.to_string())),
        }
    }
}

registry.add(
    ActionReg::new("acme.token", "test", t!("Test connection"), Arc::new(TestToken)).icon("plug"),
);
```

Answer | Shown
--- | ---
`Outcome::Done(text)` | A success message on the credential's page.
`Outcome::Failed(text)` | An error message on the credential's page.
`Outcome::Report(report)` | A page with one row a `Line`.

`Report` field | Effect
--- | ---
`note` | A sentence above the rows.
`lines` | The rows: `State::Works`, `Failed` or `NotTried`, with a detail.
`accepted` | `Some(true)` puts a refused credential back in good standing.
`offer` | Codes of the kind's other actions, shown as buttons on the report.

`ActionReg::unlisted()` keeps a button off the credential's page, for an action
a report offers.

A report whose rows take a call each fills in row by row:

```rust
#[async_trait]
impl Action for TestToken {
    fn parts(&self) -> Vec<Part> {
        vec![Part::new("token", t!("API token")), Part::new("quota", t!("Quota"))]
    }

    async fn part(&self, cx: &ActionCx<'_>, code: &str) -> PartResult {
        match code {
            "token" => PartResult::new(State::Works, "Signed in.").accepted(),
            _ => PartResult::new(State::Failed, "Nothing left this month.").offer("top-up"),
        }
    }
}
```

`PartResult::offer(code)` puts that action's button in the row.

## Usage and cost

```rust
registry.add(
    SlotReg::pooled("acme.messages.api", "acme.token")
        .label(t!("Messages"))
        .unit(t!("Messages sent"))
        .meter("marketing", t!("Marketing"))
        .meter("utility", t!("Utility")),
);
```

Each slot has a Usage page under Credentials: a period bar, charts by meter and
by credential, and calls against errors. A price a unit, set on the page for each
meter, adds an estimated cost and a projection for the month.

## Permissions

| Code | Grants |
|---|---|
| `rainmill.vault.manage` | The Credentials screen: add, change, attach and delete. |
| `rainmill.vault.reveal` | Revealing a stored secret. |

## License

MIT or Apache-2.0, at your option.
