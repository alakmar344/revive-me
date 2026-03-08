# revive-me · Legacy Data Bridge 🦀

> Convert `.dbf` (FoxPro/dBase), `.xls`, `.xlsx`, `.csv`, `.tsv` files into clean modern JSON + Excel.  
> Built in **Rust** with Actix-Web. Zero cloud, 100% local, blazing fast.

---

## What it does

| Phase | Action |
|-------|--------|
| **Read** | Detects file type automatically, uses the right Rust reader (`dbase`, `calamine`, `csv`) |
| **Clean** | Drops empty rows, coerces strings → numbers, removes duplicates |
| **Export** | Outputs clean `.json` + modern `.xlsx` with a single download click |

---

## Project Structure

```
revive-me/
├── Cargo.toml          ← Rust dependencies
├── src/
│   └── main.rs         ← Full backend: readers + curation engine + Actix-Web API
├── static/
│   └── index.html      ← Frontend (drag-drop UI, no frameworks needed)
├── tmp/
│   ├── uploads/        ← Temporary upload storage (auto-created)
│   └── outputs/        ← Converted files served for download (auto-created)
└── README.md
```

---

## Setup & Run

### Prerequisites
- [Rust](https://rustup.rs/) (stable, 1.75+)

### Steps

```bash
# 1. Clone or extract the project
cd revive-me

# 2. Build in release mode (first build takes ~2 min)
cargo build --release

# 3. Run the server
cargo run --release

# 4. Open your browser
#    http://127.0.0.1:8080
```

---

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET`  | `/api/health` | Health check + supported formats |
| `POST` | `/api/upload` | Upload a legacy file → returns conversion record |
| `GET`  | `/api/download/{filename}` | Download a converted `.json` or `.xlsx` |
| `DELETE` | `/api/cleanup/{id}` | Delete output files for a job ID |

### Upload Response Example

```json
{
  "success": true,
  "message": "Conversion successful!",
  "data": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "original_name": "customers.dbf",
    "file_type": "dbf",
    "rows": 1842,
    "columns": 12,
    "duplicates_removed": 37,
    "timestamp": "2026-03-08T10:22:01Z",
    "output_json": "550e8400_clean.json",
    "output_xlsx": "550e8400_clean.xlsx"
  }
}
```

---

## Supported Formats

| Extension | Reader | Notes |
|-----------|--------|-------|
| `.dbf` | `dbase` crate | FoxPro 2.x, dBase III/IV/V |
| `.xls` | `calamine` crate | Excel 97–2003 |
| `.xlsx` | `calamine` crate | Excel 2007+ |
| `.csv` | `csv` crate | Auto type-coercion |
| `.tsv` | `csv` crate | Tab-separated |

---

## Curation Engine (What Gets Cleaned)

1. **Type Casting** — `"100.50"` → `100.5` (float), `"TRUE"` → `true` (bool)
2. **Empty Row Removal** — Rows where every field is null/blank are dropped
3. **Deduplication** — Identical rows (compared as JSON) are removed; count shown in UI

---

## Configuration

Edit constants at the top of `src/main.rs`:

```rust
const UPLOAD_DIR: &str = "./tmp/uploads";   // where uploads land
const OUTPUT_DIR: &str = "./tmp/outputs";   // where clean files go
const MAX_FILE_SIZE: usize = 50 * 1024 * 1024; // 50 MB limit
```

---

## Built by

**Al Aqmar Tinwala** — eSAMz AI · revive-me · CiboCocinar  
Rust backend · Zero cloud · Privacy-first
