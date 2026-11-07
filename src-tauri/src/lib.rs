//! Backend de Soft Explorer: catálogo SQLite, indexado incremental, cachés de imagen
//! fuera de la biblioteca y comandos Tauri invocados por la interfaz React.

use image::{DynamicImage, ImageFormat, ImageReader};
use natord::compare as natural_compare;
use rars::{ArchiveReadOptions, ArchiveReader};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};
use tauri::{AppHandle, Emitter, Manager, State};
use zip::ZipArchive;

const MAX_ARCHIVE_PAGE_BYTES: u64 = 256 * 1024 * 1024;

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
    chapter_count: i64,
    is_series: bool,
    cover_path: String,
    cover_entry: Option<String>,
    is_archive: bool,
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

/// Una página puede ser un archivo normal o una entrada contenida en CBZ/CBR.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PageSource {
    path: String,
    entry: Option<String>,
    label: String,
}

/// Capítulo individual; su progreso se guarda aparte del progreso de la obra completa.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Chapter {
    id: i64,
    manga_id: i64,
    path: String,
    kind: String,
    title: String,
    page_count: i64,
    cover_path: String,
    cover_entry: Option<String>,
    last_page: i64,
    last_read: i64,
}

#[derive(Clone)]
struct CachedManga {
    pages: i64,
    chapter_count: i64,
    is_series: bool,
    cover: String,
    cover_entry: Option<String>,
    folder_mtime: i64,
}

struct ScannedManga {
    path: String,
    title: String,
    genre: String,
    page_count: i64,
    chapter_count: i64,
    is_series: bool,
    cover_path: String,
    cover_entry: Option<String>,
    folder_mtime: i64,
    unchanged: bool,
    chapters: Vec<ScannedChapter>,
}

