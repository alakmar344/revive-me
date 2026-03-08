// revive-me — Legacy Data Bridge
// Converts .dbf / .xls / .xlsx / .csv  →  clean JSON + modern .xlsx
// Backend: Rust + Actix-web
// Author: Al Aqmar Tinwala

use actix_files::Files;
use actix_multipart::Multipart;
use actix_web::{
    delete, get, middleware, post, web, App, HttpRequest, HttpResponse, HttpServer, Responder,
};
use anyhow::{anyhow, Context, Result};
use calamine::{open_workbook_auto, DataType, Reader};
use chrono::Utc;
use dbase::FieldValue;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;
use xlsxwriter::*;

// ─── Constants ────────────────────────────────────────────────────────────────

const UPLOAD_DIR: &str = "./tmp/uploads";
const OUTPUT_DIR: &str = "./tmp/outputs";
const MAX_FILE_SIZE: usize = 50 * 1024 * 1024; // 50 MB

// ─── Data Structures ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConversionRecord {
    pub id: String,
    pub original_name: String,
    pub file_type: String,
    pub rows: usize,
    pub columns: usize,
    pub duplicates_removed: usize,
    pub timestamp: String,
    pub output_json: String,
    pub output_xlsx: String,
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub message: String,
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    fn ok(msg: &str, data: T) -> Self {
        Self { success: true, message: msg.to_string(), data: Some(data) }
    }
    fn err(msg: &str) -> ApiResponse<()> {
        ApiResponse { success: false, message: msg.to_string(), data: None }
    }
}

// ─── File Type Detection ───────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum LegacyFormat {
    Dbf,
    Xls,
    Xlsx,
    Csv,
    Tsv,
    Unknown,
}

fn detect_format(path: &Path) -> LegacyFormat {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
        Some("dbf") => LegacyFormat::Dbf,
        Some("xls") => LegacyFormat::Xls,
        Some("xlsx") => LegacyFormat::Xlsx,
        Some("csv") => LegacyFormat::Csv,
        Some("tsv") => LegacyFormat::Tsv,
        _ => LegacyFormat::Unknown,
    }
}

// ─── Readers ──────────────────────────────────────────────────────────────────

/// Read a .dbf file → Vec<HashMap<String, serde_json::Value>>
fn read_dbf(path: &Path) -> Result<Vec<HashMap<String, serde_json::Value>>> {
    let mut reader = dbase::Reader::from_path(path)
        .context("Failed to open .dbf file")?;
    let records = reader.read()
        .context("Failed to read .dbf records")?;

    let mut rows: Vec<HashMap<String, serde_json::Value>> = Vec::new();

    for record in records {
        let mut map: HashMap<String, serde_json::Value> = HashMap::new();
        for (name, value) in record.into_iter() {
            let json_val = match value {
                FieldValue::Character(Some(s)) => serde_json::Value::String(s.trim().to_string()),
                FieldValue::Numeric(Some(n)) => {
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(n)
                            .unwrap_or_else(|| serde_json::Number::from(0)),
                    )
                }
                FieldValue::Float(Some(f)) => {
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(f as f64)
                            .unwrap_or_else(|| serde_json::Number::from(0)),
                    )
                }
                FieldValue::Logical(Some(b)) => serde_json::Value::Bool(b),
                FieldValue::Date(Some(d)) => {
                    serde_json::Value::String(format!("{}-{:02}-{:02}", d.year(), d.month(), d.day()))
                }
                FieldValue::Integer(n) => serde_json::Value::Number(serde_json::Number::from(n)),
                _ => serde_json::Value::Null,
            };
            map.insert(name, json_val);
        }
        rows.push(map);
    }
    Ok(rows)
}

