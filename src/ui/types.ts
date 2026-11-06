/** Campos serializados desde la tabla local `manga` en el backend Rust. */
export interface Manga {
  id: number;
  path: string;
  title: string;
  genre: string;
  pageCount: number;
  coverPath: string;
  favorite: boolean;
  customCover: boolean;
  lastPage: number;
  lastRead: number;
  addedAt: number;
}

/** Snapshot liviano usado para construir la navegación al arrancar. */
export interface InitialState {
  libraryPath: string | null;
  mangaCount: number;
  genres: string[];
  settings: Record<string, string>;
}

/** Contadores emitidos periódicamente por el escáner en segundo plano. */
export interface ScanProgress {
  done: number;
  total: number;
  genres: number;
  images: number;
  current: string;
}

/** Resumen final del escaneo que actualiza la base de datos. */
export interface ScanSummary {
  mangas: number;
  genres: number;
  images: number;
}

export type Route = "Inicio" | "Biblioteca" | "Favoritos";