struct ScannedChapter {
    path: String,
    kind: String,
    title: String,
    page_count: i64,
    cover_path: String,
    cover_entry: Option<String>,
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
            chapter_count INTEGER NOT NULL DEFAULT 0,
            is_series INTEGER NOT NULL DEFAULT 0,
            cover_path TEXT NOT NULL DEFAULT '',
            cover_entry TEXT NOT NULL DEFAULT '',
            folder_mtime INTEGER NOT NULL DEFAULT 0,
            custom_cover INTEGER NOT NULL DEFAULT 0,
            favorite INTEGER NOT NULL DEFAULT 0,
            last_page INTEGER NOT NULL DEFAULT 0,
            last_read INTEGER NOT NULL DEFAULT 0,
            added_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
        );
        CREATE TABLE IF NOT EXISTS chapters(
            id INTEGER PRIMARY KEY,
            manga_id INTEGER NOT NULL REFERENCES manga(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            kind TEXT NOT NULL,
            title TEXT NOT NULL,
            page_count INTEGER NOT NULL DEFAULT 0,
            cover_path TEXT NOT NULL DEFAULT '',
            cover_entry TEXT NOT NULL DEFAULT '',
            folder_mtime INTEGER NOT NULL DEFAULT 0,
            last_page INTEGER NOT NULL DEFAULT 0,
            last_read INTEGER NOT NULL DEFAULT 0,
            UNIQUE(manga_id,path,kind)
        );
        CREATE INDEX IF NOT EXISTS manga_title_idx ON manga(title COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS manga_genre_idx ON manga(genre COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS manga_favorite_idx ON manga(favorite);
        CREATE INDEX IF NOT EXISTS manga_last_read_idx ON manga(last_read);
        CREATE INDEX IF NOT EXISTS chapters_manga_idx ON chapters(manga_id);
        CREATE TEMP TABLE IF NOT EXISTS scan_keep(path TEXT PRIMARY KEY);
        CREATE TEMP TABLE IF NOT EXISTS scan_keep_chapters(
            manga_id INTEGER NOT NULL,
            path TEXT NOT NULL,
            kind TEXT NOT NULL,
            PRIMARY KEY(manga_id,path,kind)
        );",
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
    if !names.iter().any(|name| name == "cover_entry") {
        db.execute(
            "ALTER TABLE manga ADD COLUMN cover_entry TEXT NOT NULL DEFAULT ''",
            [],
        )
        .map_err(|error| error.to_string())?;
    }
    if !names.iter().any(|name| name == "chapter_count") {
        db.execute(
            "ALTER TABLE manga ADD COLUMN chapter_count INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|error| error.to_string())?;
    }
    if !names.iter().any(|name| name == "is_series") {
        db.execute(
            "ALTER TABLE manga ADD COLUMN is_series INTEGER NOT NULL DEFAULT 0",
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
    let path: String = row.get(1)?;
    let cover_entry: String = row.get(8)?;
    Ok(Manga {
        id: row.get(0)?,
        path: path.clone(),
        title: row.get(2)?,
        genre: row.get(3)?,
        page_count: row.get(4)?,
        chapter_count: row.get(5)?,
        is_series: row.get::<_, i64>(6)? != 0,
        cover_path: row.get(7)?,
        cover_entry: (!cover_entry.is_empty()).then_some(cover_entry),
        is_archive: is_archive_file(Path::new(&path)),
        favorite: row.get::<_, i64>(9)? != 0,
        custom_cover: row.get::<_, i64>(10)? != 0,
        last_page: row.get(11)?,
        last_read: row.get(12)?,
        added_at: row.get(13)?,
    })
}

/// Lee solo metadatos; nunca carga páginas ni portadas a memoria con el catálogo.
fn all_manga(db: &Connection) -> Result<Vec<Manga>, String> {
    let mut statement = db
        .prepare(
            "SELECT id,path,title,genre,page_count,chapter_count,is_series,cover_path,cover_entry,
                    favorite,custom_cover,last_page,last_read,added_at FROM manga",
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

fn is_archive_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "cbz" | "cbr"
    )
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

fn visible_archive_image(name: &str) -> bool {
    let normalized = name.replace('\\', "/");
    !normalized
        .split('/')
        .any(|part| part.starts_with('.') || part.to_ascii_lowercase().starts_with("thumb_"))
        && image_file(Path::new(&normalized))
}

/// Indexa únicamente nombres y metadatos del contenedor. El contenido comprimido se abre
/// cuando el usuario muestra una portada o una página, no durante el escaneo general.
fn archive_pages(path: &Path) -> Result<Vec<PageSource>, String> {
    let archive_path = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string();
    let mut pages = Vec::new();
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "cbz" => {
            let file = fs::File::open(path).map_err(|error| error.to_string())?;
            let mut archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
            for index in 0..archive.len() {
                let entry = archive.by_index(index).map_err(|error| error.to_string())?;
                let name = entry.name().replace('\\', "/");
                if !entry.is_dir()
                    && entry.size() <= MAX_ARCHIVE_PAGE_BYTES
                    && visible_archive_image(&name)
                {
                    pages.push(PageSource {
                        path: archive_path.clone(),
                        entry: Some(format!("zip:{index}")),
                        label: name,
                    });
                }
            }
        }
        "cbr" => {
            let archive = ArchiveReader::read_path(path).map_err(|error| error.to_string())?;
            for (index, member) in archive.members().enumerate() {
                let name = member.meta.name_lossy().replace('\\', "/");
                if !member.meta.is_directory
                    && !member.meta.is_redirection
                    && member.meta.unpacked_size <= MAX_ARCHIVE_PAGE_BYTES
                    && visible_archive_image(&name)
                {
                    pages.push(PageSource {
                        path: archive_path.clone(),
                        entry: Some(format!("rar:{index}")),
                        label: name,
                    });
                }
            }
        }
        _ => return Err("El contenedor no es CBZ ni CBR".into()),
    }
    pages.sort_by(|left, right| natural_compare(&left.label, &right.label));
    Ok(pages)
}

fn direct_image_pages(folder: &Path) -> Result<Vec<PageSource>, String> {
    let entries = fs::read_dir(folder).map_err(|error| error.to_string())?;
    let mut pages = Vec::new();
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_file() && !is_hidden(&path) && image_file(&path) {
            let label = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            pages.push((
                label.clone(),
                PageSource {
                    path: path
                        .canonicalize()
                        .unwrap_or(path)
                        .to_string_lossy()
                        .to_string(),
                    entry: None,
                    label,
                },
            ));
        }
    }
    pages.sort_by(|left, right| natural_compare(&left.0, &right.0));
    Ok(pages.into_iter().map(|(_, page)| page).collect())
}

/// Obra autocontenida: combina páginas sueltas y volúmenes archivados bajo una carpeta.
fn work_pages(work: &Path) -> Result<Vec<PageSource>, String> {
    if work.is_file() {
        return archive_pages(work).or_else(|_| Ok(Vec::new()));
    }

    let entries = fs::read_dir(work).map_err(|error| error.to_string())?;
    let mut pages: Vec<(String, PageSource)> = direct_image_pages(work)?
        .into_iter()
        .map(|page| (page.label.clone(), page))
        .collect();
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if is_hidden(&path) || !path.is_file() || !is_archive_file(&path) {
            continue;
        }
        // Un volumen corrupto no impide leer páginas sueltas ni los otros volúmenes.
        if let Ok(archive_pages) = archive_pages(&path) {
            let volume = path.file_stem().unwrap_or_default().to_string_lossy();
            for mut page in archive_pages {
                page.label = format!("{volume} / {}", page.label);
                pages.push((page.label.clone(), page));
            }
        }
    }
    pages.sort_by(|left, right| natural_compare(&left.0, &right.0));
    Ok(pages.into_iter().map(|(_, page)| page).collect())
}

fn scanned_chapter(
    path: &Path,
    kind: &str,
    title: String,
    pages: Vec<PageSource>,
) -> Option<ScannedChapter> {
    if pages.is_empty() {
        return None;
    }
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    Some(ScannedChapter {
        path: path.to_string_lossy().to_string(),
        kind: kind.to_string(),
        title,
        page_count: pages.len() as i64,
        cover_path: pages[0].path.clone(),
        cover_entry: pages[0].entry.clone(),
        folder_mtime: work_signature(&path).unwrap_or_default(),
    })
}

/// Una carpeta con subcarpetas o más de un archivo CBR/CBZ pasa a ser una serie.
fn scan_work(work: &Path) -> Result<(bool, Vec<ScannedChapter>, Vec<PageSource>), String> {
    if work.is_file() {
        return Ok((false, Vec::new(), archive_pages(work).unwrap_or_default()));
    }

    let entries = fs::read_dir(work).map_err(|error| error.to_string())?;
    let mut subfolders = Vec::new();
    let mut archives = Vec::new();
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if is_hidden(&path) {
            continue;
        }
        if path.is_dir() {
            subfolders.push(path);
        } else if path.is_file() && is_archive_file(&path) {
            archives.push(path);
        }
    }
    subfolders = natural_entries(subfolders);
    archives = natural_entries(archives);
    let is_series = !subfolders.is_empty() || archives.len() > 1;
    if !is_series {
        return Ok((false, Vec::new(), work_pages(work)?));
    }

    let mut chapters = Vec::new();
    for folder in subfolders {
        let pages = work_pages(&folder)?;
        if let Some(chapter) = scanned_chapter(&folder, "folder", work_title(&folder), pages) {
            chapters.push(chapter);
        }
    }
    for archive in archives {
        let pages = archive_pages(&archive).unwrap_or_default();
        if let Some(chapter) = scanned_chapter(&archive, "archive", work_title(&archive), pages) {
            chapters.push(chapter);
        }
    }
    if let Some(chapter) = scanned_chapter(
        work,
        "loose",
        "Páginas sueltas".to_string(),
        direct_image_pages(work)?,
    ) {
        chapters.push(chapter);
    }
    chapters.sort_by(|left, right| natural_compare(&left.title, &right.title));

    let pages = chapters
        .iter()
        .flat_map(|chapter| {
            chapter_pages(Path::new(&chapter.path), &chapter.kind)
                .unwrap_or_default()
                .into_iter()
                .map(|mut page| {
                    page.label = format!("{} / {}", chapter.title, page.label);
                    page
                })
        })
        .collect();
    Ok((true, chapters, pages))
}

fn chapter_pages(path: &Path, kind: &str) -> Result<Vec<PageSource>, String> {
    match kind {
        "archive" => archive_pages(path),
        "loose" => direct_image_pages(path),
        _ => work_pages(path),
    }
}

/// Obtiene los bytes de una única entrada. El límite evita que un archivo comprimido
/// malformado reserve cantidades arbitrarias de memoria; el resultado solo va a AppData.
fn read_archive_entry(path: &Path, entry: &str) -> Result<Vec<u8>, String> {
    let (kind, index) = entry
        .split_once(':')
        .ok_or("Referencia de página de archivo inválida")?;
    let index = index.parse::<usize>().map_err(|error| error.to_string())?;
    match kind {
        "zip" => {
            let file = fs::File::open(path).map_err(|error| error.to_string())?;
            let mut archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
            let mut member = archive.by_index(index).map_err(|error| error.to_string())?;
            if member.size() > MAX_ARCHIVE_PAGE_BYTES {
                return Err("La página del archivo supera el tamaño máximo permitido".into());
            }
            let mut bytes = Vec::with_capacity(member.size() as usize);
            member
                .by_ref()
                .take(MAX_ARCHIVE_PAGE_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            if bytes.len() as u64 > MAX_ARCHIVE_PAGE_BYTES {
                return Err("La página del archivo supera el tamaño máximo permitido".into());
            }
            Ok(bytes)
        }
        "rar" => {
            let archive = ArchiveReader::read_path(path).map_err(|error| error.to_string())?;
            let options = ArchiveReadOptions::new()
                .with_max_member_output_bytes(MAX_ARCHIVE_PAGE_BYTES)
                .with_max_total_output_bytes(1024 * 1024 * 1024)
                .with_max_reader_workspace_bytes(512 * 1024 * 1024);
            archive
                .read_member_at_with_options(index, options)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "No se encontró la página dentro del CBR".to_string())
        }
        _ => Err("Tipo de entrada de archivo no reconocido".into()),
    }
}

fn source_signature(path: &Path, entry: Option<&str>) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    Ok(format!(
        "{}:{}:{}:{}",
        path.to_string_lossy(),
        metadata.len(),
        modified,
        entry.unwrap_or_default()
    ))
}

fn decode_page(path: &Path, entry: Option<&str>) -> Result<DynamicImage, String> {
    if let Some(entry) = entry {
        let bytes = read_archive_entry(path, entry)?;
        return ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|error| error.to_string())?
            .decode()
            .map_err(|error| error.to_string());
    }
    ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|error| error.to_string())
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

