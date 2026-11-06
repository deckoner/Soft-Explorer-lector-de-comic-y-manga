import { useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { Heart, MoreHorizontal, BookOpen, Image as ImageIcon } from "lucide-react";
import type { Manga } from "./types";
import { formatCount } from "./format";

interface Props {
  mangas: Manga[];
  coverSize: number;
  compact: boolean;
  onOpen: (manga: Manga) => void;
  onFavorite: (manga: Manga) => void;
  onMore: (manga: Manga) => void;
}

export default function LibraryGrid({
  mangas,
  coverSize,
  compact,
  onOpen,
  onFavorite,
  onMore,
}: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // Virtualize rows instead of mounting every cover in a large collection.
  // The CSS auto-fill minimum and this row count deliberately share the same width.
  const gap = compact ? 1 : 1.25;
  const gapPixels = gap * 16;
  const tileWidth = compact ? Math.max(144, coverSize - 16) : coverSize;
  const minTileWidth = Math.min(tileWidth, 192);
  const columns = Math.max(1, Math.floor((width + gapPixels) / (minTileWidth + gapPixels)));
  const rows = Math.ceil(mangas.length / columns);
  const rowHeight = Math.round(minTileWidth * 1.82 + 40);
  const virtualizer = useVirtualizer({
    count: rows,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    overscan: 3,
  });
  const virtualRows = virtualizer.getVirtualItems();

  return (
    <div className="library-grid-scroll" ref={scrollRef}>
      <div className="virtual-spacer" style={{ height: `${virtualizer.getTotalSize()}px` }}>
        {virtualRows.map((row) => {
          const books = mangas.slice(row.index * columns, row.index * columns + columns);
          return (
            <div
              className="virtual-row"
              key={row.key}
              data-index={row.index}
              ref={virtualizer.measureElement}
              style={{
                transform: `translateY(${row.start}px)`,
                gridTemplateColumns: `repeat(auto-fill, minmax(min(100%, ${minTileWidth}px), 1fr))`,
                gap: `${gap}rem`,
              }}
            >
              {books.map((manga) => (
                <MangaCard
                  key={manga.id}
                  manga={manga}
                  compact={compact}
                  onOpen={onOpen}
                  onFavorite={onFavorite}
                  onMore={onMore}
                />
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** Pide una miniatura reducida, nunca la imagen original a tamaño completo. */
function MangaCard({
  manga,
  compact,
  onOpen,
  onFavorite,
  onMore,
}: Omit<Props, "mangas" | "coverSize"> & { manga: Manga }) {
  const [thumbnail, setThumbnail] = useState("");
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let alive = true;
    invoke<string>("get_thumbnail", { path: manga.coverPath })
      .then((path) => alive && setThumbnail(convertFileSrc(path)))
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
    };
  }, [manga.coverPath]);
  const progress = useMemo(
    () =>
      manga.lastPage > 0 && manga.pageCount > 0
        ? Math.min(100, Math.round(((manga.lastPage + 1) / manga.pageCount) * 100))
        : 0,
    [manga.lastPage, manga.pageCount],
  );

  return (
    <article className={`manga-card ${compact ? "compact" : ""}`} onClick={() => onOpen(manga)}>
      <div className="cover-frame">
        {thumbnail && !failed ? (
          <img
            src={thumbnail}
            alt={`Portada de ${manga.title}`}
            loading="lazy"
            onError={() => setFailed(true)}
          />
        ) : (
          <div className="cover-placeholder">
            <ImageIcon size={28} />
            <span>Portada disponible al abrir</span>
          </div>
        )}
        <div className="cover-overlay" />
        <button
          className={`favorite-chip ${manga.favorite ? "favorited" : ""}`}
          aria-label={manga.favorite ? "Quitar favorito" : "Añadir favorito"}
          onClick={(event) => {
            event.stopPropagation();
            onFavorite(manga);
          }}
        >
          <Heart size={16} fill={manga.favorite ? "currentColor" : "none"} />
        </button>
        <button
          className="card-more"
          aria-label="Más opciones"
          onClick={(event) => {
            event.stopPropagation();
            onMore(manga);
          }}
        >
          <MoreHorizontal size={17} />
        </button>
        {progress > 0 && (
          <div className="cover-progress">
            <i style={{ width: `${progress}%` }} />
          </div>
        )}
      </div>
      <div className="manga-meta">
        <div className="manga-title-row">
          <h3 title={manga.title}>{manga.title}</h3>
          {manga.favorite && <span className="tiny-favorite">✦</span>}
        </div>
        <div className="manga-subtitle">
          <span>{manga.genre}</span>
          <i />
          {progress ? (
            <span className="reading-progress">
              <BookOpen size={12} />
              {progress}%
            </span>
          ) : (
            <span>{formatCount(manga.pageCount)} págs.</span>
          )}
        </div>
      </div>
    </article>
  );
}