/// Read .xls / .xlsx using calamine
fn read_excel(path: &Path) -> Result<Vec<HashMap<String, serde_json::Value>>> {
    let mut workbook = open_workbook_auto(path)
        .context("Failed to open Excel file")?;

    // Use first sheet
    let sheet_name = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("No sheets found in workbook"))?;

    let range = workbook
        .worksheet_range(&sheet_name)
        .context("Failed to read worksheet")?;

    let mut iter = range.rows();
    let headers: Vec<String> = match iter.next() {
        Some(row) => row
            .iter()
            .map(|c| match c {
                DataType::String(s) => s.trim().to_string(),
                DataType::Float(f) => f.to_string(),
                DataType::Int(i) => i.to_string(),
                _ => String::from("column"),
            })
            .collect(),
        None => return Ok(vec![]),
    };

    // De-duplicate header names by appending index
    let headers: Vec<String> = headers
        .into_iter()
        .enumerate()
        .map(|(i, h)| if h.is_empty() { format!("col_{i}") } else { h })
        .collect();

    let mut rows = Vec::new();
    for row in iter {
        let mut map: HashMap<String, serde_json::Value> = HashMap::new();
        for (i, cell) in row.iter().enumerate() {
            let key = headers.get(i).cloned().unwrap_or_else(|| format!("col_{i}"));
            let val = match cell {
                DataType::String(s) => {
                    let trimmed = s.trim();
                    // Try numeric coercion
                    if let Ok(f) = trimmed.parse::<f64>() {
                        serde_json::Value::Number(
                            serde_json::Number::from_f64(f)
                                .unwrap_or_else(|| serde_json::Number::from(0)),
                        )
                    } else {
                        serde_json::Value::String(trimmed.to_string())
                    }
                }
                DataType::Float(f) => serde_json::Value::Number(
                    serde_json::Number::from_f64(*f)
                        .unwrap_or_else(|| serde_json::Number::from(0)),
                ),
                DataType::Int(i) => serde_json::Value::Number(serde_json::Number::from(*i)),
                DataType::Bool(b) => serde_json::Value::Bool(*b),
                DataType::Empty => serde_json::Value::Null,
                _ => serde_json::Value::String(cell.to_string()),
            };
            map.insert(key, val);
        }
        rows.push(map);
    }
    Ok(rows)
}

/// Read CSV / TSV
fn read_csv(path: &Path, delimiter: u8) -> Result<Vec<HashMap<String, serde_json::Value>>> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .from_path(path)
        .context("Failed to open CSV file")?;

    let headers: Vec<String> = rdr
        .headers()
        .context("Failed to read CSV headers")?
        .iter()
        .map(|s| s.trim().to_string())
        .collect();

    let mut rows = Vec::new();
    for result in rdr.records() {
        let record = result.context("Failed to read CSV record")?;
        let mut map: HashMap<String, serde_json::Value> = HashMap::new();
        for (i, field) in record.iter().enumerate() {
            let key = headers.get(i).cloned().unwrap_or_else(|| format!("col_{i}"));
            let trimmed = field.trim();
            let val = if let Ok(i) = trimmed.parse::<i64>() {
                serde_json::Value::Number(serde_json::Number::from(i))
            } else if let Ok(f) = trimmed.parse::<f64>() {
                serde_json::Value::Number(
                    serde_json::Number::from_f64(f)
                        .unwrap_or_else(|| serde_json::Number::from(0)),
                )
            } else if trimmed.eq_ignore_ascii_case("true") {
                serde_json::Value::Bool(true)
            } else if trimmed.eq_ignore_ascii_case("false") {
                serde_json::Value::Bool(false)
            } else if trimmed.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(trimmed.to_string())
            };
            map.insert(key, val);
        }
        rows.push(map);
    }
    Ok(rows)
}

// ─── Curation Engine ──────────────────────────────────────────────────────────

/// Remove duplicate rows (simple JSON-string comparison)
fn deduplicate(rows: Vec<HashMap<String, serde_json::Value>>) -> (Vec<HashMap<String, serde_json::Value>>, usize) {
    let mut seen = std::collections::HashSet::new();
    let original_len = rows.len();
    let deduped: Vec<_> = rows
        .into_iter()
        .filter(|row| seen.insert(serde_json::to_string(row).unwrap_or_default()))
        .collect();
    let removed = original_len - deduped.len();
    (deduped, removed)
}

/// Remove rows that are entirely null/empty
fn drop_empty_rows(rows: Vec<HashMap<String, serde_json::Value>>) -> Vec<HashMap<String, serde_json::Value>> {
    rows.into_iter()
        .filter(|row| row.values().any(|v| !matches!(v, serde_json::Value::Null) && v != ""))
        .collect()
}