fn work_title(path: &Path) -> String {
    let name = if is_archive_file(path) {
        path.file_stem().unwrap_or_default()
    } else {
        path.file_name().unwrap_or_default()
    };
    name.to_string_lossy().to_string()
}

/// Firma estable del contenido que afecta al índice. En carpetas se revisa el mtime del
/// directorio y solo los metadatos de archivos de imagen/archivo, no se decodifican páginas.
fn work_signature(work: &Path) -> Result<i64, String> {
    let mut digest = Sha256::new();
    digest.update(file_time_ns(work).to_le_bytes());
    if work.is_file() {
        let metadata = fs::metadata(work).map_err(|error| error.to_string())?;
        digest.update(metadata.len().to_le_bytes());
        digest.update(
            work.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .as_bytes(),
        );
    } else {
        let mut inputs = fs::read_dir(work)
            .map_err(|error| error.to_string())?
            .map(|entry| {
                entry
                    .map(|entry| entry.path())
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        inputs.retain(|path| {
            !is_hidden(path)
                && (path.is_dir()
                    || (path.is_file() && (image_file(path) || is_archive_file(path))))
        });
        inputs = natural_entries(inputs);
        for path in inputs {
            let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
            digest.update(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_bytes(),
            );
            digest.update(metadata.len().to_le_bytes());
            digest.update(file_time_ns(&path).to_le_bytes());
        }
    }
    let hash = digest.finalize();
    Ok(i64::from_le_bytes(hash[..8].try_into().unwrap_or_default()) & i64::MAX)
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
            .prepare("SELECT path,page_count,chapter_count,is_series,cover_path,cover_entry,folder_mtime FROM manga")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                let cover_entry: String = row.get(5)?;
                Ok((
                    row.get::<_, String>(0)?,
                    CachedManga {
                        pages: row.get(1)?,
                        chapter_count: row.get(2)?,
                        is_series: row.get::<_, i64>(3)? != 0,
                        cover: row.get(4)?,
                        cover_entry: (!cover_entry.is_empty()).then_some(cover_entry),
                        folder_mtime: row.get(6)?,
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
            if !is_hidden(&path) && (path.is_dir() || (path.is_file() && is_archive_file(&path))) {
                manga_dirs.push((path, genre_name.clone()));
            }
        }
    }
    manga_dirs.sort_by(|left, right| {
        let genre_order = natural_compare(&left.1, &right.1);
        if genre_order.is_eq() {
            natural_compare(&work_title(&left.0), &work_title(&right.0))
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
        let folder_mtime = work_signature(folder)?;
        let title = work_title(folder);
        let record = if let Some(old) = cached
            .get(&path_text)
            .filter(|old| old.folder_mtime == folder_mtime && old.pages > 0)
        {
            ScannedManga {
                path: path_text,
                title,
                genre: genre.clone(),
                page_count: old.pages,
                chapter_count: old.chapter_count,
                is_series: old.is_series,
                cover_path: old.cover.clone(),
                cover_entry: old.cover_entry.clone(),
                folder_mtime,
                unchanged: true,
                chapters: Vec::new(),
            }
        } else {
            let (is_series, chapters, pages) = match scan_work(folder) {
                Ok(result) => result,
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
                chapter_count: chapters.len() as i64,
                is_series,
                cover_path: pages[0].path.clone(),
                cover_entry: pages[0].entry.clone(),
                folder_mtime,
                unchanged: false,
                chapters,
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
    db.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS scan_keep_chapters(
            manga_id INTEGER NOT NULL, path TEXT NOT NULL, kind TEXT NOT NULL,
            PRIMARY KEY(manga_id,path,kind)
        )",
    )
    .map_err(|error| error.to_string())?;
    let transaction = db
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM scan_keep", [])
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM scan_keep_chapters", [])
        .map_err(|error| error.to_string())?;
    {
        let mut upsert = transaction
            .prepare(
                "INSERT INTO manga(path,title,genre,page_count,chapter_count,is_series,
                    cover_path,cover_entry,folder_mtime)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(path) DO UPDATE SET title=excluded.title,genre=excluded.genre,
                    page_count=excluded.page_count,chapter_count=excluded.chapter_count,
                    is_series=excluded.is_series,folder_mtime=excluded.folder_mtime,
                    cover_path=CASE WHEN manga.custom_cover=0 THEN excluded.cover_path ELSE manga.cover_path END,
                    cover_entry=CASE WHEN manga.custom_cover=0 THEN excluded.cover_entry ELSE manga.cover_entry END",
            )
            .map_err(|error| error.to_string())?;
        let mut chapter_upsert = transaction
            .prepare(
                "INSERT INTO chapters(manga_id,path,kind,title,page_count,cover_path,cover_entry,folder_mtime)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(manga_id,path,kind) DO UPDATE SET title=excluded.title,
                    page_count=excluded.page_count,cover_path=excluded.cover_path,
                    cover_entry=excluded.cover_entry,folder_mtime=excluded.folder_mtime",
            )
            .map_err(|error| error.to_string())?;
        for manga in &scanned {
            upsert
                .execute(params![
                    manga.path,
                    manga.title,
                    manga.genre,
                    manga.page_count,
                    manga.chapter_count,
                    if manga.is_series { 1_i64 } else { 0_i64 },
                    manga.cover_path,
                    manga.cover_entry.as_deref().unwrap_or_default(),
                    manga.folder_mtime
                ])
                .map_err(|error| error.to_string())?;
            let manga_id: i64 = transaction
                .query_row("SELECT id FROM manga WHERE path=?1", [&manga.path], |row| {
                    row.get(0)
                })
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO scan_keep(path) VALUES(?1)",
                    [&manga.path],
                )
                .map_err(|error| error.to_string())?;
            if manga.unchanged {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO scan_keep_chapters(manga_id,path,kind)
                         SELECT manga_id,path,kind FROM chapters WHERE manga_id=?1",
                        [manga_id],
                    )
                    .map_err(|error| error.to_string())?;
                continue;
            }
            for chapter in &manga.chapters {
                chapter_upsert
                    .execute(params![
                        manga_id,
                        chapter.path,
                        chapter.kind,
                        chapter.title,
                        chapter.page_count,
                        chapter.cover_path,
                        chapter.cover_entry.as_deref().unwrap_or_default(),
                        chapter.folder_mtime
                    ])
                    .map_err(|error| error.to_string())?;
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO scan_keep_chapters(manga_id,path,kind)
                         VALUES(?1,?2,?3)",
                        params![manga_id, chapter.path, chapter.kind],
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    transaction
        .execute(
            "DELETE FROM chapters WHERE NOT EXISTS (
                SELECT 1 FROM scan_keep_chapters keep
                WHERE keep.manga_id=chapters.manga_id AND keep.path=chapters.path AND keep.kind=chapters.kind
            )",
            [],
        )
        .map_err(|error| error.to_string())?;
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
fn get_pages(state: State<'_, AppData>, manga_id: i64) -> Result<Vec<PageSource>, String> {
    let db = connect(&state.database)?;
    pages_for_manga(&db, manga_id)
}

fn read_chapter(row: &rusqlite::Row<'_>) -> rusqlite::Result<Chapter> {
    let cover_entry: String = row.get(7)?;
    Ok(Chapter {
        id: row.get(0)?,
        manga_id: row.get(1)?,
        path: row.get(2)?,
        kind: row.get(3)?,
        title: row.get(4)?,
        page_count: row.get(5)?,
        cover_path: row.get(6)?,
        cover_entry: (!cover_entry.is_empty()).then_some(cover_entry),
        last_page: row.get(8)?,
        last_read: row.get(9)?,
    })
}

fn manga_chapters(db: &Connection, manga_id: i64) -> Result<Vec<Chapter>, String> {
    let mut statement = db
        .prepare(
            "SELECT id,manga_id,path,kind,title,page_count,cover_path,cover_entry,last_page,last_read
             FROM chapters WHERE manga_id=?1",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([manga_id], read_chapter)
        .map_err(|error| error.to_string())?;
    let mut chapters = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    chapters.sort_by(|left, right| natural_compare(&left.title, &right.title));
    Ok(chapters)
}

fn pages_for_manga(db: &Connection, manga_id: i64) -> Result<Vec<PageSource>, String> {
    let (path, is_series): (String, bool) = db
        .query_row(
            "SELECT path,is_series FROM manga WHERE id=?1",
            [manga_id],
            |row| Ok((row.get(0)?, row.get::<_, i64>(1)? != 0)),
        )
        .map_err(|error| error.to_string())?;
    if !is_series {
        return work_pages(Path::new(&path));
    }

    let mut pages = Vec::new();
    for chapter in manga_chapters(db, manga_id)? {
        for mut page in chapter_pages(Path::new(&chapter.path), &chapter.kind)? {
            page.label = format!("{} / {}", chapter.title, page.label);
            pages.push(page);
        }
    }
    Ok(pages)
}

#[tauri::command]
fn list_chapters(state: State<'_, AppData>, manga_id: i64) -> Result<Vec<Chapter>, String> {
    let db = connect(&state.database)?;
    manga_chapters(&db, manga_id)
}

#[tauri::command]
fn get_chapter_pages(
    state: State<'_, AppData>,
    chapter_id: i64,
) -> Result<Vec<PageSource>, String> {
    let db = connect(&state.database)?;
    let chapter = db
        .query_row(
            "SELECT id,manga_id,path,kind,title,page_count,cover_path,cover_entry,last_page,last_read
             FROM chapters WHERE id=?1",
            [chapter_id],
            read_chapter,
        )
        .map_err(|error| error.to_string())?;
    chapter_pages(Path::new(&chapter.path), &chapter.kind)
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
fn set_cover(
    state: State<'_, AppData>,
    manga_id: i64,
    path: String,
    entry: Option<String>,
) -> Result<(), String> {
    let db = connect(&state.database)?;
    let selected = PathBuf::from(&path);
    // La UI solo puede elegir páginas que el backend ya enumeró dentro de esa obra.
    let is_member_page = pages_for_manga(&db, manga_id)?
        .iter()
        .any(|page| page.path == path && page.entry == entry);
    if !is_member_page {
        return Err(
            "El archivo elegido no pertenece a esta obra o no es una imagen compatible".into(),
        );
    }
    db.execute(
        "UPDATE manga SET cover_path=?1,cover_entry=?2,custom_cover=1 WHERE id=?3",
        params![
            selected.to_string_lossy(),
            entry.unwrap_or_default(),
            manga_id
        ],
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

/// Persiste progreso por capítulo y actualiza el resumen de lectura de la serie.
#[tauri::command]
fn save_chapter_progress(
    state: State<'_, AppData>,
    chapter_id: i64,
    page: i64,
) -> Result<(), String> {
    let db = connect(&state.database)?;
    let manga_id: i64 = db
        .query_row(
            "SELECT manga_id FROM chapters WHERE id=?1",
            [chapter_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    db.execute(
        "UPDATE chapters SET last_page=?1,last_read=strftime('%s','now') WHERE id=?2",
        params![page.max(0), chapter_id],
    )
    .map_err(|error| error.to_string())?;
    let pages_read: i64 = db
        .query_row(
            "SELECT COALESCE(SUM(last_page+1),0) FROM chapters
             WHERE manga_id=?1 AND last_read>0",
            [manga_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    db.execute(
        "UPDATE manga SET last_page=?1,last_read=strftime('%s','now') WHERE id=?2",
        params![pages_read.saturating_sub(1).max(0), manga_id],
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

fn get_thumbnail_blocking(
    state: AppData,
    path: String,
    entry: Option<String>,
) -> Result<String, String> {
    let _guard = state.image_gate.lock().map_err(|error| error.to_string())?;
    let source = PathBuf::from(&path);
    let valid_source = source.is_file()
        && if entry.is_some() {
            is_archive_file(&source)
        } else {
            image_file(&source)
        };
    if !valid_source {
        return Err("La portada ya no está disponible".into());
    }
    // El hash de ruta/tamaño/mtime invalida la miniatura cuando cambia el original,
    // e incluye la entrada para distinguir páginas que comparten el mismo archivo CBZ/CBR.
    let signature = source_signature(&source, entry.as_deref())?;
    let key = format!("{:x}", Sha256::digest(signature.as_bytes()));
    let destination = state.thumbnails.join(format!("{key}.jpg"));
    if !destination.exists() {
        let image = decode_page(&source, entry.as_deref())?;
        let thumbnail = image.thumbnail(420, 620).to_rgb8();
        DynamicImage::ImageRgb8(thumbnail)
            .save_with_format(&destination, ImageFormat::Jpeg)
            .map_err(|error| error.to_string())?;
    }
    Ok(destination.to_string_lossy().to_string())
}

/// Decodifica miniaturas fuera del hilo del WebView para evitar bloqueos de interfaz.
#[tauri::command]
async fn get_thumbnail(
    state: State<'_, AppData>,
    path: String,
    entry: Option<String>,
) -> Result<String, String> {
    let app_data = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || get_thumbnail_blocking(app_data, path, entry))
        .await
        .map_err(|error| error.to_string())?
}

fn get_reader_image_blocking(
    state: AppData,
    path: String,
    entry: Option<String>,
    label: String,
) -> Result<String, String> {
    let _guard = state.image_gate.lock().map_err(|error| error.to_string())?;
    let source = PathBuf::from(&path);
    let source_is_valid = source.is_file()
        && if entry.is_some() {
            is_archive_file(&source)
        } else {
            image_file(&source)
        };
    if !source_is_valid {
        return Err("La página ya no está disponible".into());
    }
    let extension_source = if label.is_empty() { &path } else { &label };
    let extension = Path::new(extension_source)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    // GIF se convierte a un fotograma estático y TIFF a PNG porque WebView no admite
    // uniformemente estos formatos como páginas directas.
    let needs_static_conversion = matches!(extension.as_str(), "gif" | "tif" | "tiff");
    if entry.is_none() && !needs_static_conversion {
        return Ok(source.to_string_lossy().to_string());
    }
    let signature = source_signature(&source, entry.as_deref())?;
    let key = format!("{:x}", Sha256::digest(signature.as_bytes()));
    let output_extension = if needs_static_conversion {
        "png"
    } else {
        extension.as_str()
    };
    let destination = state.readers.join(format!("{key}.{output_extension}"));
    if !destination.exists() {
        if entry.is_some() && !needs_static_conversion {
            let bytes = read_archive_entry(&source, entry.as_deref().unwrap())?;
            fs::write(&destination, bytes).map_err(|error| error.to_string())?;
        } else {
            let image = decode_page(&source, entry.as_deref())?;
            image
                .save_with_format(&destination, ImageFormat::Png)
                .map_err(|error| error.to_string())?;
        }
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
async fn get_reader_image(
    state: State<'_, AppData>,
    path: String,
    entry: Option<String>,
    label: String,
) -> Result<String, String> {
    let app_data = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        get_reader_image_blocking(app_data, path, entry, label)
    })
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
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Desarrollo y versión instalada usan bases de datos separadas. Los datos
            // existentes se conservan al iniciar cualquiera de las dos versiones.
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
            list_chapters,
            get_chapter_pages,
            toggle_favorite,
            set_cover,
            save_progress,
            save_chapter_progress,
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};
    use std::{io::Write, time::SystemTime};
    use zip::{write::SimpleFileOptions, ZipWriter};

    struct TempFolder(PathBuf);

    impl Drop for TempFolder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample_cbz() -> (TempFolder, PathBuf, Vec<u8>) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("soft-explorer-archive-test-{unique}"));
        fs::create_dir_all(&root).unwrap();
        let temp = TempFolder(root.clone());
        let path = root.join("Volume.cbz");
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::from_pixel(2, 3, Rgb([242, 109, 79])))
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let bytes = png.into_inner();
        let mut archive = ZipWriter::new(fs::File::create(&path).unwrap());
        archive
            .start_file("10.png", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&bytes).unwrap();
        archive
            .start_file("2.png", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&bytes).unwrap();
        archive
            .start_file(".hidden/secret.jpg", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&bytes).unwrap();
        archive.finish().unwrap();
        (temp, path, bytes)
    }

    #[test]
    fn cbz_pages_are_naturally_sorted_and_hidden_entries_are_skipped() {
        let (_temp, path, _) = sample_cbz();
        let pages = archive_pages(&path).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].label, "2.png");
        assert_eq!(pages[1].label, "10.png");
    }

    #[test]
    fn cbz_entry_can_be_read_without_extracting_into_the_archive_directory() {
        let (temp, path, expected) = sample_cbz();
        let pages = archive_pages(&path).unwrap();
        let bytes = read_archive_entry(
            Path::new(&pages[0].path),
            pages[0].entry.as_deref().unwrap(),
        )
        .unwrap();
        assert_eq!(bytes, expected);
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
    }
}
