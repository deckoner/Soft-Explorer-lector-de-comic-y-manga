import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  BookOpen,
  Check,
  ChevronDown,
  ChevronRight,
  Compass,
  FolderOpen,
  Heart,
  Home,
  LibraryBig,
  ListFilter,
  Maximize2,
  Minus,
  PanelLeftClose,
  Search,
  Settings2,
  SlidersHorizontal,
  Sun,
  X,
} from "lucide-react";
import type { InitialState, Manga, Route, ScanProgress, ScanSummary } from "./types";
import { errorMessage, formatCount } from "./format";
import LibraryGrid from "./LibraryGrid";
import Reader from "./Reader";

type Dialog = "settings" | "cover" | "menu" | null;

// These defaults are mirrored in the local SQLite preferences store.
const DEFAULT_SETTINGS: Record<string, string> = {
  theme: "light",
  direction: "western",
  fit: "screen",
  doublePage: "false",
  autoSeconds: "5",
  coverSize: "190",
  nextKey: "PageDown",
  previousKey: "PageUp",
};

/** Ventana principal: navegación, filtros, preferencias y conexión con comandos Tauri. */
export default function App() {
  const [state, setState] = useState<InitialState | null>(null);
  const [mangas, setMangas] = useState<Manga[]>([]);
  const [route, setRoute] = useState<Route>("Inicio");
  const [genre, setGenre] = useState("");
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState("Nombre A-Z");
  const [coverSize, setCoverSize] = useState(190);
  const [compact, setCompact] = useState(false);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [selected, setSelected] = useState<Manga | null>(null);
  const [reading, setReading] = useState<Manga | null>(null);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [scanning, setScanning] = useState(false);
  const [toast, setToast] = useState("");
  const [settings, setSettings] = useState(DEFAULT_SETTINGS);
  // Start with a compact rail at medium widths and a closed drawer on small screens.
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => window.innerWidth < 1100);
  const searchInput = useRef<HTMLInputElement>(null);

  const notify = useCallback((message: string) => {
    setToast(message);
    window.setTimeout(() => setToast(""), 3400);
  }, []);

  const refreshManga = useCallback(
    async (overrides?: { genre?: string; route?: Route }) => {
      const activeRoute = overrides?.route ?? route;
      const activeGenre = overrides?.genre ?? genre;
      try {
        const data = await invoke<Manga[]>("list_manga", {
          query,
          genre: activeGenre || null,
          favoritesOnly: activeRoute === "Favoritos",
          sort,
        });
        setMangas(data);
      } catch (error) {
        notify(`No se pudo cargar la biblioteca: ${errorMessage(error)}`);
      }
    },
    [genre, notify, query, route, sort],
  );

  const runScan = useCallback(
    async (announce = true) => {
      // Rust performs the filesystem walk off the UI thread and streams progress events.
      setScanning(true);
      setProgress({ done: 0, total: 0, genres: 0, images: 0, current: "Preparando escaneo…" });
      try {
        const result = await invoke<ScanSummary>("scan_library");
        const fresh = await invoke<InitialState>("get_initial_state");
        setState(fresh);
        await refreshManga();
        if (announce) notify(`Biblioteca actualizada · ${formatCount(result.mangas)} mangas`);
      } catch (error) {
        notify(`No se pudo completar el escaneo: ${errorMessage(error)}`);
      } finally {
        setScanning(false);
        setProgress(null);
      }
    },
    [notify, refreshManga],
  );

  useEffect(() => {
    let alive = true;
    let unlisten: UnlistenFn | undefined;
    // A plain browser can preview the empty-state UI; native commands run only in Tauri.
    if (!isTauri()) {
      setState({ libraryPath: null, mangaCount: 0, genres: [], settings: DEFAULT_SETTINGS });
      setSettings(DEFAULT_SETTINGS);
      return () => {
        alive = false;
      };
    }
    listen<ScanProgress>("scan-progress", (event) => {
      if (alive) setProgress(event.payload);
    }).then((stop) => {
      unlisten = stop;
    });
    invoke<InitialState>("get_initial_state")
      .then(async (initial) => {
        if (!alive) return;
        const merged = { ...DEFAULT_SETTINGS, ...initial.settings };
        setSettings(merged);
        setCoverSize(Number(merged.coverSize) || 190);
        setState(initial);
        if (initial.libraryPath) {
          await invoke<Manga[]>("list_manga", {
            query: "",
            genre: null,
            favoritesOnly: false,
            sort: "Nombre A-Z",
          }).then((items) => alive && setMangas(items));
          await runScan(false);
        }
      })
      .catch((error) => notify(`No se pudo abrir Soft Explorer: ${errorMessage(error)}`));
    return () => {
      alive = false;
      unlisten?.();
    };
    // Initial load should only run once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!state) return;
    // Avoid a SQLite round-trip for every character typed into search.
    const handle = window.setTimeout(() => void refreshManga(), 110);
    return () => window.clearTimeout(handle);
  }, [state, route, genre, query, sort, refreshManga]);

  useEffect(() => {
    document.documentElement.dataset.theme = settings.theme === "dark" ? "dark" : "light";
  }, [settings.theme]);

  useEffect(() => {
    const focusSearch = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        searchInput.current?.focus();
      }
    };
    window.addEventListener("keydown", focusSearch);
    return () => window.removeEventListener("keydown", focusSearch);
  }, []);

  useEffect(() => {
    const compact = window.matchMedia("(max-width: 1100px)");
    // Keep the sidebar mode aligned with the actual CSS viewport, not physical pixels.
    const syncSidebar = (event: MediaQueryListEvent) => setSidebarCollapsed(event.matches);
    compact.addEventListener("change", syncSidebar);
    return () => compact.removeEventListener("change", syncSidebar);
  }, []);

  const chooseLibrary = useCallback(async () => {
    try {
      const folder = await open({
        directory: true,
        multiple: false,
        title: "Elige tu biblioteca de mangas",
      });
      if (typeof folder !== "string") return;
      await invoke("set_library_path", { path: folder });
      const fresh = await invoke<InitialState>("get_initial_state");
      setState(fresh);
      setGenre("");
      setRoute("Inicio");
      await runScan(true);
    } catch (error) {
      notify(`No se pudo abrir esa carpeta: ${errorMessage(error)}`);
    }
  }, [notify, runScan]);

  const changeRoute = (next: Route) => {
    setRoute(next);
    setGenre("");
  };

  const toggleFavorite = async (manga: Manga) => {
    try {
      await invoke("toggle_favorite", { mangaId: manga.id });
      setMangas((current) =>
        current.map((item) =>
          item.id === manga.id ? { ...item, favorite: !item.favorite } : item,
        ),
      );
      setSelected((current) =>
        current?.id === manga.id ? { ...current, favorite: !current.favorite } : current,
      );
    } catch (error) {
      notify(errorMessage(error));
    }
  };

  const saveSettings = async (next: Record<string, string>) => {
    try {
      await Promise.all(
        Object.entries(next).map(([key, value]) => invoke("set_preference", { key, value })),
      );
      setSettings(next);
      setCoverSize(Number(next.coverSize) || 190);
      setDialog(null);
      notify("Preferencias guardadas");
    } catch (error) {
      notify(`No se pudieron guardar las preferencias: ${errorMessage(error)}`);
    }
  };

  const heading =
    genre ||
    (route === "Favoritos"
      ? "Tus favoritos"
      : route === "Biblioteca"
        ? "Toda tu biblioteca"
        : "Explora tu colección");
  const currentCount = mangas.length;
  const hasLibrary = Boolean(state?.libraryPath);

  return (
    <div className={`app-shell ${sidebarCollapsed ? "sidebar-collapsed" : ""}`}>
      <header
        className="window-bar"
        onMouseDown={(event) => {
          const target = event.target as HTMLElement;
          if (event.button === 0 && isTauri() && !target.closest("button, input, select, a")) {
            void getCurrentWindow().startDragging();
          }
        }}
      >
        <div className="window-brand">
          <span className="brand-mark small">
            <BookOpen size={14} strokeWidth={2.4} />
          </span>
          <span>SOFT EXPLORER</span>
        </div>
        <div className="window-actions">
          <button
            className="window-action"
            aria-label="Minimizar"
            onClick={() => void invoke("minimize_window")}
          >
            <Minus size={14} />
          </button>
          <button
            className="window-action"
            aria-label="Maximizar"
            onClick={() => void invoke("toggle_maximize")}
          >
            <Maximize2 size={13} />
          </button>
          <button
            className="window-action close"
            aria-label="Cerrar"
            onClick={() => void invoke("close_window")}
          >
            <X size={14} />
          </button>
        </div>
      </header>

      <div className="app-body">
        <aside className="sidebar">
          <div className="brand-lockup">
            <div className="brand-mark">
              <BookOpen size={21} strokeWidth={2.2} />
            </div>
            <div className="brand-copy">
              <strong>Soft Explorer</strong>
              <span>MANGA / DESKTOP TOOL</span>
            </div>
            <button
              className="icon-button sidebar-toggle"
              onClick={() => setSidebarCollapsed(true)}
              aria-label="Contraer menú"
            >
              <PanelLeftClose size={17} />
            </button>
          </div>
          <div className="side-label">MANGA / LOCAL</div>
          <nav className="main-nav">
            <NavItem
              icon={<Home size={18} />}
              label="Inicio"
              active={route === "Inicio" && !genre}
              onClick={() => changeRoute("Inicio")}
            />
            <NavItem
              icon={<LibraryBig size={18} />}
              label="Biblioteca"
              active={route === "Biblioteca" && !genre}
              count={state?.mangaCount}
              onClick={() => changeRoute("Biblioteca")}
            />
            <NavItem
              icon={<Heart size={18} />}
              label="Favoritos"
              active={route === "Favoritos"}
              onClick={() => changeRoute("Favoritos")}
            />
          </nav>
          <div className="genre-header">
            <span className="side-label">GÉNEROS</span>
            <span className="genre-count">{state?.genres.length ?? 0}</span>
          </div>
          <nav className="genre-nav">
            {(state?.genres ?? []).map((name) => (
              <button
                key={name}
                className={`genre-link ${genre === name ? "selected" : ""}`}
                onClick={() => {
                  setGenre(name);
                  setRoute("Biblioteca");
                }}
              >
                <span className="genre-dot" />
                {name}
                <span className="genre-arrow">›</span>
              </button>
            ))}
            {!state?.genres.length && <p className="genre-empty">Tus géneros aparecerán aquí.</p>}
          </nav>
          <div className="sidebar-spacer" />
          <div className="sidebar-library-card">
            <div className="library-orbit">
              <Compass size={19} />
            </div>
            <p>{hasLibrary ? "ROOT / CONNECTED" : "ARCHIVE / UNLINKED"}</p>
            <span>{hasLibrary ? state?.libraryPath : "Selecciona una carpeta local"}</span>
            <button onClick={() => void chooseLibrary()}>
              {hasLibrary ? "Cambiar carpeta" : "Elegir carpeta"}
              <ChevronRight size={14} />
            </button>
          </div>
          <button className="settings-link" onClick={() => setDialog("settings")}>
            <Settings2 size={18} />
            <span>Configuración</span>
          </button>
        </aside>
        <button
          className={`sidebar-scrim ${sidebarCollapsed ? "hidden" : ""}`}
          aria-label="Cerrar menú"
          onClick={() => setSidebarCollapsed(true)}
        />

        <main className="main-content">
          {sidebarCollapsed && (
            <button className="reopen-sidebar" onClick={() => setSidebarCollapsed(false)}>
              <PanelLeftClose size={16} />
            </button>
          )}
          <div className="content-scroll">
            <section className="welcome-row">
              <div>
                <h1>
                  {heading}
                  <span className="title-period">.</span>
                </h1>
                <p className="welcome-sub">Colección local</p>
              </div>
              <div className="header-actions">
                <button
                  className="sun-button"
                  title="Cambiar tema"
                  onClick={() => {
                    const next = {
                      ...settings,
                      theme: settings.theme === "dark" ? "light" : "dark",
                    };
                    void saveSettings(next);
                  }}
                >
                  <Sun size={17} />
                </button>
                <button
                  className="primary-button scan-button"
                  onClick={() => (hasLibrary ? void runScan(true) : void chooseLibrary())}
                  disabled={scanning}
                >
                  {scanning ? <span className="spinner" /> : <LibraryBig size={17} />}
                  {scanning ? "Actualizando…" : "Actualizar biblioteca"}
                </button>
              </div>
            </section>

            <section className="dashboard-strip">
              <div className="strip-copy">
                <span className="strip-kicker">
                  MANGA INDEX <i /> ISSUE 001
                </span>
                <strong>
                  Tu biblioteca.
                  <br />
                  En orden.
                </strong>
              </div>
              <div className="strip-art" aria-hidden="true">
                <span className="archive-stamp">
                  SE
                  <br />
                  <b>01</b>
                </span>
                <div className="archive-lines">
                  <i />
                  <i />
                  <i />
                  <i />
                  <i />
                  <i />
                  <i />
                  <i />
                  <i />
                </div>
                <span className="archive-mark">✳</span>
              </div>
              <div className="strip-stats">
                <div className="stat-item">
                  <span>OBRAS</span>
                  <strong>{formatCount(state?.mangaCount ?? 0)}</strong>
                </div>
                <div className="stat-divider" />
                <div className="stat-item">
                  <span>GÉNEROS</span>
                  <strong>{formatCount(state?.genres.length ?? 0)}</strong>
                </div>
                <div className="stat-divider" />
                <div className="stat-item">
                  <span>EN ESTA VISTA</span>
                  <strong>{formatCount(currentCount)}</strong>
                </div>
              </div>
            </section>

            {progress && (
              <div className="scan-progress">
                <div className="progress-top">
                  <span>
                    <span className="spinner coral-spinner" /> {progress.current}
                  </span>
                  <b>
                    {progress.total
                      ? `${Math.round((progress.done / progress.total) * 100)}%`
                      : "…"}
                  </b>
                </div>
                <div className="progress-track">
                  <i
                    style={{
                      width: progress.total ? `${(progress.done / progress.total) * 100}%` : "18%",
                    }}
                  />
                </div>
                <div className="progress-meta">
                  <span>{progress.genres} géneros</span>
                  <span>
                    {formatCount(progress.done)} / {formatCount(progress.total)} obras
                  </span>
                  <span>{formatCount(progress.images)} imágenes</span>
                </div>
              </div>
            )}

            <section className="collection-section">
              <div className="section-heading">
                <div>
                  <span className="eyebrow small-eyebrow">
                    CATÁLOGO /{" "}
                    {route === "Favoritos" ? "FAVORITOS" : genre ? "GÉNERO" : "TODAS LAS OBRAS"}
                  </span>
                  <h2>
                    {genre ||
                      (route === "Favoritos"
                        ? "Favoritos"
                        : route === "Biblioteca"
                          ? "Todas las obras"
                          : "Índice de lectura")}
                  </h2>
                </div>
                <div className="collection-tools">
                  <label className="search-box">
                    <Search size={17} />
                    <input
                      ref={searchInput}
                      value={query}
                      onChange={(event) => setQuery(event.target.value)}
                      placeholder="Buscar una obra…"
                    />
                    <kbd>Ctrl K</kbd>
                  </label>
                  <label className="select-box">
                    <ListFilter size={16} />
                    <select
                      value={sort}
                      onChange={(event) => setSort(event.target.value)}
                      aria-label="Ordenar mangas"
                    >
                      <option>Nombre A-Z</option>
                      <option>Nombre Z-A</option>
                      <option>Género</option>
                      <option>Favoritos primero</option>
                      <option>Últimos añadidos</option>
                      <option>Últimos leídos</option>
                    </select>
                    <ChevronDown size={14} />
                  </label>
                  <label className="cover-size-control" title="Tamaño de las portadas">
                    <input
                      type="range"
                      min="144"
                      max="192"
                      step="8"
                      value={coverSize}
                      onChange={(event) => {
                        const value = event.target.value;
                        setCoverSize(Number(value));
                        setSettings((current) => ({ ...current, coverSize: value }));
                        void invoke("set_preference", { key: "coverSize", value });
                      }}
                    />
                  </label>
                  <button
                    className={`view-toggle ${compact ? "active" : ""}`}
                    title="Vista compacta"
                    onClick={() => setCompact(!compact)}
                  >
                    <SlidersHorizontal size={16} />
                  </button>
                </div>
              </div>

              {!hasLibrary ? (
                <EmptyLibrary onChoose={() => void chooseLibrary()} />
              ) : mangas.length ? (
                <LibraryGrid
                  mangas={mangas}
                  coverSize={coverSize}
                  compact={compact}
                  onOpen={(manga) => setReading(manga)}
                  onFavorite={(manga) => void toggleFavorite(manga)}
                  onMore={(manga) => {
                    setSelected(manga);
                    setDialog("menu");
                  }}
                />
              ) : (
                <EmptyResults
                  hasQuery={Boolean(query)}
                  onClear={() => {
                    setQuery("");
                    setGenre("");
                    changeRoute("Inicio");
                  }}
                />
              )}
            </section>
            <footer className="content-footer">
              <span>SOFT EXPLORER / BUILD 01</span>
            </footer>
          </div>
        </main>
      </div>

      {dialog === "settings" && (
        <SettingsDialog
          settings={settings}
          coverSize={coverSize}
          onSave={saveSettings}
          onClose={() => setDialog(null)}
          onChooseLibrary={() => {
            setDialog(null);
            void chooseLibrary();
          }}
          notify={notify}
        />
      )}
      {dialog === "cover" && selected && (
        <CoverDialog
          manga={selected}
          onClose={() => setDialog(null)}
          onSaved={() => {
            setDialog(null);
            void refreshManga();
          }}
          notify={notify}
        />
      )}
      {dialog === "menu" && selected && (
        <ActionDialog
          manga={selected}
          onClose={() => setDialog(null)}
          onRead={() => {
            setDialog(null);
            setReading(selected);
          }}
          onCover={() => setDialog("cover")}
          onFavorite={() => {
            void toggleFavorite(selected);
            setDialog(null);
          }}
        />
      )}
      {reading && (
        <Reader
          manga={reading}
          settings={settings}
          onClose={() => {
            setReading(null);
            void refreshManga();
          }}
          notify={notify}
        />
      )}
      {toast && (
        <div className="toast">
          <span className="toast-check">
            <Check size={14} />
          </span>
          {toast}
        </div>
      )}
    </div>
  );
}

