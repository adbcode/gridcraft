//! Offline business-review contracts. No provider, database, network or workbook mutation.
//!
//! The caller applies [`SalesReview::sheet_values`] through `range.setValues` at A1, using
//! the live-session bridge's conflict checks. Amounts stay in integer EUR cents throughout.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

const WEEK_SECONDS: i64 = 604_800;
pub const MAX_RECORDS: usize = 10_000;
pub const MAX_RULES: usize = 100;
pub const MAX_CATEGORIES: usize = 100;
pub const MAX_DRILLDOWN_ROWS: usize = 100;
const MAX_TEXT_BYTES: usize = 256;
const MAX_AMOUNT_MINOR: i64 = 1_000_000_000_000;
// GridCraft's numeric cells use f64. Keep integer monetary totals exactly representable.
const MAX_EXACT_INTEGER: i64 = 9_007_199_254_740_991;

pub const METRIC_DEFINITION: &str =
    "EUR cents: paid-order gross minus recorded refunds, attributed to the original order timestamp; no currency conversion";
pub const DRILLDOWN_LIMITATION: &str = "These records describe the difference; they do not establish why customer demand changed.";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReviewError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("unsupported rule; only 'exclude test orders' is supported")]
    UnsupportedRule,
    #[error("input exceeds the bounded review limits")]
    LimitExceeded,
    #[error("monetary total exceeds exact spreadsheet integer precision")]
    AmountOverflow,
    #[error("continuation does not match the outstanding run/session/tool request")]
    ContinuationMismatch,
    #[error("a tool result with this id was already recorded with different content")]
    ConflictingReplay,
    #[error("the run cannot accept this operation in its current state")]
    InvalidRunState,
}

