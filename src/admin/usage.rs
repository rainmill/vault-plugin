//! The Usage page: what a slot spent over a period, by meter and by
//! credential, what that costs at the stored prices, and where the month is
//! heading.

use std::collections::{BTreeMap, HashMap};

use chrono::{Datelike, Duration, NaiveDate, Utc};
use laterite_admin::axum::http::StatusCode;
use laterite_admin::axum::response::{IntoResponse, Redirect, Response};
use laterite_admin::chart::{
    stats, Chart, ChartQuery, Format, Kind, Period, Series, Stat, CHART_ASSETS,
};
use laterite_admin::html::Markup;
use laterite_admin::{AuthenticatedUser, FlashLevel, PageHead, SessionHandle, Shell};
use laterite_core::{t, Text};

use super::{escape, failure, Cx};
use crate::registry::SlotReg;
use crate::store::{self, Rate, UsageRow};

/// The currency a slot's prices are in until one is stored.
const DEFAULT_CURRENCY: &str = "USD";

/// The meters a slot counts under: its own, or the one unnamed meter.
fn meters(slot: &SlotReg) -> Vec<(String, Text)> {
    if slot.meters.is_empty() {
        vec![(
            String::new(),
            slot.unit.clone().unwrap_or_else(|| t!("Units")),
        )]
    } else {
        slot.meters.clone()
    }
}

/// What `rows` cost at `rates`. A meter with no price costs nothing.
fn cost(rows: &[UsageRow], rates: &[Rate]) -> f64 {
    rows.iter()
        .map(|row| {
            rates
                .iter()
                .find(|r| r.slot == row.slot && r.meter == row.meter)
                .map_or(0.0, |r| r.price * row.units as f64)
        })
        .sum()
}

/// The last day of `day`'s month.
fn month_end(day: NaiveDate) -> NaiveDate {
    let first = day.with_day(1).unwrap_or(day);
    (first + Duration::days(32)).with_day(1).unwrap_or(first) - Duration::days(1)
}

/// The month's cost at the pace so far: what was spent, over the days gone,
/// times the days in the month.
pub(crate) fn projected(spent: f64, today: NaiveDate) -> f64 {
    spent / f64::from(today.day()) * f64::from(month_end(today).day())
}

