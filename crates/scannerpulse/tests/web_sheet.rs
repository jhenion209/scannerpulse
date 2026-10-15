mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{TimeZone, Utc};
use common::{call, talkgroup};
use http_body_util::BodyExt;
use scannerpulse::web::{self, AppState};
use sqlx::PgPool;
use tower::ServiceExt;

/// The sheet's XML, read back from the stored zip.
fn sheet_xml(data: &[u8]) -> String {
    let mut pos = 0;
    loop {
        assert_eq!(
            data[pos..pos + 4],
            0x0403_4b50u32.to_le_bytes(),
            "a local header"
        );
        let size = u32::from_le_bytes(data[pos + 18..pos + 22].try_into().unwrap()) as usize;
        let name_len = u16::from_le_bytes(data[pos + 26..pos + 28].try_into().unwrap()) as usize;
        let name = &data[pos + 30..pos + 30 + name_len];
        let body = &data[pos + 30 + name_len..pos + 30 + name_len + size];
        if name == b"xl/worksheets/sheet1.xml" {
            return String::from_utf8(body.to_vec()).unwrap();
        }
        pos += 30 + name_len + size;
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn the_search_is_exported_newest_first_with_text_kept_as_text(pool: PgPool) {
    let tg = talkgroup(&pool, "SYSA", 7).await;
    let at = |minute| Utc.with_ymd_and_hms(2031, 7, 1, 17, minute, 0).unwrap();
    call(&pool, tg, at(0), Some("engine five responding")).await;
    call(&pool, tg, at(1), Some("=SUM(A1:A9) engine")).await;
    call(&pool, tg, at(2), Some("medic two")).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('time_zone', '\"America/New_York\"')")
        .execute(&pool)
        .await
        .unwrap();
    let app = web::router(AppState::new(
        pool.clone(),
        (*pool.connect_options()).clone(),
    ));
    let response = app
        .oneshot(
            Request::get("/api/export/xlsx?q=engine")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(
        headers["content-type"],
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    );
    let disposition = headers["content-disposition"].to_str().unwrap();
    assert!(
        disposition.contains("20310701_130000-20310701_130100_ScannerPulse_TG-7-SYSA.xlsx"),
        "{disposition}"
    );
    assert!(!headers.contains_key("x-export-capped-at"));
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let sheet = sheet_xml(&body);
    assert_eq!(
        sheet.matches("<row ").count(),
        3,
        "the header and two matches"
    );
    let formula = sheet.find("=SUM(A1:A9) engine").unwrap();
    let older = sheet.find("engine five responding").unwrap();
    assert!(formula < older, "newest first");
    assert!(!sheet.contains("<f>"));
    assert!(
        sheet.contains("<t xml:space=\"preserve\">13:01:00</t>"),
        "the site's local time"
    );
    assert!(sheet.contains("<c><v>2</v></c>"), "the length in seconds");
}
