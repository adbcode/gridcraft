//! Shared, provider-independent workspace contracts and original synthetic workbooks.
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::{
    Session,
    agent_bridge::Identity,
    review_workflow::{ComparisonPeriods, SalesReview, compare_sales, demo_sales},
};

pub const WEEKLY_ID: &str = "6cd8012b-4bf0-4ce3-8da0-000000000001";
pub const FORECAST_ID: &str = "6cd8012b-4bf0-4ce3-8da0-000000000002";
pub const INVENTORY_ID: &str = "6cd8012b-4bf0-4ce3-8da0-000000000003";
pub const SALES_DATASET: &str = "workspace_sales_records";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentVersion {
    pub workbook_id: String,
    pub version: i64,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRun {
    pub run_id: String,
    pub workbook_session_id: String,
    pub prompt: String,
    pub context: Json,
    pub attachments: Vec<AttachmentVersion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDispatch {
    pub tool_call_id: String,
    pub method: String,
    pub params: Json,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolContinuation {
    pub workbook_session_id: String,
    pub tool_call_id: String,
    pub result: Json,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunView {
    pub run_id: String,
    pub workbook_session_id: String,
    pub status: String,
    pub pending: Option<ToolDispatch>,
    pub message: String,
    pub completed_actions: Vec<Json>,
    pub sources: Vec<Json>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryFile {
    pub workbook_id: String,
    pub title: String,
    pub latest_version: i64,
    pub source: String,
    pub preview: Vec<Vec<Json>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSnapshot {
    pub workbook_id: String,
    pub export_id: String,
    pub workbook_session_id: String,
    pub identity: Identity,
    pub storage_version: i64,
    pub sha256: String,
    pub byte_length: usize,
}

/// Monetary inputs are EUR, while the business query always computes integer cents.
pub fn review_values(review: &SalesReview) -> Vec<Vec<Json>> {
    let mut values = review.sheet_values();
    for row in values.iter_mut().skip(1).take(review.categories().len()) {
        for cell in row.iter_mut().skip(1).take(2) {
            if let Some(cents) = cell.as_i64() {
                *cell = json!(cents as f64 / 100.0);
            }
        }
    }
    for row in &mut values {
        if row.first().and_then(Json::as_str) == Some("Metric")
            && let Some(value) = row.get_mut(1)
        {
            *value = json!(review.provenance().metric_definition.replace("EUR cents", "EUR (computed from integer cents)"));
        }
    }
    values
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeechTranscript {
    pub text: String,
    pub language_code: Option<String>,
}

pub fn demo_catalog() -> [(&'static str, &'static str); 3] {
    [(WEEKLY_ID, "Weekly Sales.xlsx"), (FORECAST_ID, "Quarterly Forecast.xlsx"), (INVENTORY_ID, "Inventory.xlsx")]
}

pub fn demo_preview(id: &str) -> crate::Result<Vec<Vec<Json>>> {
    match id {
        WEEKLY_ID => {
            let review = compare_sales(&demo_sales(), &[], "synthetic-weekly-v1", ComparisonPeriods::demo())
                .map_err(|e| crate::EngineError::Other(e.to_string()))?;
            Ok(review_values(&review))
        }
        FORECAST_ID => Ok(vec![
            vec![json!("Quarter"), json!("Revenue EUR"), json!("Cost EUR"), json!("Margin EUR")],
            vec![json!("Q1 2027"), json!(12000), json!(8400), json!("=B2-C2")],
            vec![json!("Q2 2027"), json!(15000), json!(10200), json!("=B3-C3")],
            vec![json!("Total"), json!("=SUM(B2:B3)"), json!("=SUM(C2:C3)"), json!("=B4-C4")],
        ]),
        INVENTORY_ID => Ok(vec![
            vec![json!("Product"), json!("Units"), json!("Unit cost EUR"), json!("Value EUR")],
            vec![json!("Jackets"), json!(24), json!(35), json!("=B2*C2")],
            vec![json!("Shoes"), json!(40), json!(22), json!("=B3*C3")],
            vec![json!("Hats"), json!(18), json!(8), json!("=B4*C4")],
            vec![json!("Total"), json!("=SUM(B2:B4)"), Json::Null, json!("=SUM(D2:D4)")],
        ]),
        _ => Err(crate::EngineError::Other("unknown mock workbook".into())),
    }
}

/// Generate XLSX using the production writer. Callers open these bytes with `file.open`.
pub fn demo_xlsx(id: &str) -> crate::Result<Vec<u8>> {
    let title = demo_catalog()
        .into_iter()
        .find(|(key, _)| *key == id)
        .map(|(_, title)| title)
        .ok_or_else(|| crate::EngineError::Other("unknown mock workbook".into()))?;
    let mut session = Session::new();
    session.new_workbook();
    session.execute("sheet.rename", json!({"name": if id == WEEKLY_ID {"Review"} else {"Summary"}}))?;
    let values = demo_preview(id)?;
    let last_col = if id == WEEKLY_ID { "E" } else { "D" };
    session.execute("range.setValues", json!({"range":format!("A1:{last_col}{}", values.len()),"values":values}))?;
    session.execute("home.columnWidth", json!({"cols":"A:A", "chars":25}))?;
    session.execute("home.columnWidth", json!({"cols":"B:E", "chars":22}))?;
    session.execute("home.bold", json!({"range":format!("A1:{last_col}1"),"on":true}))?;
    let money = if id == WEEKLY_ID {
        "B2:D5"
    } else if id == FORECAST_ID {
        "B2:D4"
    } else {
        "C2:D5"
    };
    session.execute("home.numberFormat", json!({"range":money,"code":"\"€\"#,##0.00;[Red]-\"€\"#,##0.00"}))?;
    if id == WEEKLY_ID {
        session.execute("home.numberFormat", json!({"range":"E2:E5","code":"0.0%"}))?;
        session.execute("home.insertSheet", json!({"name":"Orders"}))?;
        let mut rows = vec![vec![
            json!("Record"),
            json!("Category"),
            json!("UTC timestamp"),
            json!("Gross EUR"),
            json!("Refund EUR"),
            json!("Net EUR"),
            json!("Status"),
            json!("Test order"),
        ]];
        for (i, sale) in demo_sales().iter().enumerate() {
            let row = i.saturating_add(2);
            rows.push(vec![
                json!(sale.id),
                json!(sale.category),
                json!(sale.timestamp_utc),
                json!(sale.amount_minor as f64 / 100.0),
                json!(sale.refunded_minor as f64 / 100.0),
                json!(format!("=D{row}-E{row}")),
                json!(format!("{:?}", sale.status)),
                json!(sale.is_test),
            ]);
        }
        session.execute("range.setValues", json!({"range":"A1:H13","values":rows}))?;
        session.execute("home.numberFormat", json!({"range":"D2:F13","code":"\"€\"#,##0.00"}))?;
        session.execute("home.columnWidth", json!({"cols":"A:H","chars":23}))?;
        session.execute("home.insertSheet", json!({"name":"Sources and Checks"}))?;
        session.execute(
            "range.setValues",
            json!({"range":"A1:B8","values":[
                ["Source","Original synthetic mock sales fixture v1; no business provenance verified"],
                ["Previous UTC week","2026-10-05 inclusive to 2026-10-12 exclusive"],
                ["Current UTC week","2026-10-12 inclusive to 2026-10-19 exclusive"],
                ["Metric","Paid-order EUR net revenue; refunds attributed to order period"],
                ["Records",12],["Initial rules","None; current EUR 655 includes EUR 500 test order"],
                ["With explicit exclusion","Previous EUR 150; current EUR 155"],
                ["Backend dataset",SALES_DATASET]
            ]}),
        )?;
        session.execute("home.columnWidth", json!({"cols":"A:A","chars":28}))?;
        session.execute("home.columnWidth", json!({"cols":"B:B","chars":90}))?;
        session.execute("sheet.activate", json!({"sheet":"Review"}))?;
    }
    crate::io::save_bytes(&session.doc()?.wb, title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{CellRef, Value};

    #[test]
    fn generated_files_open_with_formulas_formats_and_complete_fixture() {
        for (id, title) in demo_catalog() {
            let bytes = demo_xlsx(id).unwrap();
            assert!(bytes.starts_with(b"PK"));
            let mut session = Session::new();
            session.execute("file.open", json!({"name":title,"base64":crate::io::base64_encode(&bytes)})).unwrap();
            let doc = session.doc().unwrap();
            assert!(!doc.is_dirty());
            assert!(doc.undo.is_empty());
            if id == WEEKLY_ID {
                assert_eq!(doc.wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Review", "Orders", "Sources and Checks"]);
                let sheet = doc.wb.active().unwrap();
                assert_eq!(sheet.value(CellRef::parse("B5").unwrap()), Value::number(150.0));
                assert_eq!(sheet.value(CellRef::parse("C5").unwrap()), Value::number(655.0));
                assert_eq!(sheet.value(CellRef::parse("D5").unwrap()), Value::number(505.0));
                assert!(sheet.cell(CellRef::parse("D5").unwrap()).unwrap().formula.is_some());
                session.execute("cell.set", json!({"cell":"C2","input":"99"})).unwrap();
                assert_ne!(session.doc().unwrap().wb.active().unwrap().value(CellRef::parse("C5").unwrap()), Value::number(655.0));
            }
        }
    }
}