pub(super) async fn page(cx: &Cx, shell: Shell, slot: &str, query: ChartQuery) -> Response {
    let Some(slot) = cx.slots.iter().find(|s| s.code == slot) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let db = cx.ctx.db();
    let today = Utc::now().date_naive();
    let period = Period::from_query(&query, today);
    let month_start = today.with_day(1).unwrap_or(today);
    let loaded = async {
        Ok::<_, crate::VaultError>((
            store::usage_between(db, Some(&slot.code), period.from(), period.to()).await?,
            store::usage_between(db, Some(&slot.code), month_start, today).await?,
            store::rates(db).await?,
            store::list(db).await?,
        ))
    };
    let (rows, month, rates, items) = match loaded.await {
        Ok(loaded) => loaded,
        Err(e) => return failure(e),
    };
    let rates: Vec<Rate> = rates.into_iter().filter(|r| r.slot == slot.code).collect();
    let currency = rates
        .first()
        .map_or(DEFAULT_CURRENCY.to_string(), |r| r.currency.clone());
    let money = Format::Money(currency.clone());
    let meters = meters(slot);
    let unit = slot.unit.clone().unwrap_or_else(|| t!("Units"));
    let mut body = String::new();

    // The slots, as tabs.
    if cx.slots.len() > 1 {
        let mut tabs = String::new();
        for other in cx.slots.iter() {
            tabs.push_str(&format!(
                r#"<a class="lat-period__preset{}" href="{}?{}">{}</a>"#,
                if other.code == slot.code { " is-on" } else { "" },
                escape(&cx.ctx.url(&format!("usage/{}", other.code))),
                escape(&period.query()),
                escape(&shell.tt(&other.label)),
            ));
        }
        body.push_str(&format!(
            r#"<div class="lat-period"><div class="lat-period__presets">{tabs}</div></div>"#
        ));
    }
    body.push_str(period.bar(&shell, &query).as_str());

    // The figures.
    let units: i64 = rows.iter().map(|r| r.units).sum();
    let mut figures = vec![Stat::new(unit.clone(), Format::Number.text(units as f64))
        .note(t!("In this period"))];
    if !rates.is_empty() {
        let month_cost = cost(&month, &rates);
        figures.push(
            Stat::new(t!("Estimated cost"), money.text(cost(&rows, &rates)))
                .note(t!("In this period")),
        );
        figures.push(
            Stat::new(t!("This month so far"), money.text(month_cost))
                .note(t!("At the prices below")),
        );
        figures.push(
            Stat::new(
                t!("Projected for the month"),
                money.text(projected(month_cost, today)),
            )
            .note(t!("At the pace so far")),
        );
    }
    body.push_str(stats(&shell, &figures).as_str());

    // Units by meter.
    let dated = |pick: &dyn Fn(&UsageRow) -> Option<f64>| -> Vec<(NaiveDate, f64)> {
        rows.iter()
            .filter_map(|r| pick(r).map(|v| (r.day, v)))
            .collect()
    };
    let known = |meter: &str| meters.iter().any(|(code, _)| code == meter);
    let mut by_meter = Chart::new("units", period.clone()).title(unit.clone());
    for (code, label) in &meters {
        by_meter = by_meter.series(Series::daily(
            if code.is_empty() { "units" } else { code },
            label.clone(),
            &period,
            dated(&|r| (r.meter == *code).then_some(r.units as f64)),
        ));
    }
    if rows.iter().any(|r| !known(&r.meter) && r.units > 0) {
        by_meter = by_meter.series(Series::daily(
            "other",
            t!("Other"),
            &period,
            dated(&|r| (!known(&r.meter)).then_some(r.units as f64)),
        ));
    }
    body.push_str(by_meter.render(&shell, &query).as_str());

    // Cost by meter.
    if !rates.is_empty() {
        let mut costs = Chart::new("cost", period.clone())
            .title(t!("Estimated cost"))
            .kind(Kind::StackedBar)
            .format(money.clone());
        for (code, label) in &meters {
            let price = rates
                .iter()
                .find(|r| r.meter == *code)
                .map_or(0.0, |r| r.price);
            costs = costs.series(Series::daily(
                if code.is_empty() { "units" } else { code },
                label.clone(),
                &period,
                dated(&|r| (r.meter == *code).then_some(r.units as f64 * price)),
            ));
        }
        body.push_str(costs.render(&shell, &query).as_str());
    }

    // Units by credential, for a slot that cycles through several.
    let mut used: BTreeMap<i64, ()> = BTreeMap::new();
    for row in &rows {
        used.insert(row.item_id, ());
    }
    if slot.pooled && used.len() > 1 {
        let mut by_item = Chart::new("credentials", period.clone()).title(t!("By credential"));
        for id in used.keys() {
            let label = items
                .iter()
                .find(|i| i.id == *id)
                .map_or_else(|| format!("#{id}"), |i| i.label.clone());
            by_item = by_item.series(Series::daily(
                format!("c{id}"),
                Text::dynamic(label),
                &period,
                dated(&|r| (r.item_id == *id).then_some(r.units as f64)),
            ));
        }
        body.push_str(by_item.render(&shell, &query).as_str());
    }

    // Calls and errors.
    let calls = Chart::new("calls", period.clone())
        .title(t!("Calls"))
        .kind(Kind::Bar)
        .series(Series::daily(
            "ok",
            t!("Succeeded"),
            &period,
            dated(&|r| Some(r.calls as f64)),
        ))
        .series(Series::daily(
            "failed",
            t!("Failed"),
            &period,
            dated(&|r| Some(r.errors as f64)),
        ));
    body.push_str(calls.render(&shell, &query).as_str());

    // The prices.
    let mut inputs = String::new();
    for (i, (code, label)) in meters.iter().enumerate() {
        let entered = rates
            .iter()
            .find(|r| r.meter == *code)
            .map_or("", |r| r.entered.as_str());
        inputs.push_str(&format!(
            r#"<div class="lat-field" style="grid-column:span 4"><label class="lat-label" for="price_{i}">{}</label><input class="lat-input" id="price_{i}" name="price_{i}" inputmode="decimal" value="{}" placeholder="0.00"></div>"#,
            escape(&shell.tt(label)),
            escape(entered),
        ));
    }
    body.push_str(&format!(
        r#"<form class="lat-card" method="post" action="{action}"><input type="hidden" name="_csrf" value="{csrf}"><h2 class="lat-section-title">{title}</h2><p class="lat-note">{help}</p><div class="lat-form-grid">{inputs}<div class="lat-field" style="grid-column:span 4"><label class="lat-label" for="currency">{currency_label}</label><input class="lat-input" id="currency" name="currency" maxlength="3" value="{currency}"></div></div><div class="lat-form-actions"><button class="lat-btn lat-btn--primary" type="submit">{save}</button></div></form>"#,
        action = escape(&cx.ctx.url(&format!("usage/{}/rates", slot.code))),
        csrf = escape(shell.csrf_token()),
        title = escape(&shell.t("Price of one unit")),
        help = escape(&shell.t(
            "Used for the estimates on this page only. Leave a price blank to stop estimating it."
        )),
        currency_label = escape(&shell.t("Currency code")),
        currency = escape(&currency),
        save = escape(&shell.t("Save prices")),
    ));

    let title = Text::dynamic(format!(
        "{}: {}",
        shell.t("Usage"),
        shell.tt(&slot.label)
    ));
    shell.page_with(
        PageHead::new(title)
            .back(t!("Credentials"), cx.index())
            .assets(CHART_ASSETS),
        Markup::raw_unchecked(body),
    )
}

