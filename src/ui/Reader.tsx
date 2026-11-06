import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent,
  type WheelEvent,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ArrowLeft,
  ArrowRight,
  ChevronLeft,
  ChevronRight,
  Maximize2,
  Pause,
  Play,
  SkipBack,
  SkipForward,
  Square,
  X,
} from "lucide-react";
import type { Manga } from "./types";
import { errorMessage } from "./format";

type Settings = Record<string, string>;

export default function Reader({
  manga,
  settings,
  onClose,
  notify,
}: {
  manga: Manga;
  settings: Settings;
  onClose: () => void;
  notify: (message: string) => void;
}) {
  const [pages, setPages] = useState<string[]>([]);
  const [readerImages, setReaderImages] = useState<string[]>([]);
  const [page, setPage] = useState(manga.lastPage || 0);
  const [busy, setBusy] = useState(true);
  const [fit, setFit] = useState(settings.fit || "screen");
  const [direction, setDirection] = useState(settings.direction || "western");
  const [doublePage, setDoublePage] = useState(settings.doublePage === "true");

  // Presentation mode starts clean; mouse movement reveals controls temporarily.
  const [controls, setControls] = useState(false);
  const [playing, setPlaying] = useState(false);
  const [autoActive, setAutoActive] = useState(false);
  const [seconds, setSeconds] = useState(Number(settings.autoSeconds) || 5);
  const [zoom, setZoom] = useState(1);
  const [error, setError] = useState("");
  const fadeTimer = useRef<number | undefined>(undefined);
  const total = pages.length;

  useEffect(() => {
    let alive = true;
    invoke<string[]>("get_pages", { mangaId: manga.id })
      .then((data) => {
        if (alive) {
          setPages(data);
          setPage(Math.min(Math.max(0, manga.lastPage), Math.max(0, data.length - 1)));
        }
      })
      .catch((reason) => setError(errorMessage(reason)))
      .finally(() => alive && setBusy(false));
    // Use the native fullscreen state rather than simulating it with CSS.
    void getCurrentWindow().setFullscreen(true);
    return () => {
      alive = false;
      void getCurrentWindow().setFullscreen(false);
      if (fadeTimer.current) window.clearTimeout(fadeTimer.current);
    };
  }, [manga.id, manga.lastPage]);

  const close = useCallback(() => {
    setPlaying(false);
    setAutoActive(false);
    void getCurrentWindow().setFullscreen(false);
    onClose();
  }, [onClose]);

  const savePage = useCallback(
    (next: number) => {
      setPage(next);
      void invoke("save_progress", { mangaId: manga.id, page: next }).catch((reason) =>
        notify(errorMessage(reason)),
      );
    },
    [manga.id, notify],
  );

  const next = useCallback(() => {
    if (!total) return;
    const step = doublePage && page !== 0 ? 2 : 1;
    if (page + step >= total) {
      savePage(total - 1);
      setPlaying(false);
      setAutoActive(false);
    } else savePage(page + step);
  }, [doublePage, page, savePage, total]);

  const previous = useCallback(() => {
    const step = doublePage && page > 1 ? 2 : 1;
    savePage(Math.max(0, page - step));
  }, [doublePage, page, savePage]);

  const first = () => savePage(0);
  const last = () => savePage(Math.max(0, total - 1));
  const pageIndices = useMemo(() => {
    // Keep the cover alone in a spread and swap the pair for right-to-left reading.
    if (doublePage && page > 0 && page + 1 < total)
      return direction === "eastern" ? [page + 1, page] : [page, page + 1];
    return [page];
  }, [direction, doublePage, page, total]);

  useEffect(() => {
    if (!pages.length) return;
    // Resolve only the visible page(s); GIF/TIFF conversion is delegated to Rust workers.
    let alive = true;
    setReaderImages([]);
    Promise.all(
      pageIndices.map((index) =>
        invoke<string>("get_reader_image", { path: pages[index] })
          .then((path) => convertFileSrc(path))
          .catch(() => ""),
      ),
    )
      .then((images) => alive && setReaderImages(images))
      .catch(() => alive && setReaderImages([]));
    return () => {
      alive = false;
    };
  }, [pageIndices, pages]);

  useEffect(() => {
    if (!playing) return;
    const timer = window.setInterval(() => {
      const step = doublePage && page !== 0 ? 2 : 1;
      if (page + step >= total) {
        savePage(Math.max(0, total - 1));
        setPlaying(false);
        setAutoActive(false);
      } else savePage(page + step);
    }, seconds * 1000);
    return () => window.clearInterval(timer);
  }, [doublePage, page, playing, savePage, seconds, total]);

  const revealControls = () => {
    setControls(true);
    if (fadeTimer.current) window.clearTimeout(fadeTimer.current);
    fadeTimer.current = window.setTimeout(() => setControls(false), 2600);
  };

  const enterPresentation = () => {
    // Autoplay should not leave its toolbar sitting on top of the artwork.
    if (fadeTimer.current) window.clearTimeout(fadeTimer.current);
    setControls(false);
  };

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        close();
      } else if (event.key === "F11") {
        event.preventDefault();
        const window = getCurrentWindow();
        void window.isFullscreen().then((fullscreen) => window.setFullscreen(!fullscreen));
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        direction === "western" ? next() : previous();
      } else if (event.key === "ArrowLeft") {
        event.preventDefault();
        direction === "western" ? previous() : next();
      } else if (
        event.key === " " ||
        event.key === "PageDown" ||
        event.key === settings.nextKey ||
        event.key.toUpperCase() === settings.nextKey
      ) {
        event.preventDefault();
        next();
      } else if (
        event.key === "Backspace" ||
        event.key === "PageUp" ||
        event.key === settings.previousKey ||
        event.key.toUpperCase() === settings.previousKey
      ) {
        event.preventDefault();
        previous();
      } else if (event.key === "Home") {
        event.preventDefault();
        first();
      } else if (event.key === "End") {
        event.preventDefault();
        last();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [close, direction, next, previous, settings.nextKey, settings.previousKey, total]);

  const clickPage = (event: MouseEvent<HTMLDivElement>) => {
    const forward = event.clientX > window.innerWidth / 2;
    const logicalNext = direction === "western" ? forward : !forward;
    logicalNext ? next() : previous();
  };

  const wheelPage = (event: WheelEvent<HTMLDivElement>) => {
    event.preventDefault();
    if (fit === "zoom" || event.ctrlKey)
      setZoom((value) => Math.max(0.5, Math.min(3, value + (event.deltaY < 0 ? 0.1 : -0.1))));
    else event.deltaY > 0 ? next() : previous();
  };

  const fitClass = `fit-${fit}`;
  const pageLabelEnd = Math.min(total, page + (doublePage && page > 0 ? 2 : 1));

  return (
    <div
      className={`reader-shell ${controls ? "controls-visible" : "controls-hidden"}`}
      onMouseMove={revealControls}
    >
      <div className="reader-stage" onClick={clickPage} onWheel={wheelPage}>
        {busy ? (
          <div className="reader-loading">
            <span className="spinner" />
            <span>Preparando tus páginas…</span>
          </div>
        ) : error ? (
          <div className="reader-error">
            <span className="error-orb">!</span>
            <h2>No pudimos abrir esta obra</h2>
            <p>{error}</p>
            <button className="quiet-button" onClick={close}>
              Volver a la biblioteca
            </button>
          </div>
        ) : !total ? (
          <div className="reader-error">
            <h2>Esta carpeta está vacía</h2>
            <button className="quiet-button" onClick={close}>
              Cerrar lector
            </button>
          </div>
        ) : (
          <div
            className={`reader-pages ${pageIndices.length === 2 ? "spread" : ""} ${fitClass}`}
            style={{ transform: `scale(${zoom})` }}
          >
            {pageIndices.map((index, position) =>
              readerImages.length === pageIndices.length ? (
                readerImages[position] ? (
                  <img
                    key={pages[index]}
                    draggable={false}
                    src={readerImages[position]}
                    alt={`Página ${index + 1}`}
                    onError={() =>
                      setReaderImages((current) =>
                        current.map((source, imageIndex) =>
                          imageIndex === position ? "" : source,
                        ),
                      )
                    }
                  />
                ) : (
                  <div className="reader-page-broken" key={pages[index]}>
                    Esta página no se pudo abrir.
                    <br />
                    <span>Usa → para continuar</span>
                  </div>
                )
              ) : (
                <div className="reader-page-loading" key={pages[index]}>
                  <span className="spinner" />
                </div>
              ),
            )}
          </div>
        )}
        <button
          className="reader-hit previous"
          aria-label="Página anterior"
          onClick={(event) => {
            event.stopPropagation();
            previous();
          }}
        >
          <ChevronLeft />
        </button>
        <button
          className="reader-hit next"
          aria-label="Página siguiente"
          onClick={(event) => {
            event.stopPropagation();
            next();
          }}
        >
          <ChevronRight />
        </button>
      </div>

      <div className={`reader-top ${controls ? "visible" : "hidden"}`}>
        <button className="reader-back" onClick={close}>
          <ArrowLeft size={17} />
          <span>Volver</span>
        </button>
        <div className="reader-title">
          <span className="reader-kicker">AHORA LEYENDO</span>
          <strong>{manga.title}</strong>
        </div>
        <div className="reader-tools">
          <button
            className="reader-small-button"
            onClick={() => setDirection((value) => (value === "western" ? "eastern" : "western"))}
          >
            <ArrowLeft size={15} />
            <ArrowRight size={15} />
            <span>{direction === "western" ? "Occidental" : "Oriental"}</span>
          </button>
          <button
            className={`reader-small-button ${doublePage ? "active" : ""}`}
            onClick={() => setDoublePage(!doublePage)}
          >
            Dos páginas
          </button>
          <button className="reader-exit" onClick={close}>
            <X size={16} /> Salir
          </button>
        </div>
      </div>

      <div className={`reader-bottom ${controls ? "visible" : "hidden"}`}>
        <div className="reader-bottom-row">
          <button className="reader-icon" title="Primera página" onClick={first}>
            <SkipBack size={17} />
          </button>
          <button className="reader-nav" onClick={previous}>
            <ChevronLeft size={17} />
            Anterior
          </button>
          <div className="reader-page-count">
            <strong>
              {total ? page + 1 : 0}
              {pageLabelEnd > page + 1 ? `–${pageLabelEnd}` : ""}
            </strong>
            <span> / {total} páginas</span>
          </div>
          <button className="reader-nav" onClick={next}>
            Siguiente
            <ChevronRight size={17} />
          </button>
          <button className="reader-icon" title="Última página" onClick={last}>
            <SkipForward size={17} />
          </button>
          <div className="reader-separator" />
          <select
            className="reader-fit"
            value={fit}
            onChange={(event) => {
              setFit(event.target.value);
              setZoom(1);
            }}
            aria-label="Ajuste de página"
          >
            <option value="screen">Ajustar a pantalla</option>
            <option value="width">Ajustar al ancho</option>
            <option value="height">Ajustar a la altura</option>
            <option value="original">Tamaño original</option>
            <option value="zoom">Zoom</option>
          </select>
          <button
            className={`reader-play ${playing ? "playing" : ""}`}
            onClick={() => {
              if (autoActive && playing) setPlaying(false);
              else {
                setAutoActive(true);
                setPlaying(true);
                enterPresentation();
              }
            }}
          >
            {playing ? <Pause size={15} /> : <Play size={15} fill="currentColor" />}
            {playing ? "Pausar" : autoActive ? "Reanudar" : "Auto"}
          </button>
          {autoActive && (
            <button
              className="reader-icon stop-auto"
              title="Detener lectura automática"
              onClick={() => {
                setAutoActive(false);
                setPlaying(false);
                revealControls();
              }}
            >
              <Square size={12} fill="currentColor" />
            </button>
          )}
          <select
            className="reader-speed"
            value={seconds}
            onChange={(event) => setSeconds(Number(event.target.value))}
            aria-label="Velocidad automática"
          >
            {[1, 2, 3, 5, 10, 15, 30].map((second) => (
              <option key={second} value={second}>
                {second}s
              </option>
            ))}
          </select>
          <button
            className="reader-icon fullscreen-exit"
            onClick={() => void getCurrentWindow().setFullscreen(false)}
            title="Salir de pantalla completa"
          >
            <Maximize2 size={16} />
          </button>
        </div>
        <div className="reader-progress-line">
          <input
            type="range"
            min="1"
            max={Math.max(1, total)}
            value={Math.min(total || 1, page + 1)}
            onChange={(event) => {
              setZoom(1);
              const nextPage = Number(event.target.value) - 1;
              setPage(nextPage);
              void invoke("save_progress", { mangaId: manga.id, page: nextPage });
            }}
          />
          <span
            className="progress-thumb"
            style={{ left: `${total > 1 ? (page / (total - 1)) * 100 : 0}%` }}
          />
        </div>
      </div>
      <div className="reader-corner-hint">
        <span>←</span>
        <i />
        ESC para volver
      </div>
    </div>
  );
}