pub type ReviewResult<T> = std::result::Result<T, ReviewError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Currency {
    #[serde(rename = "EUR")]
    Eur,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaleStatus {
    Paid,
    Pending,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sale {
    pub id: String,
    pub category: String,
    /// Original order timestamp, seconds since Unix epoch in UTC.
    pub timestamp_utc: i64,
    pub amount_minor: i64,
    /// Cumulative refunds at query time, attributed back to the original order period.
    pub refunded_minor: i64,
    pub currency: Currency,
    pub status: SaleStatus,
    pub is_test: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Period {
    pub start_utc: i64,
    pub end_exclusive_utc: i64,
}

impl Period {
    fn contains(self, timestamp: i64) -> bool {
        timestamp >= self.start_utc && timestamp < self.end_exclusive_utc
    }

    fn is_full_utc_week(self) -> bool {
        self.end_exclusive_utc.checked_sub(self.start_utc) == Some(WEEK_SECONDS)
            // Unix epoch was a Thursday; Monday 00:00 UTC is four days later.
            && self.start_utc.rem_euclid(WEEK_SECONDS) == 345_600
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonPeriods {
    pub previous: Period,
    pub current: Period,
}

impl ComparisonPeriods {
    /// 2026-10-05 00:00 UTC..2026-10-12 and 2026-10-12..2026-10-19, end-exclusive.
    pub fn demo() -> Self {
        Self {
            previous: Period { start_utc: 1_791_158_400, end_exclusive_utc: 1_791_763_200 },
            current: Period { start_utc: 1_791_763_200, end_exclusive_utc: 1_792_368_000 },
        }
    }

    fn validate(self) -> ReviewResult<()> {
        if !self.previous.is_full_utc_week() || !self.current.is_full_utc_week() || self.previous.end_exclusive_utc != self.current.start_utc {
            return Err(ReviewError::InvalidInput("comparison requires consecutive full Monday-to-Monday UTC weeks".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    ExcludeTestOrders,
}

/// Construct with [`ReviewRule::from_instruction`] so unsupported wording cannot silently
/// become an ineffective saved rule. The persistence adapter must reconstruct through it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReviewRule {
    id: String,
    original_wording: String,
    kind: RuleKind,
    enabled: bool,
}

impl ReviewRule {
    pub fn from_instruction(id: &str, wording: &str) -> ReviewResult<Self> {
        validate_text(id, "rule id")?;
        validate_text(wording, "rule wording")?;
        let normalized = wording.trim().trim_end_matches('.').to_ascii_lowercase();
        if !matches!(normalized.as_str(), "exclude test orders" | "remember to exclude test orders") {
            return Err(ReviewError::UnsupportedRule);
        }
        Ok(Self { id: id.into(), original_wording: wording.into(), kind: RuleKind::ExcludeTestOrders, enabled: true })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn original_wording(&self) -> &str {
        &self.original_wording
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

/// Bounded retrieval for run context and queries. Duplicate identities are rejected even
/// when one copy is disabled; storage should enforce the same rule.
pub fn enabled_rules(rules: &[ReviewRule]) -> ReviewResult<Vec<&ReviewRule>> {
    if rules.len() > MAX_RULES {
        return Err(ReviewError::LimitExceeded);
    }
    let mut ids = BTreeSet::new();
    for rule in rules {
        if !ids.insert(rule.id.as_str()) {
            return Err(ReviewError::InvalidInput("duplicate rule id".into()));
        }
    }
    let mut enabled: Vec<_> = rules.iter().filter(|rule| rule.enabled).collect();
    enabled.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(enabled)
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CategoryComparison {
    pub category: String,
    pub previous_minor: i64,
    pub current_minor: i64,
    pub delta_minor: i64,
    /// Fractional growth, e.g. -0.4 for -40%; None when the previous period is zero.
    pub growth: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QueryProvenance {
    pub query_id: String,
    pub periods: ComparisonPeriods,
    pub metric_definition: String,
    pub applied_rule_ids: Vec<String>,
    pub previous_record_count: usize,
    pub current_record_count: usize,
    /// Number of supplied source rows, including rows excluded by time/status/rules.
    pub source_record_count: usize,
    pub currency: Currency,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SalesReview {
    categories: Vec<CategoryComparison>,
    total: CategoryComparison,
    provenance: QueryProvenance,
}

impl SalesReview {
    pub fn categories(&self) -> &[CategoryComparison] {
        &self.categories
    }

    pub fn total(&self) -> &CategoryComparison {
        &self.total
    }

    pub fn provenance(&self) -> &QueryProvenance {
        &self.provenance
    }

    /// Five-column rectangular command payload, anchored at A1. Monetary cells are integer
    /// EUR cents, and generated formulas recalculate if a user edits the summary values.
    /// All external text is forced to literal cell text, including formula-like labels.
    pub fn sheet_values(&self) -> Vec<Vec<Json>> {
        let mut rows = vec![vec![json!("Group"), json!("Prior"), json!("Current"), json!("Delta"), json!("Growth")]];
        for (index, category) in self.categories.iter().enumerate() {
            let row = index.saturating_add(2);
            rows.push(vec![
                literal(&category.category),
                json!(category.previous_minor),
                json!(category.current_minor),
                json!(format!("=C{row}-B{row}")),
                json!(format!("=IF(B{row}=0,\"n/a\",D{row}/B{row})")),
            ]);
        }
        let total_row = rows.len().saturating_add(1);
        let last_category_row = total_row.saturating_sub(1);
        let previous = if self.categories.is_empty() { json!(0) } else { json!(format!("=SUM(B2:B{last_category_row})")) };
        let current = if self.categories.is_empty() { json!(0) } else { json!(format!("=SUM(C2:C{last_category_row})")) };
        rows.push(vec![
            json!("Total"),
            previous,
            current,
            json!(format!("=C{total_row}-B{total_row}")),
            json!(format!("=IF(B{total_row}=0,\"n/a\",D{total_row}/B{total_row})")),
        ]);
        rows.push(vec![Json::Null; 5]);
        let p = &self.provenance;
        let fields = [
            ("Query", p.query_id.clone()),
            ("Metric", p.metric_definition.clone()),
            ("Prior", period_label(p.periods.previous)),
            ("Current", period_label(p.periods.current)),
            ("Rules", if p.applied_rule_ids.is_empty() { "none".into() } else { p.applied_rule_ids.join(", ") }),
            ("Rows", format!("prior: {}; current: {}; source: {}", p.previous_record_count, p.current_record_count, p.source_record_count)),
            ("Zero", "Growth is fractional; prior zero = n/a, including zero-to-zero".into()),
            ("Detail", DRILLDOWN_LIMITATION.into()),
        ];
        for (label, value) in fields {
            rows.push(vec![json!(label), literal(&value), Json::Null, Json::Null, Json::Null]);
        }
        rows
    }

    /// The review payload expressed in EUR rather than the engine's integer cents.
    ///
    /// This is used by the built-in workbook so that the workbook's visible
    /// number formats describe the values users edit.
    pub fn sheet_values_eur(&self) -> Vec<Vec<Json>> {
        let mut rows = vec![vec![json!("Group"), json!("Prior (EUR)"), json!("Current (EUR)"), json!("Delta (EUR)"), json!("Growth")]];
        for (index, category) in self.categories.iter().enumerate() {
            let row = index.saturating_add(2);
            rows.push(vec![
                literal(&category.category),
                json!(category.previous_minor as f64 / 100.0),
                json!(category.current_minor as f64 / 100.0),
                json!(format!("=C{row}-B{row}")),
                json!(format!("=IF(B{row}=0,\"n/a\",D{row}/B{row})")),
            ]);
        }
        let total_row = rows.len().saturating_add(1);
        let last_category_row = total_row.saturating_sub(1);
        let previous = if self.categories.is_empty() { json!(0.0) } else { json!(format!("=SUM(B2:B{last_category_row})")) };
        let current = if self.categories.is_empty() { json!(0.0) } else { json!(format!("=SUM(C2:C{last_category_row})")) };
        rows.push(vec![
            json!("Total"),
            previous,
            current,
            json!(format!("=C{total_row}-B{total_row}")),
            json!(format!("=IF(B{total_row}=0,\"n/a\",D{total_row}/B{total_row})")),
        ]);
        rows.push(vec![Json::Null; 5]);
        let p = &self.provenance;
        let fields = [
            ("Query", p.query_id.clone()),
            ("Metric", p.metric_definition.clone()),
            ("Prior", period_label(p.periods.previous)),
            ("Current", period_label(p.periods.current)),
            ("Rules", if p.applied_rule_ids.is_empty() { "none".into() } else { p.applied_rule_ids.join(", ") }),
            ("Rows", format!("prior: {}; current: {}; source: {}", p.previous_record_count, p.current_record_count, p.source_record_count)),
            ("Zero", "Growth is fractional; prior zero = n/a, including zero-to-zero".into()),
            ("Detail", DRILLDOWN_LIMITATION.into()),
        ];
        for (label, value) in fields {
            rows.push(vec![json!(label), literal(&value), Json::Null, Json::Null, Json::Null]);
        }
        rows
    }
}

fn period_label(period: Period) -> String {
    format!("{} .. {} UTC (end excluded)", utc_day(period.start_utc), utc_day(period.end_exclusive_utc))
}

fn utc_day(timestamp: i64) -> String {
    let day = timestamp.div_euclid(86_400);
    let first = gridcraft_core::date::days_from_civil(0, 1, 1);
    let last = gridcraft_core::date::days_from_civil(9999, 12, 31);
    if !(first..=last).contains(&day) {
        return format!("{timestamp} Unix seconds");
    }
    let (year, month, day) = gridcraft_core::date::civil_from_days(day);
    format!("{year:04}-{month:02}-{day:02}")
}

fn literal(text: &str) -> Json {
    json!(format!("'{text}"))
}

fn validate_text(text: &str, field: &str) -> ReviewResult<()> {
    if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES || text.chars().any(char::is_control) {
        return Err(ReviewError::InvalidInput(format!("invalid {field}")));
    }
    Ok(())
}

fn validate_sales(records: &[Sale]) -> ReviewResult<()> {
    if records.len() > MAX_RECORDS {
        return Err(ReviewError::LimitExceeded);
    }
    let mut ids = BTreeSet::new();
    for record in records {
        validate_text(&record.id, "sale id")?;
        validate_text(&record.category, "category")?;
        if !ids.insert(record.id.as_str()) {
            return Err(ReviewError::InvalidInput("duplicate sale id".into()));
        }
        if !(0..=MAX_AMOUNT_MINOR).contains(&record.amount_minor) || !(0..=record.amount_minor).contains(&record.refunded_minor) {
            return Err(ReviewError::InvalidInput("amounts must be nonnegative EUR cents; refunds cannot exceed gross sales".into()));
        }
    }
    Ok(())
}

fn qualifies(record: &Sale, exclude_tests: bool) -> bool {
    record.status == SaleStatus::Paid && !(exclude_tests && record.is_test)
}

fn add_amount(total: i64, amount: i64) -> ReviewResult<i64> {
    let value = total.checked_add(amount).ok_or(ReviewError::AmountOverflow)?;
    if value > MAX_EXACT_INTEGER {
        return Err(ReviewError::AmountOverflow);
    }
    Ok(value)
}

fn comparison(category: String, previous: i64, current: i64) -> CategoryComparison {
    // Both totals are nonnegative and bounded below i64::MAX, so subtraction is safe.
    let delta = current.saturating_sub(previous);
    CategoryComparison {
        category,
        previous_minor: previous,
        current_minor: current,
        delta_minor: delta,
        growth: (previous != 0).then(|| delta as f64 / previous as f64),
    }
}

pub fn compare_sales(records: &[Sale], rules: &[ReviewRule], query_id: &str, periods: ComparisonPeriods) -> ReviewResult<SalesReview> {
    validate_text(query_id, "query id")?;
    periods.validate()?;
    validate_sales(records)?;
    let enabled = enabled_rules(rules)?;
    let exclude_tests = enabled.iter().any(|rule| rule.kind == RuleKind::ExcludeTestOrders);
    let mut totals: BTreeMap<&str, (i64, i64)> = BTreeMap::new();
    let mut previous_total = 0;
    let mut current_total = 0;
    let mut previous_count = 0;
    let mut current_count = 0;
    for record in records {
        if !qualifies(record, exclude_tests) {
            continue;
        }
        let previous = periods.previous.contains(record.timestamp_utc);
        if !previous && !periods.current.contains(record.timestamp_utc) {
            continue;
        }
        if !totals.contains_key(record.category.as_str()) && totals.len() >= MAX_CATEGORIES {
            return Err(ReviewError::LimitExceeded);
        }
        let entry = totals.entry(record.category.as_str()).or_default();
        let net = record.amount_minor.saturating_sub(record.refunded_minor);
        if previous {
            entry.0 = add_amount(entry.0, net)?;
            previous_total = add_amount(previous_total, net)?;
            previous_count += 1;
        } else {
            entry.1 = add_amount(entry.1, net)?;
            current_total = add_amount(current_total, net)?;
            current_count += 1;
        }
    }
    Ok(SalesReview {
        categories: totals.into_iter().map(|(category, (previous, current))| comparison(category.into(), previous, current)).collect(),
        total: comparison("Total".into(), previous_total, current_total),
        provenance: QueryProvenance {
            query_id: query_id.into(),
            periods,
            metric_definition: METRIC_DEFINITION.into(),
            applied_rule_ids: enabled.iter().map(|rule| rule.id.clone()).collect(),
            previous_record_count: previous_count,
            current_record_count: current_count,
            source_record_count: records.len(),
            currency: Currency::Eur,
        },
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DrillDown {
    pub query_id: String,
    pub periods: ComparisonPeriods,
    pub applied_rule_ids: Vec<String>,
    pub category: String,
    pub rows: Vec<Sale>,
    pub total_matching: usize,
    pub truncated: bool,
    pub limitation: String,
}

/// The same filters as the summary. Rows are ordered by timestamp and id, never presented
/// as causal evidence. An upstream service must query all relevant records before aggregation.
pub fn drill_down(
    records: &[Sale],
    rules: &[ReviewRule],
    query_id: &str,
    periods: ComparisonPeriods,
    category: &str,
    limit: usize,
) -> ReviewResult<DrillDown> {
    validate_text(category, "category")?;
    if limit == 0 || limit > MAX_DRILLDOWN_ROWS {
        return Err(ReviewError::LimitExceeded);
    }
    let summary = compare_sales(records, rules, query_id, periods)?;
    let exclude_tests = !summary.provenance.applied_rule_ids.is_empty();
    let mut matching: Vec<_> = records
        .iter()
        .filter(|r| {
            r.category == category
                && qualifies(r, exclude_tests)
                && (periods.previous.contains(r.timestamp_utc) || periods.current.contains(r.timestamp_utc))
        })
        .collect();
    matching.sort_by(|a, b| a.timestamp_utc.cmp(&b.timestamp_utc).then_with(|| a.id.cmp(&b.id)));
    let total_matching = matching.len();
    Ok(DrillDown {
        query_id: query_id.into(),
        periods,
        applied_rule_ids: summary.provenance.applied_rule_ids,
        category: category.into(),
        rows: matching.into_iter().take(limit).cloned().collect(),
        total_matching,
        truncated: total_matching > limit,
        limitation: DRILLDOWN_LIMITATION.into(),
    })
}

/// Original synthetic records only; no vendor workbook or customer data.
pub fn demo_sales() -> Vec<Sale> {
    let p = ComparisonPeriods::demo();
    [
        ("p-jackets-a", "Jackets", p.previous.start_utc, 6_000, 0, SaleStatus::Paid, false),
        ("p-jackets-b", "Jackets", p.previous.start_utc + 86_400, 5_000, 1_000, SaleStatus::Paid, false),
        ("p-shoes", "Shoes", p.previous.start_utc + 172_800, 5_000, 0, SaleStatus::Paid, false),
        ("p-hats-refunded", "Hats", p.previous.start_utc + 259_200, 1_000, 1_000, SaleStatus::Paid, false),
        ("c-jackets", "Jackets", p.current.start_utc, 7_000, 1_000, SaleStatus::Paid, false),
        ("c-shoes", "Shoes", p.current.start_utc + 86_400, 7_500, 0, SaleStatus::Paid, false),
        ("c-hats", "Hats", p.current.start_utc + 172_800, 2_000, 0, SaleStatus::Paid, false),
        ("c-test", "Jackets", p.current.start_utc + 259_200, 50_000, 0, SaleStatus::Paid, true),
        ("c-pending", "Jackets", p.current.start_utc + 345_600, 100_000, 0, SaleStatus::Pending, false),
        ("p-cancelled", "Shoes", p.previous.start_utc + 345_600, 9_000, 0, SaleStatus::Cancelled, false),
        ("before-period", "Jackets", p.previous.start_utc - 1, 111, 0, SaleStatus::Paid, false),
        ("after-period", "Jackets", p.current.end_exclusive_utc, 222, 0, SaleStatus::Paid, false),
    ]
    .into_iter()
    .map(|(id, category, timestamp_utc, amount_minor, refunded_minor, status, is_test)| Sale {
        id: id.into(),
        category: category.into(),
        timestamp_utc,
        amount_minor,
        refunded_minor,
        currency: Currency::Eur,
        status,
        is_test,
    })
    .collect()
}

pub fn demo_rules() -> ReviewResult<Vec<ReviewRule>> {
    Ok(vec![ReviewRule::from_instruction("exclude-tests-v1", "Remember to exclude test orders.")?])
}

/// Pure in-memory resume contract, to be persisted transactionally by a future backend.
/// A tool result is evidence supplied by the caller, not proof of a workbook mutation. The
/// backend must verify that caller through its authenticated live-workbook bridge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolReceipt {
    pub run_id: String,
    pub workbook_session_id: String,
    pub tool_use_id: String,
    pub result: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Thinking,
    AwaitingTool,
    ProviderTimeout,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunState {
    run_id: String,
    workbook_session_id: String,
    outstanding_tool_use_id: Option<String>,
    completed: Vec<ToolReceipt>,
    status: RunStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContinuationDisposition {
    Accepted,
    Replay,
}

impl RunState {
    pub fn new(run_id: &str, workbook_session_id: &str) -> ReviewResult<Self> {
        validate_text(run_id, "run id")?;
        validate_text(workbook_session_id, "workbook session id")?;
        Ok(Self {
            run_id: run_id.into(),
            workbook_session_id: workbook_session_id.into(),
            outstanding_tool_use_id: None,
            completed: vec![],
            status: RunStatus::Thinking,
        })
    }

    pub fn request_tool(&mut self, tool_use_id: &str) -> ReviewResult<()> {
        validate_text(tool_use_id, "tool use id")?;
        if self.status != RunStatus::Thinking || self.outstanding_tool_use_id.is_some() || self.completed.iter().any(|r| r.tool_use_id == tool_use_id)
        {
            return Err(ReviewError::InvalidRunState);
        }
        if self.completed.len() >= 100 {
            return Err(ReviewError::LimitExceeded);
        }
        self.outstanding_tool_use_id = Some(tool_use_id.into());
        self.status = RunStatus::AwaitingTool;
        Ok(())
    }

    /// A matching duplicate is acknowledged as replay and changes no state. The caller
    /// returns the stored result and MUST NOT ask the browser to execute it again.
    pub fn continue_tool(&mut self, receipt: ToolReceipt) -> ReviewResult<ContinuationDisposition> {
        if receipt.run_id != self.run_id || receipt.workbook_session_id != self.workbook_session_id {
            return Err(ReviewError::ContinuationMismatch);
        }
        if let Some(previous) = self.completed.iter().find(|r| r.tool_use_id == receipt.tool_use_id) {
            return if previous == &receipt { Ok(ContinuationDisposition::Replay) } else { Err(ReviewError::ConflictingReplay) };
        }
        if self.status != RunStatus::AwaitingTool || self.outstanding_tool_use_id.as_deref() != Some(receipt.tool_use_id.as_str()) {
            return Err(ReviewError::ContinuationMismatch);
        }
        if receipt.result.len() > 64 * 1024 {
            return Err(ReviewError::LimitExceeded);
        }
        self.completed.push(receipt);
        self.outstanding_tool_use_id = None;
        self.status = RunStatus::Thinking;
        Ok(ContinuationDisposition::Accepted)
    }

    /// Provider failure is a separate outcome from already completed browser actions.
    pub fn provider_timeout(&mut self) -> ReviewResult<()> {
        if self.status != RunStatus::Thinking {
            return Err(ReviewError::InvalidRunState);
        }
        self.status = RunStatus::ProviderTimeout;
        Ok(())
    }

    pub fn resume_after_timeout(&mut self) -> ReviewResult<()> {
        if self.status != RunStatus::ProviderTimeout {
            return Err(ReviewError::InvalidRunState);
        }
        self.status = RunStatus::Thinking;
        Ok(())
    }

    pub fn complete(&mut self) -> ReviewResult<()> {
        if self.status != RunStatus::Thinking {
            return Err(ReviewError::InvalidRunState);
        }
        self.status = RunStatus::Complete;
        Ok(())
    }

    pub fn status(&self) -> &RunStatus {
        &self.status
    }

    pub fn completed(&self) -> &[ToolReceipt] {
        &self.completed
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn review() -> SalesReview {
        compare_sales(&demo_sales(), &demo_rules().unwrap(), "fixture-query-v1", ComparisonPeriods::demo()).unwrap()
    }

    #[test]
    fn fixture_matches_exact_documented_answers_and_provenance() {
        let review = review();
        assert_eq!(
            review.categories(),
            &[comparison("Hats".into(), 0, 2_000), comparison("Jackets".into(), 10_000, 6_000), comparison("Shoes".into(), 5_000, 7_500),]
        );
        assert_eq!(review.total(), &comparison("Total".into(), 15_000, 15_500));
        assert_eq!(review.provenance.previous_record_count, 4);
        assert_eq!(review.provenance.current_record_count, 3);
        assert_eq!(review.provenance.source_record_count, 12);
        assert_eq!(review.provenance.applied_rule_ids, vec!["exclude-tests-v1"]);
    }

    #[test]
    fn saved_rule_is_retrieved_and_changes_the_query() {
        let mut rules = demo_rules().unwrap();
        assert_eq!(enabled_rules(&rules).unwrap().first().unwrap().original_wording(), "Remember to exclude test orders.");
        rules.first_mut().unwrap().set_enabled(false);
        assert!(enabled_rules(&rules).unwrap().is_empty());
        let with_tests = compare_sales(&demo_sales(), &rules, "without-rule", ComparisonPeriods::demo()).unwrap();
        assert_eq!(with_tests.total.current_minor, 65_500);
        assert_eq!(with_tests.provenance.current_record_count, 4);
        assert!(with_tests.provenance.applied_rule_ids.is_empty());
        assert_eq!(ReviewRule::from_instruction("other", "Always blame weather"), Err(ReviewError::UnsupportedRule));
    }

    #[test]
    fn period_boundaries_and_zero_baselines_are_explicit() {
        let r = review();
        assert_eq!(r.categories.first().unwrap().growth, None);
        let empty = compare_sales(&[], &[], "empty", ComparisonPeriods::demo()).unwrap();
        assert_eq!(empty.total.growth, None);
        let mut periods = ComparisonPeriods::demo();
        periods.current.end_exclusive_utc -= 1;
        assert!(compare_sales(&demo_sales(), &[], "partial-week", periods).is_err());
        periods.previous.start_utc = i64::MIN;
        periods.previous.end_exclusive_utc = i64::MAX;
        assert!(compare_sales(&[], &[], "hostile-period", periods).is_err());
    }

    #[test]
    fn drilldown_uses_summary_filters_and_disclaims_causality() {
        let detail = drill_down(&demo_sales(), &demo_rules().unwrap(), "jackets-detail", ComparisonPeriods::demo(), "Jackets", 2).unwrap();
        assert_eq!(detail.total_matching, 3);
        assert_eq!(detail.rows.len(), 2);
        assert!(detail.truncated);
        assert!(detail.rows.iter().all(|r| !r.is_test && r.status == SaleStatus::Paid));
        assert_eq!(detail.applied_rule_ids, vec!["exclude-tests-v1"]);
        assert_eq!(detail.limitation, DRILLDOWN_LIMITATION);
        assert!(drill_down(&[], &[], "q", ComparisonPeriods::demo(), "Jackets", usize::MAX).is_err());
    }

    #[test]
    fn rejects_invalid_money_duplicate_records_and_oversized_queries() {
        let mut records = demo_sales();
        records.first_mut().unwrap().refunded_minor = i64::MAX;
        assert!(compare_sales(&records, &[], "bad-refund", ComparisonPeriods::demo()).is_err());
        let sale = demo_sales().first().unwrap().clone();
        assert!(compare_sales(&[sale.clone(), sale.clone()], &[], "duplicate", ComparisonPeriods::demo()).is_err());
        assert_eq!(compare_sales(&vec![sale; MAX_RECORDS + 1], &[], "huge", ComparisonPeriods::demo()), Err(ReviewError::LimitExceeded));
        let rule = demo_rules().unwrap().remove(0);
        assert!(enabled_rules(&[rule.clone(), rule]).is_err());
        let mut large = Vec::new();
        for index in 0..9_008 {
            large.push(Sale {
                id: format!("large-{index}"),
                category: "One".into(),
                timestamp_utc: ComparisonPeriods::demo().previous.start_utc,
                amount_minor: MAX_AMOUNT_MINOR,
                refunded_minor: 0,
                currency: Currency::Eur,
                status: SaleStatus::Paid,
                is_test: false,
            });
        }
        assert_eq!(compare_sales(&large, &[], "precision", ComparisonPeriods::demo()), Err(ReviewError::AmountOverflow));
    }

    #[test]
    fn sheet_applies_calculates_and_undoes_as_one_native_operation() {
        let mut session = crate::Session::new();
        session.add_document(crate::DocState::new(gridcraft_model::Workbook::new(), None, "Review".into()));
        let rows = review().sheet_values();
        assert!(rows.iter().all(|r| r.len() == 5));
        session.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
        let sheet = session.doc().unwrap().wb.sheet(0).unwrap();
        // Alphabetical category order: Hats row 2, Jackets row 3, Shoes row 4, Total row 5.
        assert_eq!(sheet.value(gridcraft_core::CellRef::new(2, 3)), gridcraft_core::Value::number(-4_000.0));
        assert_eq!(sheet.value(gridcraft_core::CellRef::new(2, 4)), gridcraft_core::Value::number(-0.4));
        assert_eq!(sheet.value(gridcraft_core::CellRef::new(1, 4)), gridcraft_core::Value::text("n/a"));
        assert_eq!(sheet.value(gridcraft_core::CellRef::new(4, 3)), gridcraft_core::Value::number(500.0));
        assert_eq!(session.doc().unwrap().undo.len(), 1);
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(session.doc().unwrap().wb.sheet(0).unwrap().value(gridcraft_core::CellRef::new(0, 0)), gridcraft_core::Value::Empty);
    }

    #[test]
    fn external_labels_are_literal_even_if_they_look_like_formulas() {
        let mut records = demo_sales();
        records.first_mut().unwrap().category = "=1+1".into();
        let review = compare_sales(&records, &[], "=2+2", ComparisonPeriods::demo()).unwrap();
        let mut session = crate::Session::new();
        session.add_document(crate::DocState::new(gridcraft_model::Workbook::new(), None, "Review".into()));
        session.execute("range.setValues", json!({"range": "A1", "values": review.sheet_values()})).unwrap();
        assert_eq!(session.doc().unwrap().wb.sheet(0).unwrap().value(gridcraft_core::CellRef::new(1, 0)), gridcraft_core::Value::text("=1+1"));
    }

    #[test]
    fn resumable_contract_replays_without_reapplying_and_retains_timeout_actions() {
        let mut run = RunState::new("run-1", "session-1").unwrap();
        run.request_tool("tool-1").unwrap();
        let receipt = ToolReceipt {
            run_id: "run-1".into(),
            workbook_session_id: "session-1".into(),
            tool_use_id: "tool-1".into(),
            result: "verified D3=-4000".into(),
        };
        let mut wrong = receipt.clone();
        wrong.workbook_session_id = "session-2".into();
        assert_eq!(run.continue_tool(wrong), Err(ReviewError::ContinuationMismatch));
        assert_eq!(run.continue_tool(receipt.clone()), Ok(ContinuationDisposition::Accepted));
        assert_eq!(run.continue_tool(receipt.clone()), Ok(ContinuationDisposition::Replay));
        let mut changed = receipt.clone();
        changed.result = "different".into();
        assert_eq!(run.continue_tool(changed), Err(ReviewError::ConflictingReplay));
        run.provider_timeout().unwrap();
        assert_eq!(run.status(), &RunStatus::ProviderTimeout);
        assert_eq!(run.completed(), std::slice::from_ref(&receipt));
        assert_eq!(run.continue_tool(receipt), Ok(ContinuationDisposition::Replay));
        assert_eq!(run.status(), &RunStatus::ProviderTimeout);
        run.resume_after_timeout().unwrap();
        run.request_tool("tool-2").unwrap();
        assert_eq!(run.complete(), Err(ReviewError::InvalidRunState));
        assert_eq!(run.provider_timeout(), Err(ReviewError::InvalidRunState));
    }
}
