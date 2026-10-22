//! Original, deterministic workbooks used by the agentic workspace.

use gridcraft_model::Workbook;
use serde_json::{Value as Json, json};

use crate::review_workflow::{ComparisonPeriods, SaleStatus, compare_sales, demo_sales};
use crate::{Result, Session};

/// Build the default workbook shown by the browser and native app.
pub fn weekly_sales_workbook() -> Result<Workbook> {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("sheet.rename", json!({"name": "Review"}))?;
    session.execute("home.insertSheet", json!({"name": "Orders"}))?;
    session.execute("home.insertSheet", json!({"name": "Sources and Checks"}))?;

    let sales = demo_sales();
    let mut orders = vec![vec![
        json!("Id"),
        json!("Timestamp (UTC)"),
        json!("Category"),
        json!("Amount"),
        json!("Refund"),
        json!("Net"),
        json!("Status"),
        json!("Test"),
    ]];
    for (index, sale) in sales.iter().enumerate() {
        let row = index.saturating_add(2);
        let timestamp = gridcraft_core::date::civil_from_days(sale.timestamp_utc.div_euclid(86_400));
        let (year, month, day) = timestamp;
        let seconds = sale.timestamp_utc.rem_euclid(86_400);
        let hour = seconds / 3_600;
        let minute = seconds / 60 % 60;
        let second = seconds % 60;
        orders.push(vec![
            literal(&sale.id),
            json!(format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")),
            literal(&sale.category),
            json!(sale.amount_minor as f64 / 100.0),
            json!(sale.refunded_minor as f64 / 100.0),
            json!(format!("=D{row}-E{row}")),
            json!(match sale.status {
                SaleStatus::Paid => "paid",
                SaleStatus::Pending => "pending",
                SaleStatus::Cancelled => "cancelled",
            }),
            json!(if sale.is_test { "yes" } else { "no" }),
        ]);
    }
    session.execute("sheet.activate", json!({"sheet": 1}))?;
    session.execute("range.setValues", json!({"range": "A1", "values": orders}))?;
    session.execute("home.numberFormat", json!({"range": "D2:F13", "code": "\"€\"#,##0.00"}))?;
    session.execute("home.columnWidth", json!({"cols": "A:A", "width": 90}))?;
    session.execute("home.columnWidth", json!({"cols": "B:B", "width": 150}))?;
    session.execute("home.columnWidth", json!({"cols": "C:C", "width": 90}))?;
    session.execute("home.columnWidth", json!({"cols": "D:F", "width": 80}))?;
    session.execute("home.columnWidth", json!({"cols": "G:H", "width": 70}))?;

    let review =
        compare_sales(&sales, &[], "weekly-sales-default", ComparisonPeriods::demo()).map_err(|e| crate::EngineError::Other(e.to_string()))?;
    session.execute("sheet.activate", json!({"sheet": 0}))?;
    session.execute("range.setValues", json!({"range": "A1", "values": review.sheet_values_eur()}))?;
    let total_row = review.categories().len().saturating_add(2);
    session.execute("home.numberFormat", json!({"range": format!("B2:D{total_row}"), "code": "\"€\"#,##0.00"}))?;
    session.execute("home.numberFormat", json!({"range": format!("E2:E{total_row}"), "code": "0.0%"}))?;
    session.execute("home.columnWidth", json!({"cols": "A:A", "width": 160}))?;
    session.execute("home.columnWidth", json!({"cols": "B:D", "width": 95}))?;
    session.execute("home.columnWidth", json!({"cols": "E:E", "width": 75}))?;

    session.execute("sheet.activate", json!({"sheet": 2}))?;
    session.execute(
        "range.setValues",
        json!({"range": "A1", "values": [
            ["Field", "Value"],
            ["Periods", "2026-10-05..2026-10-12 and 2026-10-12..2026-10-19 UTC (end excluded)"],
            ["Metric", "Paid-order gross minus refunds, EUR; no currency conversion"],
            ["Rules applied", "none"],
            ["Record counts", "Prior: 4; Current: 4; Source: 12"],
            ["Query id", "weekly-sales-default"],
            ["Check: prior paid net", "=IF(SUM(Orders!F2:F5)=Review!B5,\"PASS\",\"FAIL\")"],
            ["Check: current paid net", "=IF(SUM(Orders!F6:F9)=Review!C5,\"PASS\",\"FAIL\")"],
        ]}),
    )?;
    session.execute("home.columnWidth", json!({"cols": "A:A", "width": 180}))?;
    session.execute("home.columnWidth", json!({"cols": "B:B", "width": 450}))?;
    session.execute("sheet.activate", json!({"sheet": 0}))?;
    Ok((*session.doc()?.wb).clone())
}

/// Serialize the built-in workbook to XLSX bytes.
pub fn weekly_sales_xlsx() -> Result<Vec<u8>> {
    let workbook = weekly_sales_workbook()?;
    crate::io::save_bytes(&workbook, "Weekly Sales.xlsx")
}

fn literal(text: &str) -> Json {
    json!(format!("'{text}"))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::io;
    use gridcraft_core::{CellRef, Value};

    #[test]
    fn weekly_sales_round_trips_with_expected_sheets_and_totals() {
        let bytes = weekly_sales_xlsx().expect("demo workbook");
        let (wb, warnings) = io::open_bytes("Weekly Sales.xlsx", &bytes).expect("round trip");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Review", "Orders", "Sources and Checks"]);
        assert_eq!(wb.sheet(0).expect("review").value(CellRef::new(4, 1)), Value::number(150.0));
        assert_eq!(wb.sheet(0).expect("review").value(CellRef::new(4, 2)), Value::number(655.0));
        assert_eq!(wb.sheet(1).expect("orders").cells.iter().count(), 104);
        assert_eq!(wb.sheet(2).expect("checks").value(CellRef::new(6, 1)), Value::text("PASS"));
        assert_eq!(wb.sheet(2).expect("checks").value(CellRef::new(7, 1)), Value::text("PASS"));
    }
}