pub(super) async fn save_rates(
    cx: &Cx,
    user: AuthenticatedUser,
    session: SessionHandle,
    slot: &str,
    data: HashMap<String, String>,
) -> Response {
    let Some(slot) = cx.slots.iter().find(|s| s.code == slot) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let back = cx.ctx.url(&format!("usage/{}", slot.code));
    let currency = data
        .get("currency")
        .map(|c| c.trim().to_ascii_uppercase())
        .filter(|c| c.len() == 3 && c.chars().all(|ch| ch.is_ascii_alphabetic()))
        .unwrap_or_else(|| DEFAULT_CURRENCY.to_string());
    let meters = meters(slot);
    // Read every price before storing any, so a bad one changes nothing.
    let mut prices = Vec::new();
    for (i, (code, _)) in meters.iter().enumerate() {
        let entered = data
            .get(&format!("price_{i}"))
            .map(|p| p.trim())
            .unwrap_or_default();
        if entered.is_empty() {
            prices.push((code, None));
        } else if entered.parse::<f64>().is_ok_and(|p| p.is_finite() && p >= 0.0) {
            prices.push((code, Some(entered)));
        } else {
            session.push_flash(
                FlashLevel::Error,
                t!("A price must be a number, such as 0.0026."),
            );
            return Redirect::to(&back).into_response();
        }
    }
    for (code, price) in prices {
        if let Err(e) = store::set_rate(cx.ctx.db(), &slot.code, code, price, &currency).await {
            return failure(e);
        }
    }
    cx.ctx
        .audit(
            &user,
            "rainmill.vault.rate.update",
            Some("vault_rates"),
            None,
            Some(&slot.code),
            Some(&currency),
        )
        .await;
    session.push_flash(FlashLevel::Success, t!("The prices were saved."));
    Redirect::to(&back).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_month_is_projected_from_its_pace_so_far() {
        let day = |d| NaiveDate::from_ymd_opt(2026, 11, d).unwrap();
        assert_eq!(projected(10.0, day(10)), 30.0);
        assert_eq!(projected(30.0, day(30)), 30.0);
        assert_eq!(month_end(NaiveDate::from_ymd_opt(2026, 2, 3).unwrap()).day(), 28);
    }
}