// ─── Writer ───────────────────────────────────────────────────────────────────

fn write_xlsx(rows: &[HashMap<String, serde_json::Value>], path: &Path) -> Result<()> {
    if rows.is_empty() {
        // Write empty file
        let wb = Workbook::new(path.to_str().unwrap())?;
        let mut ws = wb.add_worksheet(None)?;
        ws.write_string(0, 0, "No data", None)?;
        wb.close()?;
        return Ok(());
    }

    // Collect ordered headers (preserve insertion order from first row)
    let headers: Vec<String> = rows[0].keys().cloned().collect();

    let wb = Workbook::new(path.to_str().unwrap())?;
    let mut ws = wb.add_worksheet(Some("Clean Data"))?;

    // Header format — bold
    let bold = wb.add_format().set_bold();

    for (col, header) in headers.iter().enumerate() {
        ws.write_string(0, col as u16, header, Some(&bold))?;
    }

    for (row_i, row) in rows.iter().enumerate() {
        for (col_i, key) in headers.iter().enumerate() {
            let val = row.get(key).unwrap_or(&serde_json::Value::Null);
            match val {
                serde_json::Value::Number(n) => {
                    ws.write_number((row_i + 1) as u32, col_i as u16, n.as_f64().unwrap_or(0.0), None)?;
                }
                serde_json::Value::Bool(b) => {
                    ws.write_string((row_i + 1) as u32, col_i as u16, if *b { "TRUE" } else { "FALSE" }, None)?;
                }
                serde_json::Value::Null => {
                    ws.write_blank((row_i + 1) as u32, col_i as u16, None)?;
                }
                other => {
                    ws.write_string((row_i + 1) as u32, col_i as u16, &other.to_string().trim_matches('"').to_string(), None)?;
                }
            }
        }
    }

    wb.close()?;
    Ok(())
}

// ─── Core Conversion Pipeline ─────────────────────────────────────────────────

fn convert_legacy(input_path: &Path, original_name: &str, job_id: &str) -> Result<ConversionRecord> {
    let format = detect_format(input_path);

    let raw_rows = match format {
        LegacyFormat::Dbf  => read_dbf(input_path)?,
        LegacyFormat::Xls | LegacyFormat::Xlsx => read_excel(input_path)?,
        LegacyFormat::Csv  => read_csv(input_path, b',')?,
        LegacyFormat::Tsv  => read_csv(input_path, b'\t')?,
        LegacyFormat::Unknown => return Err(anyhow!("Unsupported file type. Accepted: .dbf, .xls, .xlsx, .csv, .tsv")),
    };

    let file_type = format!("{:?}", format).to_lowercase();
    let cleaned = drop_empty_rows(raw_rows);
    let columns = cleaned.first().map(|r| r.len()).unwrap_or(0);
    let (deduped, dups_removed) = deduplicate(cleaned);
    let rows = deduped.len();

    // Paths for outputs
    fs::create_dir_all(OUTPUT_DIR)?;
    let json_filename = format!("{job_id}_clean.json");
    let xlsx_filename = format!("{job_id}_clean.xlsx");
    let json_path = PathBuf::from(OUTPUT_DIR).join(&json_filename);
    let xlsx_path = PathBuf::from(OUTPUT_DIR).join(&xlsx_filename);

    // Write JSON
    let json_str = serde_json::to_string_pretty(&deduped)?;
    fs::write(&json_path, &json_str)?;

    // Write XLSX
    write_xlsx(&deduped, &xlsx_path)?;

    Ok(ConversionRecord {
        id: job_id.to_string(),
        original_name: original_name.to_string(),
        file_type,
        rows,
        columns,
        duplicates_removed: dups_removed,
        timestamp: Utc::now().to_rfc3339(),
        output_json: json_filename,
        output_xlsx: xlsx_filename,
    })
}

// ─── HTTP Handlers ────────────────────────────────────────────────────────────

#[get("/api/health")]
async fn health() -> impl Responder {
    HttpResponse::Ok().json(ApiResponse::ok("revive-me is running 🟢", serde_json::json!({
        "version": "1.0.0",
        "supported": [".dbf", ".xls", ".xlsx", ".csv", ".tsv"]
    })))
}