function NavItem({
  icon,
  label,
  active,
  count,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  active: boolean;
  count?: number;
  onClick: () => void;
}) {
  return (
    <button className={`nav-item ${active ? "active" : ""}`} onClick={onClick}>
      {icon}
      <span>{label}</span>
      {count !== undefined && <small>{count}</small>}
    </button>
  );
}

function EmptyLibrary({ onChoose }: { onChoose: () => void }) {
  return (
    <div className="empty-state onboarding">
      <div className="empty-illustration" aria-hidden="true">
        <div className="empty-glow" />
        <div className="empty-page page-one">
          <span />
          <span />
          <span />
        </div>
        <div className="empty-page page-two">
          <b>01</b>
          <i />
          <i />
        </div>
        <div className="empty-star star-a">+</div>
        <div className="empty-star star-b">✳</div>
      </div>
      <span className="eyebrow">ARCHIVE / EMPTY</span>
      <h3>
        La colección empieza
        <br />
        con una carpeta.
      </h3>
      <p>
        Selecciona el directorio que contiene tus géneros. Se indexa en local, tus originales se
        quedan en su sitio.
      </p>
      <button className="primary-button" onClick={onChoose}>
        <FolderOpen size={17} /> Elegir biblioteca
      </button>
    </div>
  );
}

