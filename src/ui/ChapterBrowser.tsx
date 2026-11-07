import { useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { BookOpen, ChevronRight, Image as ImageIcon } from "lucide-react";
import type { Chapter, Manga } from "./types";
import { errorMessage, formatCount } from "./format";

/** Vista integrada de capítulos; las portadas se leen desde la caché privada de la app. */
export default function ChapterBrowser({
  manga,
  onRead,
  notify,
}: {
  manga: Manga;
  onRead: (chapter: Chapter) => void;
  notify: (message: string) => void;
}) {
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    invoke<Chapter[]>("list_chapters", { mangaId: manga.id })
      .then((items) => alive && setChapters(items))
      .catch((error) => alive && notify(errorMessage(error)))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [manga.id, notify]);

  const resumeChapter = [...chapters]
    .filter((chapter) => chapter.lastRead > 0)
    .sort((left, right) => right.lastRead - left.lastRead)[0];

  return (
    <section className="chapter-browser" aria-label={`Capítulos de ${manga.title}`}>
      <div className="chapter-browser-heading">
        <div>
          <span className="eyebrow small-eyebrow">ÍNDICE DE LA SERIE</span>
          <h2>Capítulos</h2>
          <p>
            {loading ? "Buscando capítulos…" : `${formatCount(chapters.length)} capítulos`}
            <i />
            {formatCount(manga.pageCount)} páginas
          </p>
        </div>
        {resumeChapter && (
          <button className="chapter-continue" onClick={() => onRead(resumeChapter)}>
            <BookOpen size={17} />
            <span>
              <small>CONTINUAR LEYENDO</small>
              <b>{resumeChapter.title}</b>
            </span>
            <span className="chapter-continue-page">Pág. {resumeChapter.lastPage + 1}</span>
            <ChevronRight size={17} />
          </button>
        )}
      </div>

      {loading ? (
        <div className="chapter-browser-empty">
          <span className="spinner" /> Cargando capítulos…
        </div>
      ) : chapters.length ? (
        <div className="chapter-card-grid">
          {chapters.map((chapter, index) => (
            <ChapterCard key={chapter.id} chapter={chapter} index={index} onRead={onRead} />
          ))}
        </div>
      ) : (
        <div className="chapter-browser-empty">
          No se encontraron capítulos legibles en esta serie.
        </div>
      )}
    </section>
  );
}

function ChapterCard({
  chapter,
  index,
  onRead,
}: {
  chapter: Chapter;
  index: number;
  onRead: (chapter: Chapter) => void;
}) {
  const cardRef = useRef<HTMLButtonElement>(null);
  const [visible, setVisible] = useState(false);
  const [thumbnail, setThumbnail] = useState("");
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    const element = cardRef.current;
    if (!element) return;
    const observer = new IntersectionObserver(([entry]) => {
      if (entry.isIntersecting) {
        setVisible(true);
        observer.disconnect();
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!visible) return;
    let alive = true;
    invoke<string>("get_thumbnail", { path: chapter.coverPath, entry: chapter.coverEntry })
      .then((path) => alive && setThumbnail(convertFileSrc(path)))
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
    };
  }, [chapter.coverPath, chapter.coverEntry, visible]);

  const progress =
    chapter.lastRead > 0 && chapter.pageCount > 0
      ? Math.min(100, Math.round(((chapter.lastPage + 1) / chapter.pageCount) * 100))
      : 0;

  return (
    <button ref={cardRef} className="chapter-card" onClick={() => onRead(chapter)}>
      <span className="chapter-card-cover">
        {thumbnail && !failed ? (
          <img
            src={thumbnail}
            alt={`Portada de ${chapter.title}`}
            loading="lazy"
            onError={() => setFailed(true)}
          />
        ) : (
          <span className="chapter-cover-placeholder">
            <ImageIcon size={25} />
            <small>PORTADA</small>
          </span>
        )}
        <span className="chapter-card-number">{String(index + 1).padStart(2, "0")}</span>
        {progress > 0 && (
          <span className="chapter-card-progress">
            <i style={{ width: `${progress}%` }} />
          </span>
        )}
      </span>
      <span className="chapter-card-meta">
        <b title={chapter.title}>{chapter.title}</b>
        <span>
          {formatCount(chapter.pageCount)} páginas
          {progress > 0 && <i> · {progress}% leído</i>}
        </span>
      </span>
      <ChevronRight className="chapter-card-arrow" size={16} />
    </button>
  );
}
