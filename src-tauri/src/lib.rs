//! Backend de Soft Explorer: catálogo SQLite, indexado incremental, cachés de imagen
//! fuera de la biblioteca y comandos Tauri invocados por la interfaz React.

use image::{DynamicImage, ImageFormat, ImageReader};
use natord::compare as natural_compare;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};
use tauri::{AppHandle, Emitter, Manager, State};

/// Directorios privados de datos derivados; ninguna ruta apunta a los originales.
#[derive(Clone)]
struct AppData {
    database: PathBuf,
    thumbnails: PathBuf,
    readers: PathBuf,
    // Limita las decodificaciones simultáneas: varias portadas grandes no deben disparar
    // a la vez el consumo de memoria del proceso.
    image_gate: Arc<Mutex<()>>,
}

/// Registro enviado al frontend; `camelCase` coincide con los tipos de TypeScript.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Manga {
    id: i64,
    path: String,
    title: String,
    genre: String,
    page_count: i64,
    cover_path: String,
    favorite: bool,
    custom_cover: bool,
    last_page: i64,
    last_read: i64,
    added_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitialState {
    library_path: Option<String>,
    manga_count: i64,
    genres: Vec<String>,
    settings: HashMap<String, String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanProgress {
    done: usize,
    total: usize,
    genres: usize,
    images: usize,
    current: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanSummary {
    mangas: usize,
    genres: usize,
    images: usize,
}

#[derive(Clone)]
struct CachedManga {
    pages: i64,
    cover: String,
    folder_mtime: i64,
}

struct ScannedManga {
    path: String,
    title: String,
    genre: String,
    page_count: i64,
    cover_path: String,
    folder_mtime: i64,
}

/// Abre una conexión corta por comando; WAL permite que lectura y escritura convivan.
fn connect(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=10000;")
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

/// Crea el esquema en la carpeta de datos del usuario y añade la columna incremental
/// cuando se abre una base creada por una versión anterior.
fn initialize_database(path: &Path) -> Result<(), String> {
    let db = connect(path)?;
    db.execute_batch(
        "PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS manga(
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            title TEXT NOT NULL,
            genre TEXT NOT NULL,
            page_count INTEGER NOT NULL DEFAULT 0,
            cover_path TEXT NOT NULL DEFAULT '',
            folder_mtime INTEGER NOT NULL DEFAULT 0,
            custom_cover INTEGER NOT NULL DEFAULT 0,
            favorite INTEGER NOT NULL DEFAULT 0,
            last_page INTEGER NOT NULL DEFAULT 0,
            last_read INTEGER NOT NULL DEFAULT 0,
            added_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        );
        CREATE INDEX IF NOT EXISTS manga_title_idx ON manga(title COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS manga_genre_idx ON manga(genre COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS manga_favorite_idx ON manga(favorite);
        CREATE INDEX IF NOT EXISTS manga_last_read_idx ON manga(last_read);
        CREATE TEMP TABLE IF NOT EXISTS scan_keep(path TEXT PRIMARY KEY);",
    )
    .map_err(|error| error.to_string())?;

    let mut columns = db
        .prepare("PRAGMA table_info(manga)")
        .map_err(|error| error.to_string())?;
    let names: Vec<String> = columns
        .query_map([], |row| row.get(1))
        .map_err(|error| error.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|error| error.to_string())?;
    if !names.iter().any(|name| name == "folder_mtime") {
        db.execute(
            "ALTER TABLE manga ADD COLUMN folder_mtime INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn setting(db: &Connection, key: &str) -> Result<Option<String>, String> {
    db.query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
        row.get(0)
    })
    .optional()
    .map_err(|error| error.to_string())
}

/// Guarda preferencias sin construir SQL con valores proporcionados por la interfaz.
fn set_setting(db: &Connection, key: &str, value: &str) -> Result<(), String> {
    db.execute(
        "INSERT INTO settings(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn read_manga(row: &rusqlite::Row<'_>) -> rusqlite::Result<Manga> {
    Ok(Manga {
        id: row.get(0)?,
        path: row.get(1)?,
        title: row.get(2)?,
        genre: row.get(3)?,
        page_count: row.get(4)?,
        cover_path: row.get(5)?,
        favorite: row.get::<_, i64>(6)? != 0,
        custom_cover: row.get::<_, i64>(7)? != 0,
        last_page: row.get(8)?,
        last_read: row.get(9)?,
        added_at: row.get(10)?,
    })
}

/// Lee solo metadatos; nunca carga páginas ni portadas a memoria con el catálogo.
fn all_manga(db: &Connection) -> Result<Vec<Manga>, String> {
    let mut statement = db
        .prepare(
            "SELECT id,path,title,genre,page_count,cover_path,favorite,custom_cover,
                    last_page,last_read,added_at FROM manga",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], read_manga)
        .map_err(|error| error.to_string())?;
    let mangas = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(mangas)
}

/// Formatos admitidos. Las miniaturas con prefijo reservado se excluyen para no
/// reindexar archivos derivados como si fueran páginas originales.
fn image_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "gif" | "avif" | "tif" | "tiff"
    ) && !path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .starts_with("thumb_")
        && !path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .starts_with("thumbnail_")
}

fn is_hidden(path: &Path) -> bool {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .starts_with('.')
    {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Ok(metadata) = fs::metadata(path) {
            return metadata.file_attributes() & 0x2 != 0;
        }
    }
    false
}

/// Ordena archivos y carpetas de manera natural: `2.jpg` aparece antes de `10.jpg`.
fn natural_entries(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort_by(|left, right| {
        natural_compare(
            &left.file_name().unwrap_or_default().to_string_lossy(),
            &right.file_name().unwrap_or_default().to_string_lossy(),
        )
    });
    paths
}

/// Lista únicamente imágenes directas de una obra y las ordena de forma natural.
fn image_pages(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(folder).map_err(|error| error.to_string())?;
    let mut pages = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_file() && !is_hidden(&path) && image_file(&path) {
            pages.push(path);
        }
    }
    Ok(natural_entries(pages))
}

fn first_readable_image(pages: &[PathBuf]) -> &Path {
    pages
        .iter()
        .find(|path| {
            ImageReader::open(path)
                .ok()
                .and_then(|reader| reader.with_guessed_format().ok())
                .and_then(|reader| reader.into_dimensions().ok())
                .is_some()
        })
        .map(PathBuf::as_path)
        .unwrap_or(&pages[0])
}

/// Usa la fecha de modificación del directorio como marca incremental. Añadir o quitar
/// páginas cambia esta marca; si permanece igual, se reutilizan portada y recuento.
fn file_time_ns(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

/// Escanea `Biblioteca/Género/Obra` en un worker de Tauri. Primero reúne un catálogo
/// completo; solo después sincroniza SQLite en una transacción para no perder datos
/// si una carpeta no se pudo leer.
fn scan_and_persist(app: &AppHandle, database: &Path) -> Result<ScanSummary, String> {
    let db = connect(database)?;
    let root = setting(&db, "library")?.ok_or("Selecciona una carpeta de biblioteca primero")?;
    let root = PathBuf::from(root);
    if !root.is_dir() {
        return Err("La carpeta de biblioteca ya no está disponible".into());
    }

    // La caché por ruta permite saltar el listado de páginas de las obras sin cambios.
    let mut cached = HashMap::<String, CachedManga>::new();
    {
        let mut statement = db
            .prepare("SELECT path,page_count,cover_path,folder_mtime FROM manga")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    CachedManga {
                        pages: row.get(1)?,
                        cover: row.get(2)?,
                        folder_mtime: row.get(3)?,
                    },
                ))
            })
            .map_err(|error| error.to_string())?;
        for row in rows.flatten() {
            cached.insert(row.0, row.1);
        }
    }

    let mut genre_paths = Vec::new();
    for entry in
        fs::read_dir(&root).map_err(|error| format!("No se puede leer la biblioteca: {error}"))?
    {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() && !is_hidden(&path) {
            genre_paths.push(path);
        }
    }
    let genres = natural_entries(genre_paths);
    let mut manga_dirs: Vec<(PathBuf, String)> = Vec::new();
    for genre in &genres {
        let genre_name = genre
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let entries = fs::read_dir(genre).map_err(|error| error.to_string())?;
        for entry in entries {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_dir() && !is_hidden(&path) {
                manga_dirs.push((path, genre_name.clone()));
            }
        }
    }
    manga_dirs.sort_by(|left, right| {
        let genre_order = natural_compare(&left.1, &right.1);
        if genre_order.is_eq() {
            natural_compare(
                &left.0.file_name().unwrap_or_default().to_string_lossy(),
                &right.0.file_name().unwrap_or_default().to_string_lossy(),
            )
        } else {
            genre_order
        }
    });

    let mut scanned = Vec::with_capacity(manga_dirs.len());
    let mut image_count = 0usize;
    let total = manga_dirs.len();
    for (index, (folder, genre)) in manga_dirs.iter().enumerate() {
        let path = folder.canonicalize().unwrap_or_else(|_| folder.clone());
        let path_text = path.to_string_lossy().to_string();
        let folder_mtime = file_time_ns(folder);
        let title = folder
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let record = if let Some(old) = cached
            .get(&path_text)
            .filter(|old| old.folder_mtime == folder_mtime && old.pages > 0)
        {
            ScannedManga {
                path: path_text,
                title,
                genre: genre.clone(),
                page_count: old.pages,
                cover_path: old.cover.clone(),
                folder_mtime,
            }
        } else {
            let pages = match image_pages(folder) {
                Ok(pages) => pages,
                Err(error) => return Err(format!("No se pudo leer {}: {error}", folder.display())),
            };
            if pages.is_empty() {
                if (index + 1) % 50 == 0 || index + 1 == total {
                    emit_progress(app, index + 1, total, genres.len(), image_count, &title);
                }
                continue;
            }
            ScannedManga {
                path: path_text,
                title,
                genre: genre.clone(),
                page_count: pages.len() as i64,
                cover_path: first_readable_image(&pages)
                    .canonicalize()
                    .unwrap_or_else(|_| first_readable_image(&pages).to_path_buf())
                    .to_string_lossy()
                    .to_string(),
                folder_mtime,
            }
        };
        image_count += record.page_count as usize;
        // Limita los eventos IPC: una biblioteca enorme no debe emitir un evento por obra.
        if (index + 1) % 50 == 0 || index + 1 == total {
            emit_progress(
                app,
                index + 1,
                total,
                genres.len(),
                image_count,
                &format!("{} · {}", genre, record.title),
            );
        }
        scanned.push(record);
    }

    // Una tabla temporal evita listas SQL gigantes (y el límite de parámetros de SQLite)
    // al borrar entradas que ya no existen.
    db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS scan_keep(path TEXT PRIMARY KEY)")
        .map_err(|error| error.to_string())?;
    let transaction = db
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM scan_keep", [])
        .map_err(|error| error.to_string())?;
    {
        let mut upsert = transaction
            .prepare(
                "INSERT INTO manga(path,title,genre,page_count,cover_path,folder_mtime)
                 VALUES(?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(path) DO UPDATE SET title=excluded.title,genre=excluded.genre,
                    page_count=excluded.page_count,folder_mtime=excluded.folder_mtime,
                    cover_path=CASE WHEN manga.custom_cover=0 THEN excluded.cover_path ELSE manga.cover_path END",
            )
            .map_err(|error| error.to_string())?;
        for manga in &scanned {
            upsert
                .execute(params![
                    manga.path,
                    manga.title,
                    manga.genre,
                    manga.page_count,
                    manga.cover_path,
                    manga.folder_mtime
                ])
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO scan_keep(path) VALUES(?1)",
                    [&manga.path],
                )
                .map_err(|error| error.to_string())?;
        }
    }
    transaction
        .execute(
            "DELETE FROM manga WHERE path NOT IN (SELECT path FROM scan_keep)",
            [],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;

    Ok(ScanSummary {
        mangas: scanned.len(),
        genres: genres.len(),
        images: image_count,
    })
}

fn emit_progress(
    app: &AppHandle,
    done: usize,
    total: usize,
    genres: usize,
    images: usize,
    current: &str,
) {
    let _ = app.emit(
        "scan-progress",
        ScanProgress {
            done,
            total,
            genres,
            images,
            current: current.to_string(),
        },
    );
}

/// Estado inicial pequeño para construir la navegación antes de pedir el listado completo.
/// Devuelve contadores, categorías y preferencias para montar la ventana principal.
#[tauri::command]
fn get_initial_state(state: State<'_, AppData>) -> Result<InitialState, String> {
    let db = connect(&state.database)?;
    let library_path = setting(&db, "library")?;
    let manga_count = db
        .query_row("SELECT COUNT(*) FROM manga", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    let mut genre_statement = db
        .prepare("SELECT DISTINCT genre FROM manga ORDER BY genre COLLATE NOCASE")
        .map_err(|error| error.to_string())?;
    let genres = genre_statement
        .query_map([], |row| row.get(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|error| error.to_string())?;
    let mut settings = HashMap::new();
    let mut statement = db
        .prepare("SELECT key,value FROM settings")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?;
    for row in rows.flatten() {
        settings.insert(row.0, row.1);
    }
    Ok(InitialState {
        library_path,
        manga_count,
        genres,
        settings,
    })
}

/// Persiste la raíz seleccionada y habilita acceso de solo lectura al protocolo de imágenes.
#[tauri::command]
fn set_library_path(app: AppHandle, state: State<'_, AppData>, path: String) -> Result<(), String> {
    let folder = PathBuf::from(&path);
    if !folder.is_dir() {
        return Err("La carpeta seleccionada no existe o no es accesible".into());
    }
    // Autoriza al WebView a mostrar imágenes solo dentro de la carpeta que eligió la persona.
    app.asset_protocol_scope()
        .allow_directory(&folder, true)
        .map_err(|error| error.to_string())?;
    let db = connect(&state.database)?;
    set_setting(
        &db,
        "library",
        &folder.canonicalize().unwrap_or(folder).to_string_lossy(),
    )
}

/// Ejecuta el escáner fuera del hilo de UI y envía eventos de progreso al frontend.
#[tauri::command]
async fn scan_library(app: AppHandle, state: State<'_, AppData>) -> Result<ScanSummary, String> {
    let app_handle = app.clone();
    let database = state.database.clone();
    tauri::async_runtime::spawn_blocking(move || scan_and_persist(&app_handle, &database))
        .await
        .map_err(|error| error.to_string())?
}

/// Filtra y ordena metadatos; React recibe una lista virtualizada y pide portadas visibles.
#[tauri::command]
fn list_manga(
    state: State<'_, AppData>,
    query: String,
    genre: Option<String>,
    favorites_only: bool,
    sort: String,
) -> Result<Vec<Manga>, String> {
    let db = connect(&state.database)?;
    let query = query.trim().to_lowercase();
    let genre = genre.unwrap_or_default();
    let mut mangas = all_manga(&db)?
        .into_iter()
        .filter(|manga| query.is_empty() || manga.title.to_lowercase().contains(&query))
        .filter(|manga| genre.is_empty() || manga.genre == genre)
        .filter(|manga| !favorites_only || manga.favorite)
        .collect::<Vec<_>>();
    match sort.as_str() {
        "Nombre Z-A" => mangas.sort_by(|a, b| natural_compare(&b.title, &a.title)),
        "Género" => mangas.sort_by(|a, b| {
            a.genre
                .to_lowercase()
                .cmp(&b.genre.to_lowercase())
                .then_with(|| natural_compare(&a.title, &b.title))
        }),
        "Favoritos primero" => mangas.sort_by(|a, b| {
            b.favorite
                .cmp(&a.favorite)
                .then_with(|| natural_compare(&a.title, &b.title))
        }),
        "Últimos añadidos" => mangas.sort_by(|a, b| b.added_at.cmp(&a.added_at)),
        "Últimos leídos" => mangas.sort_by(|a, b| b.last_read.cmp(&a.last_read)),
        _ => mangas.sort_by(|a, b| natural_compare(&a.title, &b.title)),
    }
    Ok(mangas)
}

/// Devuelve rutas de página ordenadas; las imágenes se leen bajo demanda en el lector.
#[tauri::command]
fn get_pages(state: State<'_, AppData>, manga_id: i64) -> Result<Vec<String>, String> {
    let db = connect(&state.database)?;
    let folder: String = db
        .query_row("SELECT path FROM manga WHERE id=?1", [manga_id], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    image_pages(Path::new(&folder)).map(|pages| {
        pages
            .into_iter()
            .map(|path| {
                path.canonicalize()
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string()
            })
            .collect()
    })
}

/// Cambia el favorito en SQLite, identificado por ID estable y no por posición del grid.
#[tauri::command]
fn toggle_favorite(state: State<'_, AppData>, manga_id: i64) -> Result<(), String> {
    let db = connect(&state.database)?;
    db.execute(
        "UPDATE manga SET favorite=1-favorite WHERE id=?1",
        [manga_id],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// Guarda la ruta de portada elegida, sin copiar, mover ni editar el archivo de origen.
#[tauri::command]
fn set_cover(state: State<'_, AppData>, manga_id: i64, path: String) -> Result<(), String> {
    let db = connect(&state.database)?;
    let manga_path: String = db
        .query_row("SELECT path FROM manga WHERE id=?1", [manga_id], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    let selected = PathBuf::from(&path);
    // La portada personalizada debe ser una imagen perteneciente a la propia obra.
    if !selected.starts_with(Path::new(&manga_path))
        || !selected.is_file()
        || !image_file(&selected)
    {
        return Err(
            "El archivo elegido no pertenece a esta obra o no es una imagen compatible".into(),
        );
    }
    db.execute(
        "UPDATE manga SET cover_path=?1,custom_cover=1 WHERE id=?2",
        params![selected.to_string_lossy(), manga_id],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// Guarda página (índice base cero) y fecha de última lectura.
#[tauri::command]
fn save_progress(state: State<'_, AppData>, manga_id: i64, page: i64) -> Result<(), String> {
    let db = connect(&state.database)?;
    db.execute(
        "UPDATE manga SET last_page=?1,last_read=strftime('%s','now') WHERE id=?2",
        params![page.max(0), manga_id],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// Persiste tema, dirección, zoom predeterminado y demás preferencias locales.
#[tauri::command]
fn set_preference(state: State<'_, AppData>, key: String, value: String) -> Result<(), String> {
    let db = connect(&state.database)?;
    set_setting(&db, &key, &value)
}

fn get_thumbnail_blocking(state: AppData, path: String) -> Result<String, String> {
    let _guard = state.image_gate.lock().map_err(|error| error.to_string())?;
    let source = PathBuf::from(&path);
    if !source.is_file() || !image_file(&source) {
        return Err("La portada ya no está disponible".into());
    }
    let metadata = fs::metadata(&source).map_err(|error| error.to_string())?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    // El hash de ruta/tamaño/mtime invalida la miniatura cuando cambia el original,
    // sin escribir nunca sobre ese original.
    let signature = format!(
        "{}:{}:{}",
        source.to_string_lossy(),
        metadata.len(),
        modified
    );
    let key = format!("{:x}", Sha256::digest(signature.as_bytes()));
    let destination = state.thumbnails.join(format!("{key}.jpg"));
    if !destination.exists() {
        let image = ImageReader::open(&source)
            .map_err(|error| error.to_string())?
            .with_guessed_format()
            .map_err(|error| error.to_string())?
            .decode()
            .map_err(|error| error.to_string())?;
        let thumbnail = image.thumbnail(420, 620).to_rgb8();
        DynamicImage::ImageRgb8(thumbnail)
            .save_with_format(&destination, ImageFormat::Jpeg)
            .map_err(|error| error.to_string())?;
    }
    Ok(destination.to_string_lossy().to_string())
}

/// Decodifica miniaturas fuera del hilo del WebView para evitar bloqueos de interfaz.
#[tauri::command]
async fn get_thumbnail(state: State<'_, AppData>, path: String) -> Result<String, String> {
    let app_data = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || get_thumbnail_blocking(app_data, path))
        .await
        .map_err(|error| error.to_string())?
}

fn get_reader_image_blocking(state: AppData, path: String) -> Result<String, String> {
    let _guard = state.image_gate.lock().map_err(|error| error.to_string())?;
    let source = PathBuf::from(&path);
    if !source.is_file() || !image_file(&source) {
        return Err("La página ya no está disponible".into());
    }
    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    // GIF se convierte a un fotograma estático y TIFF a PNG porque WebView no admite
    // uniformemente estos formatos como páginas directas.
    if !matches!(extension.as_str(), "gif" | "tif" | "tiff") {
        return Ok(source.to_string_lossy().to_string());
    }
    let metadata = fs::metadata(&source).map_err(|error| error.to_string())?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let signature = format!(
        "{}:{}:{}",
        source.to_string_lossy(),
        metadata.len(),
        modified
    );
    let key = format!("{:x}", Sha256::digest(signature.as_bytes()));
    let destination = state.readers.join(format!("{key}.png"));
    if !destination.exists() {
        let image = ImageReader::open(&source)
            .map_err(|error| error.to_string())?
            .with_guessed_format()
            .map_err(|error| error.to_string())?
            .decode()
            .map_err(|error| error.to_string())?;
        image
            .save_with_format(&destination, ImageFormat::Png)
            .map_err(|error| error.to_string())?;
    }
    Ok(destination.to_string_lossy().to_string())
}

/// Borra derivados guardados en AppData; nunca toca páginas ni portadas originales.
#[tauri::command]
fn clear_thumbnail_cache(state: State<'_, AppData>) -> Result<usize, String> {
    let entries = fs::read_dir(&state.thumbnails).map_err(|error| error.to_string())?;
    let mut removed = 0;
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_file() {
            fs::remove_file(path).map_err(|error| error.to_string())?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Adapta una sola página visible fuera del hilo UI cuando el WebView no admite el formato.
#[tauri::command]
async fn get_reader_image(state: State<'_, AppData>, path: String) -> Result<String, String> {
    let app_data = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || get_reader_image_blocking(app_data, path))
        .await
        .map_err(|error| error.to_string())?
}

/// Acciones nativas del título personalizado de la ventana.
#[tauri::command]
fn minimize_window(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("main")
        .ok_or("No se encontró la ventana")?
        .minimize()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn toggle_maximize(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("No se encontró la ventana")?;
    if window.is_maximized().map_err(|error| error.to_string())? {
        window.unmaximize().map_err(|error| error.to_string())
    } else {
        window.maximize().map_err(|error| error.to_string())
    }
}

#[tauri::command]
fn close_window(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("main")
        .ok_or("No se encontró la ventana")?
        .close()
        .map_err(|error| error.to_string())
}

/// Configura la ventana, el diálogo nativo y las rutas autorizadas para mostrar imágenes.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Keep development scans, test libraries and thumbnails out of the release
            // profile. Existing data is left untouched; each channel gets its own SQLite.
            let channel = if cfg!(debug_assertions) {
                "development"
            } else {
                "release"
            };
            let data_dir = app.path().app_data_dir()?.join(channel);
            let thumbnails = data_dir.join("thumbnails");
            let readers = data_dir.join("reader-cache");
            fs::create_dir_all(&thumbnails)?;
            fs::create_dir_all(&readers)?;
            let database = data_dir.join("soft-explorer.sqlite3");
            initialize_database(&database).map_err(std::io::Error::other)?;
            let db = connect(&database).map_err(std::io::Error::other)?;
            if let Some(library) = setting(&db, "library").map_err(std::io::Error::other)? {
                let path = PathBuf::from(library);
                if path.is_dir() {
                    app.asset_protocol_scope().allow_directory(&path, true)?;
                }
            }
            app.manage(AppData {
                database,
                thumbnails,
                readers,
                image_gate: Arc::new(Mutex::new(())),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_initial_state,
            set_library_path,
            scan_library,
            list_manga,
            get_pages,
            toggle_favorite,
            set_cover,
            save_progress,
            set_preference,
            get_thumbnail,
            get_reader_image,
            clear_thumbnail_cache,
            minimize_window,
            toggle_maximize,
            close_window
        ])
        .run(tauri::generate_context!())
        .expect("No se pudo iniciar Soft Explorer");
}