function EmptyResults({ hasQuery, onClear }: { hasQuery: boolean; onClear: () => void }) {
  return (
    <div className="empty-results">
      <div className="empty-search-icon">
        <Search size={24} />
      </div>
      <h3>{hasQuery ? "No encontramos esa historia" : "Todavía no hay mangas en esta vista"}</h3>
      <p>
        {hasQuery
          ? "Prueba con otro nombre o una palabra más corta."
          : "Prueba otro género o vuelve a toda la biblioteca."}
      </p>
      <button className="quiet-button" onClick={onClear}>
        Volver a explorar
      </button>
    </div>
  );
}

/** Preferencias compartidas por el catálogo y el lector; se escriben en SQLite al guardar. */
function SettingsDialog({
  settings,
  coverSize,
  onSave,
  onClose,
  onChooseLibrary,
  notify,
}: {
  settings: Record<string, string>;
  coverSize: number;
  onSave: (settings: Record<string, string>) => void;
  onClose: () => void;
  onChooseLibrary: () => void;
  notify: (message: string) => void;
}) {
  const [draft, setDraft] = useState<Record<string, string>>({
    ...settings,
    coverSize: String(coverSize),
  });
  const patch = (key: string, value: string) =>
    setDraft((current) => ({ ...current, [key]: value }));
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section className="settings-modal modal-card">
        <div className="modal-heading">
          <div>
            <span className="eyebrow">A TU MANERA</span>
            <h2>Un espacio que se siente tuyo.</h2>
            <p>Ajusta la lectura a tu ritmo.</p>
          </div>
          <button className="icon-button" onClick={onClose}>
            <X size={18} />
          </button>
        </div>
        <div className="settings-grid">
          <label className="setting-row">
            <span>
              <b>Tema visual</b>
              <small>Soft Explorer claro u oscuro</small>
            </span>
            <select value={draft.theme} onChange={(event) => patch("theme", event.target.value)}>
              <option value="light">Nube clara</option>
              <option value="dark">Nube borrascosa</option>
            </select>
          </label>
          <label className="setting-row">
            <span>
              <b>Dirección de lectura</b>
              <small>El sentido natural de tus páginas</small>
            </span>
            <select
              value={draft.direction}
              onChange={(event) => patch("direction", event.target.value)}
            >
              <option value="western">Occidental · izquierda a derecha</option>
              <option value="eastern">Oriental · derecha a izquierda</option>
            </select>
          </label>
          <label className="setting-row">
            <span>
              <b>Imagen al abrir</b>
              <small>Cómo encaja cada página en pantalla</small>
            </span>
            <select value={draft.fit} onChange={(event) => patch("fit", event.target.value)}>
              <option value="screen">Ajustar a pantalla</option>
              <option value="width">Ajustar al ancho</option>
              <option value="height">Ajustar a la altura</option>
              <option value="original">Tamaño original</option>
              <option value="zoom">Zoom</option>
            </select>
          </label>
          <label className="setting-row">
            <span>
              <b>Doble página</b>
              <small>La portada siempre se muestra sola</small>
            </span>
            <button
              className={`switch ${draft.doublePage === "true" ? "on" : ""}`}
              onClick={() => patch("doublePage", draft.doublePage === "true" ? "false" : "true")}
            >
              <i />
            </button>
          </label>
          <label className="setting-row">
            <span>
              <b>Lectura automática</b>
              <small>Segundos entre cada página</small>
            </span>
            <select
              value={draft.autoSeconds}
              onChange={(event) => patch("autoSeconds", event.target.value)}
            >
              {[1, 2, 3, 5, 10, 15, 30].map((value) => (
                <option key={value} value={value}>
                  {value} segundos
                </option>
              ))}
            </select>
          </label>
          <label className="setting-row">
            <span>
              <b>Tecla extra · siguiente</b>
              <small>Flecha derecha y espacio también avanzan</small>
            </span>
            <select
              value={draft.nextKey}
              onChange={(event) => patch("nextKey", event.target.value)}
            >
              <option value="PageDown">Av Pág</option>
              <option value="Down">Flecha abajo</option>
              <option value="Enter">Intro</option>
              <option value="F">F</option>
            </select>
          </label>
          <label className="setting-row">
            <span>
              <b>Tecla extra · anterior</b>
              <small>Flecha izquierda y retroceso también vuelven</small>
            </span>
            <select
              value={draft.previousKey}
              onChange={(event) => patch("previousKey", event.target.value)}
            >
              <option value="PageUp">Re Pág</option>
              <option value="Up">Flecha arriba</option>
              <option value="Backspace">Retroceso</option>
              <option value="B">B</option>
            </select>
          </label>
          <label className="setting-row">
            <span>
              <b>Tamaño de portada</b>
              <small>{draft.coverSize} px · ajustable desde la biblioteca</small>
            </span>
            <input
              className="range-control"
              type="range"
              min="144"
              max="192"
              step="8"
              value={draft.coverSize}
              onChange={(event) => patch("coverSize", event.target.value)}
            />
          </label>
          <div className="setting-row setting-actions-row">
            <span>
              <b>Carpeta de biblioteca</b>
              <small>La colección se lee desde la ubicación que elijas</small>
            </span>
            <button className="quiet-button" onClick={onChooseLibrary}>
              Cambiar carpeta
            </button>
          </div>
          <div className="setting-row setting-actions-row">
            <span>
              <b>Miniaturas locales</b>
              <small>Los originales no se afectan</small>
            </span>
            <button
              className="quiet-button"
              onClick={async () => {
                try {
                  const count = await invoke<number>("clear_thumbnail_cache");
                  notify(`${formatCount(count)} miniaturas eliminadas`);
                } catch (error) {
                  notify(errorMessage(error));
                }
              }}
            >
              Limpiar caché
            </button>
          </div>
        </div>
        <div className="modal-actions">
          <button className="quiet-button" onClick={onClose}>
            Cancelar
          </button>
          <button className="primary-button" onClick={() => onSave(draft)}>
            Guardar preferencias <ChevronRight size={16} />
          </button>
        </div>
      </section>
    </div>
  );
}

