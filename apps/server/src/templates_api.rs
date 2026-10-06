//! Working templates share the validated atomic new-book importer.
use crate::{AppState, documents::ApiError, pxel_file::*, pxel_workbook};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use spreadsheet_domain::{
    style::{CellStyle, Change, NumberFormat, Rgb, StyleChange},
    workbook::DocumentId,
};
use std::sync::Arc;
use uuid::Uuid;
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Template {
    Budget,
    Tasks,
    Sales,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
    schema_version: u32,
    document_id: String,
    request_id: String,
    template: Template,
}
fn text(row: u32, column: u32, value: &str) -> Cell {
    Cell {
        row,
        column,
        content: Content::Text(value.into()),
    }
}
fn number(row: u32, column: u32, value: f64) -> Cell {
    Cell {
        row,
        column,
        content: Content::Number(value),
    }
}
fn formula(row: u32, column: u32, value: &str) -> Cell {
    Cell {
        row,
        column,
        content: Content::Formula(value.into()),
    }
}
fn style(row: u32, column: u32, changes: &[StyleChange]) -> Result<Style, ApiError> {
    let style = CellStyle::default()
        .apply(changes)
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "template_invalid"))?;
    let serde_json::Value::Object(properties) =
        serde_json::to_value(crate::documents::CellStyleView::from(&style))
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "template_invalid"))?
    else {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "template_invalid",
        ));
    };
    Ok(Style {
        row,
        column,
        properties: properties.into_iter().collect(),
    })
}
pub(crate) fn book(template: Template) -> Result<Book, ApiError> {
    let (title, headers) = match template {
        Template::Budget => ("Бюджет", ["Категория", "План, ₽", "Факт, ₽", "Остаток, ₽"]),
        Template::Tasks => ("Учёт задач", ["Задача", "Исполнитель", "Статус", "Часы"]),
        Template::Sales => (
            "План продаж",
            ["Месяц", "План, ₽", "Продажи, ₽", "Выполнение"],
        ),
    };
    let mut cells = vec![text(0, 0, title)];
    for (column, header) in headers.iter().enumerate() {
        cells.push(text(1, column as u32, header));
    }
    match template {
        Template::Budget => {
            for (i, (name, plan, actual)) in [
                ("Аренда", 30000., 30000.),
                ("Продукты", 20000., 18400.),
                ("Транспорт", 7000., 6200.),
                ("Обучение", 5000., 3000.),
                ("Досуг", 8000., 5400.),
                ("Резерв", 10000., 0.),
            ]
            .into_iter()
            .enumerate()
            {
                let row = i as u32 + 2;
                cells.extend([
                    text(row, 0, name),
                    number(row, 1, plan),
                    number(row, 2, actual),
                    formula(row, 3, &format!("=B{}-C{}", row + 1, row + 1)),
                ]);
            }
            cells.extend([
                text(9, 0, "Итого"),
                formula(9, 1, "=SUM(B3:B8)"),
                formula(9, 2, "=SUM(C3:C8)"),
                formula(9, 3, "=B10-C10"),
            ]);
        }
        Template::Tasks => {
            for (i, (name, status, hours)) in [
                ("Собрать требования", "Готово", 3.),
                ("Подготовить макет", "Готово", 5.),
                ("Проверить данные", "В работе", 4.),
                ("Провести тестирование", "В работе", 6.),
                ("Согласовать результат", "Запланировано", 2.),
                ("Опубликовать отчёт", "Запланировано", 1.),
            ]
            .into_iter()
            .enumerate()
            {
                let row = i as u32 + 2;
                cells.extend([
                    text(row, 0, name),
                    text(row, 1, "Команда"),
                    text(row, 2, status),
                    number(row, 3, hours),
                ]);
            }
            cells.extend([
                text(9, 0, "Завершено задач"),
                formula(9, 1, "=COUNTIF(C3:C8,\"Готово\")"),
                text(10, 0, "Всего часов"),
                formula(10, 3, "=SUM(D3:D8)"),
            ]);
        }
        Template::Sales => {
            for (i, (name, plan, actual)) in [
                ("Январь", 100000., 95000.),
                ("Февраль", 120000., 125000.),
                ("Март", 140000., 137000.),
                ("Апрель", 150000., 155000.),
                ("Май", 160000., 168000.),
                ("Июнь", 180000., 172000.),
            ]
            .into_iter()
            .enumerate()
            {
                let row = i as u32 + 2;
                cells.extend([
                    text(row, 0, name),
                    number(row, 1, plan),
                    number(row, 2, actual),
                    formula(row, 3, &format!("=C{}/B{}", row + 1, row + 1)),
                ]);
            }
            cells.extend([
                text(9, 0, "Итого"),
                formula(9, 1, "=SUM(B3:B8)"),
                formula(9, 2, "=SUM(C3:C8)"),
                formula(9, 3, "=C10/B10"),
            ]);
        }
    }
    let mut styles = vec![style(
        0,
        0,
        &[
            StyleChange::FontBold(Change::Set(true)),
            StyleChange::Fill(Change::Set(Rgb::new([227, 242, 230]))),
        ],
    )?];
    for column in 0..4 {
        for row in [1, 9] {
            if matches!(template, Template::Sales) && row == 9 && column == 3 {
                continue;
            }
            styles.push(style(
                row,
                column,
                &[
                    StyleChange::FontBold(Change::Set(true)),
                    StyleChange::Fill(Change::Set(Rgb::new([227, 242, 230]))),
                ],
            )?);
        }
    }
    if matches!(template, Template::Sales) {
        for row in (2..8).chain(std::iter::once(9)) {
            let mut changes = vec![StyleChange::NumberFormat(Change::Set(
                NumberFormat::Percent0,
            ))];
            if row == 9 {
                changes.extend([
                    StyleChange::FontBold(Change::Set(true)),
                    StyleChange::Fill(Change::Set(Rgb::new([227, 242, 230]))),
                ]);
            }
            styles.push(style(row, 3, &changes)?);
        }
    }
    let filters = if matches!(template, Template::Tasks) {
        vec![Filter {
            sheet: 0,
            first_row: 1,
            last_row: 7,
            first_column: 0,
            last_column: 3,
            header_row: Some(1),
            conditions: vec![],
        }]
    } else {
        vec![]
    };
    Ok(Book {
        schema_version: 1,
        engine_version: spreadsheet_collab::calculation::ENGINE_VERSION.into(),
        compatibility_profile: spreadsheet_collab::calculation::COMPATIBILITY_PROFILE.into(),
        title: title.into(),
        anonymized: false,
        sheets: vec![Sheet {
            name: title.into(),
            rows: 1000,
            columns: 26,
            frozen_rows: 2,
            cells,
            styles,
            row_heights: vec![Size {
                position: 0,
                size_css_px: 40,
            }],
            column_widths: vec![
                Size {
                    position: 0,
                    size_css_px: 260,
                },
                Size {
                    position: 1,
                    size_css_px: 150,
                },
                Size {
                    position: 2,
                    size_css_px: 160,
                },
                Size {
                    position: 3,
                    size_css_px: 150,
                },
            ],
            merges: vec![Merge {
                first_row: 0,
                last_row: 0,
                first_column: 0,
                last_column: 3,
            }],
        }],
        settings: Settings {
            filters,
            ..Settings::default()
        },
    })
}
pub async fn create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<Create>,
) -> Result<(StatusCode, Json<crate::documents::Created>), ApiError> {
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "template_version"));
    }
    let principal = crate::catalog_api::member(&state, &headers, true).await?;
    let doc = DocumentId::from_bytes(*crate::catalog_api::uuid(&input.document_id)?.as_bytes())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))?;
    let request = crate::catalog_api::uuid(&input.request_id)?;
    let actor = principal.identity.actor_id;
    let permit = state
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "template_busy"))?;
    let (prepared, title) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let book = book(input.template)?;
        let bytes = serde_json::to_vec(&book)
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "template_invalid"))?;
        let digest = Sha256::digest(bytes);
        let prepared = pxel_workbook::prepare(&book, doc, actor, request, &digest)
            .map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "template_invalid"))?;
        Ok::<_, ApiError>((prepared, book.title))
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "template_unavailable"))??;
    if !matches!(state.auth.active(&principal).await, Ok(true)) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let receipt = state
        .database
        .create_pxel_imported(
            principal.identity,
            &prepared.events,
            &prepared.personal,
            &prepared.privacy,
            &title,
        )
        .await
        .map_err(crate::documents::journal_error)?;
    Ok((
        StatusCode::CREATED,
        Json(crate::documents::Created {
            document_id: Uuid::from_bytes(*doc.as_bytes()).to_string(),
            version: receipt.current_head_version.get().to_string(),
        }),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_templates_bind_calculate_and_preserve_guarded_merge_layout()
    -> Result<(), Box<dyn std::error::Error>> {
        let actor = spreadsheet_domain::workbook::ActorId::from_bytes(*Uuid::now_v7().as_bytes())?;
        for template in [Template::Budget, Template::Tasks, Template::Sales] {
            let book = book(template).map_err(|_| "template")?;
            let prepared = pxel_workbook::prepare(
                &book,
                DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?,
                actor,
                Uuid::now_v7(),
                b"template",
            )?;
            let limits = spreadsheet_formula::calculation::CalculationLimits::default();
            let mut calc = spreadsheet_formula::calculation::CalculationProjection::from_workbook(
                &prepared.book,
                limits,
            )?;
            calc.calculate(
                &prepared.book,
                limits,
                &std::sync::atomic::AtomicBool::new(false),
            )?;
            assert!(!calc.results().is_empty());
            assert!(
                calc.results()
                    .values()
                    .all(|v| !matches!(v, spreadsheet_formula::Value::Error(_)))
            );
            assert_eq!(prepared.book.sheets()[0].frozen_rows(), 2);
            assert_eq!(prepared.book.sheets()[0].merges().len(), 1);
            if matches!(template, Template::Tasks) {
                assert!(
                    calc.results()
                        .values()
                        .any(|v| *v == spreadsheet_formula::Value::Number(2.0))
                );
                assert!(
                    calc.results()
                        .values()
                        .any(|v| *v == spreadsheet_formula::Value::Number(21.0))
                );
            }
        }
        Ok(())
    }
}