#[post("/api/upload")]
async fn upload(mut payload: Multipart) -> impl Responder {
    fs::create_dir_all(UPLOAD_DIR).ok();

    let job_id = Uuid::new_v4().to_string();
    let mut original_name = String::from("unknown");
    let mut saved_path: Option<PathBuf> = None;

    // Stream multipart fields
    while let Some(Ok(mut field)) = payload.next().await {
        let disposition = field.content_disposition();
        let filename = disposition
            .get_filename()
            .map(sanitize_filename::sanitize)
            .unwrap_or_else(|| "upload.bin".to_string());
        original_name = filename.clone();

        let file_path = PathBuf::from(UPLOAD_DIR).join(format!("{job_id}_{filename}"));
        let mut file_bytes: Vec<u8> = Vec::new();

        while let Some(Ok(chunk)) = field.next().await {
            file_bytes.extend_from_slice(&chunk);
            if file_bytes.len() > MAX_FILE_SIZE {
                return HttpResponse::PayloadTooLarge()
                    .json(ApiResponse::<()>::err("File exceeds 50 MB limit"));
            }
        }

        if let Err(e) = fs::write(&file_path, &file_bytes) {
            return HttpResponse::InternalServerError()
                .json(ApiResponse::<()>::err(&format!("Failed to save file: {e}")));
        }

        saved_path = Some(file_path);
    }

    let input = match saved_path {
        Some(p) => p,
        None => {
            return HttpResponse::BadRequest()
                .json(ApiResponse::<()>::err("No file received in request"))
        }
    };

    // Run conversion
    match convert_legacy(&input, &original_name, &job_id) {
        Ok(record) => {
            // Clean up the upload temp file
            fs::remove_file(&input).ok();
            HttpResponse::Ok().json(ApiResponse::ok("Conversion successful!", record))
        }
        Err(e) => {
            fs::remove_file(&input).ok();
            HttpResponse::UnprocessableEntity()
                .json(ApiResponse::<()>::err(&format!("Conversion failed: {e}")))
        }
    }
}

#[get("/api/download/{filename}")]
async fn download(req: HttpRequest, filename: web::Path<String>) -> impl Responder {
    let safe_name = sanitize_filename::sanitize(filename.as_str());
    let file_path = PathBuf::from(OUTPUT_DIR).join(&safe_name);

    if !file_path.exists() {
        return HttpResponse::NotFound().json(ApiResponse::<()>::err("File not found"));
    }

    match fs::read(&file_path) {
        Ok(bytes) => {
            let mime = if safe_name.ends_with(".json") {
                "application/json"
            } else {
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            };
            HttpResponse::Ok()
                .insert_header(("Content-Type", mime))
                .insert_header(("Content-Disposition", format!("attachment; filename=\"{safe_name}\"")))
                .body(bytes)
        }
        Err(e) => HttpResponse::InternalServerError()
            .json(ApiResponse::<()>::err(&format!("Read error: {e}"))),
    }
}

#[delete("/api/cleanup/{id}")]
async fn cleanup(id: web::Path<String>) -> impl Responder {
    let safe_id = sanitize_filename::sanitize(id.as_str());
    let mut deleted = 0usize;
    for ext in &["_clean.json", "_clean.xlsx"] {
        let path = PathBuf::from(OUTPUT_DIR).join(format!("{safe_id}{ext}"));
        if path.exists() {
            fs::remove_file(&path).ok();
            deleted += 1;
        }
    }
    HttpResponse::Ok().json(ApiResponse::ok(&format!("Removed {deleted} file(s)"), deleted))
}

// ─── Main ─────────────────────────────────────────────────────────────────────

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    fs::create_dir_all(UPLOAD_DIR).ok();
    fs::create_dir_all(OUTPUT_DIR).ok();

    println!("🟢 revive-me Legacy Data Bridge");
    println!("   Server: http://127.0.0.1:8080");
    println!("   Drag & drop .dbf / .xls / .xlsx / .csv / .tsv to convert");

    HttpServer::new(|| {
        App::new()
            .service(health)
            .service(upload)
            .service(download)
            .service(cleanup)
            // Serve static frontend from ./static/
            .service(Files::new("/", "./static").index_file("index.html"))
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}