/** Selector limitado a páginas de la obra; la previsualización usa el caché externo. */
function CoverDialog({
  manga,
  onClose,
  onSaved,
  notify,
}: {
  manga: Manga;
  onClose: () => void;
  onSaved: () => void;
  notify: (message: string) => void;
}) {
  const [pages, setPages] = useState<string[]>([]);
  const [selectedPath, setSelectedPath] = useState(manga.coverPath);
  const [previewUrl, setPreviewUrl] = useState("");
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    invoke<string[]>("get_pages", { mangaId: manga.id })
      .then((items) => setPages(items))
      .catch((error) => notify(errorMessage(error)));
  }, [manga.id, notify]);
  useEffect(() => {
    if (!selectedPath) return;
    invoke<string>("get_thumbnail", { path: selectedPath })
      .then((path) => {
        setPreviewUrl(convertFileSrc(path));
      })
      .catch(() => setPreviewUrl(""));
  }, [selectedPath]);
  const save = async () => {
    setSaving(true);
    try {
      await invoke("set_cover", { mangaId: manga.id, path: selectedPath });
      notify("Portada actualizada");
      onSaved();
    } catch (error) {
      notify(errorMessage(error));
    } finally {
      setSaving(false);
    }
  };
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section className="cover-modal modal-card">
        <div className="modal-heading">
          <div>
            <span className="eyebrow">UNA NUEVA CUBIERTA</span>
            <h2>Elige la portada.</h2>
            <p>
              {manga.title} · {pages.length} imágenes
            </p>
          </div>
          <button className="icon-button" onClick={onClose}>
            <X size={18} />
          </button>
        </div>
        <div className="cover-picker">
          <div className="cover-page-list">
            {pages.map((path, index) => (
              <button
                key={path}
                className={`cover-page-option ${path === selectedPath ? "chosen" : ""}`}
                onClick={() => setSelectedPath(path)}
              >
                <span className="cover-page-index">{String(index + 1).padStart(2, "0")}</span>
                <span>{path.split(/[\\/]/).pop()}</span>
                {path === selectedPath && <Check size={15} />}
              </button>
            ))}
          </div>
          <div className="cover-preview">
            {previewUrl ? (
              <img src={previewUrl} alt="Vista previa de portada" />
            ) : (
              <div className="preview-loading">
                <span className="spinner" />
                Preparando vista previa
              </div>
            )}
            <span>VISTA PREVIA</span>
          </div>
        </div>
        <div className="modal-actions">
          <button className="quiet-button" onClick={onClose}>
            Cancelar
          </button>
          <button
            className="primary-button"
            onClick={() => void save()}
            disabled={!selectedPath || saving}
          >
            {saving ? "Guardando…" : "Usar esta portada"}
            <Check size={16} />
          </button>
        </div>
      </section>
    </div>
  );
}

function ActionDialog({
  manga,
  onClose,
  onRead,
  onCover,
  onFavorite,
}: {
  manga: Manga;
  onClose: () => void;
  onRead: () => void;
  onCover: () => void;
  onFavorite: () => void;
}) {
  return (
    <div
      className="modal-backdrop menu-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section className="action-menu modal-card">
        <div className="action-menu-title">{manga.title}</div>
        <button onClick={onRead}>
          <BookOpen size={16} />
          {manga.lastPage ? `Continuar · página ${manga.lastPage + 1}` : "Leer desde el inicio"}
        </button>
        <button onClick={onFavorite}>
          <Heart size={16} />
          {manga.favorite ? "Quitar de favoritos" : "Guardar en favoritos"}
        </button>
        <button onClick={onCover}>
          <LibraryBig size={16} />
          Cambiar portada…
        </button>
        <button onClick={onClose}>
          <X size={16} />
          Cerrar menú
        </button>
      </section>
    </div>
  );
}
