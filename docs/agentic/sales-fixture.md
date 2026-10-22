# Deterministic sales review fixture

Updated: 2026-10-22. Status: pure Rust fixture and query contract; PostgreSQL storage is implemented in the server baseline.

The fixture in `crates/engine/src/review_workflow.rs` contains original synthetic order
records. It needs no credentials, network service or vendor-authored workbook. The query
and its generated rectangular sheet are a preparation for the product described in
the implementation plan. The fixture alone does not demonstrate a live model or Aiven integration.

## Definitions

The comparison uses two complete calendar weeks, both starting Monday at 00:00 UTC:

| Period | Inclusive start | Exclusive end | Unix seconds |
|---|---|---|---|
| Previous | 2026-10-05 | 2026-10-12 | 1791158400–1791763200 |
| Current | 2026-10-12 | 2026-10-19 | 1791763200–1792368000 |

Revenue is **paid-order gross amount minus recorded refunds**, in integer EUR cents.
The complete refund amount known at query time is attributed to the original order's
timestamp. This is an order-cohort measure, not a cash-flow measure; a later refund can
change a historical comparison. Pending and cancelled orders never count. Only EUR is
supported; unknown currencies fail deserialization instead of undergoing conversion.

Growth is `(current - previous) / previous`, expressed as a fraction. When the previous
amount is zero, growth is undefined (`None` in the result and `n/a` in the spreadsheet),
including a zero-to-zero comparison. Partial weeks, overlapping weeks and arbitrary
rolling periods are rejected by this initial contract.

The explicitly saved rule has id `exclude-tests-v1`, original wording **“Remember to
exclude test orders.”**, structured kind `exclude_test_orders`, and `enabled = true`.
Enabled rules are retrieved in stable id order and used in the query filter. Disabling
the rule changes the query. The constructor recognizes only “exclude test orders” and
“remember to exclude test orders”, ignoring case, surrounding whitespace and a final
period. Other instructions produce an unsupported-rule error and are not saved as
ineffective memory. A future agent may propose a supported instruction for user review;
this fixture does not claim general natural-language rule extraction.

## Source records and expected results

All timestamps below are UTC. `amount` and `refund` are integer EUR cents.

| Id | Timestamp | Category | Amount | Refund | Status | Test |
|---|---|---|---:|---:|---|---|
| p-jackets-a | 2026-10-05 00:00 | Jackets | 6000 | 0 | paid | no |
| p-jackets-b | 2026-10-06 00:00 | Jackets | 5000 | 1000 | paid | no |
| p-shoes | 2026-10-07 00:00 | Shoes | 5000 | 0 | paid | no |
| p-hats-refunded | 2026-10-08 00:00 | Hats | 1000 | 1000 | paid | no |
| c-jackets | 2026-10-12 00:00 | Jackets | 7000 | 1000 | paid | no |
| c-shoes | 2026-10-13 00:00 | Shoes | 7500 | 0 | paid | no |
| c-hats | 2026-10-14 00:00 | Hats | 2000 | 0 | paid | no |
| c-test | 2026-10-15 00:00 | Jackets | 50000 | 0 | paid | yes |
| c-pending | 2026-10-16 00:00 | Jackets | 100000 | 0 | pending | no |
| p-cancelled | 2026-10-09 00:00 | Shoes | 9000 | 0 | cancelled | no |
| before-period | 2026-10-04 23:59:59 | Jackets | 111 | 0 | paid | no |
| after-period | 2026-10-19 00:00 | Jackets | 222 | 0 | paid | no |

With the exclusion rule enabled:

| Category | Previous EUR | Current EUR | Delta EUR | Growth |
|---|---:|---:|---:|---:|
| Hats | 0.00 | 20.00 | 20.00 | undefined |
| Jackets | 100.00 | 60.00 | -40.00 | -40% |
| Shoes | 50.00 | 75.00 | 25.00 | 50% |
| Total | 150.00 | 155.00 | 5.00 | 3⅓% |

Qualifying record counts are **4 previous / 3 current**, from **12 supplied records**.
The fully refunded Hats order still counts as a qualifying record with zero net revenue.
Disabling the exclusion rule yields current revenue of **65500 cents (€655.00)** across
four current records. Boundary records test inclusive starts and exclusive ends.

Every summary carries a query id, both period boundaries, the metric definition,
currency, applied rule ids, qualifying counts for each week and total source count.
Category detail uses the same filters, reports its total matching count, sorts by
timestamp then id, and explicitly indicates truncation. Jackets has three matching
records. This demonstrates which recorded sales/refunds make up the change, and does
not assert a causal explanation of demand. The returned detail includes that limitation.

## Workbook and resume contracts

`SalesReview::sheet_values()` produces five columns, anchored at **A1**, for the existing
`range.setValues` command. Compact headers (`Group`, `Prior`, `Current`, `Delta`, `Growth`)
fit the standard column widths. Monetary values remain integer EUR cents, stated at the
start of the metric provenance; growth is fractional. The prior/current provenance rows
show readable UTC dates and explicitly exclude the end date. Delta, growth and total cells
are live formulas. External category labels and provenance
text are forced to literal strings so a category such as `=1+1` cannot inject a formula.
The rows include provenance beneath the editable table. A single command creates one
native undo entry; this foundation does not promise multi-command agent batch undo.

The query validates up to 10000 records, 100 categories and 100 rules. Drill-down accepts
1–100 returned rows. Duplicate source/rule identifiers, negative values, refunds larger
than gross sales, overlong text and unsafe integer totals are errors. A record amount is
capped at 10^12 cents and all aggregates at 2^53−1, the exact integer range of spreadsheet
numeric values. An upstream adapter must not truncate source data to meet those bounds:
it must reject it or implement a complete, equivalently validated database aggregate.

`RunState` is an in-memory, single-outstanding-tool state machine. It checks run,
workbook-session and tool-use identities, accepts exact receipt retries as replay,
rejects a different result under the same tool-use id and retains completed receipts
after a provider timeout. Resume continues from that evidence; it does not promise that
the workbook was unchanged. It accepts at most 100 tool completions, with each result
capped at 64 KiB. The caller must verify browser action results and persist every
transition transactionally. There is no HTTP endpoint, provider call, durable recovery,
authentication, `/start` idempotency store or PostgreSQL connection in this module.

The [server migrations](../../apps/gridcraft-server/migrations/) define implemented
PostgreSQL storage for sales, explicit rules, immutable query evidence and workbook snapshots.
Server comparisons reuse this fixture's pure query semantics; each stored answer includes
the exact source rows, ingestion labels and rule context used at its source revision.
Bounded imports change subsequent answers without rewriting earlier query evidence.
Database snapshot versions remain distinct from browser change tokens. Browser upload/ack
coordination and durable run/continuation storage remain future integration work; later
browser edits must remain unsaved when an earlier snapshot is acknowledged. See
[service API](service-api.md) and [validation](validation.md) for actual scope and checks.

## Focused verification

Run `cargo test -p gridcraft-engine review_workflow` after normal workspace dependencies
are available. The tests cover exact fixture answers, enabled-rule retrieval and effect,
period boundaries, undefined growth, drill-down limits, invalid monetary inputs,
duplicate identities, integer precision limits, formula-label injection, native
calculation/undo, continuation matching, replay conflicts and provider-timeout receipts.

Revision history: 2026-10-21 — initial offline fixture, worksheet output, resume contract
and unexecuted PostgreSQL schema. 2026-10-22 — replace the obsolete schema draft with the
implemented server storage boundary and immutable evidence contract.
