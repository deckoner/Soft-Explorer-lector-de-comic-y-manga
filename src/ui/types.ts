/** Campos serializados desde la tabla local `manga` en el backend Rust. */
export interface Manga {
  id: number;
  path: string;
  title: string;
  genre: string;
  pageCount: number;
  chapterCount: number;
  isSeries: boolean;
  coverPath: string;
  coverEntry: string | null;
  isArchive: boolean;
  favorite: boolean;
  customCover: boolean;
  lastPage: number;
  lastRead: number;
  addedAt: number;
}

/** Volumen o carpeta secundaria de una serie, con progreso independiente. */
export interface Chapter {
  id: number;
  mangaId: number;
  path: string;
  kind: string;
  title: string;
  pageCount: number;
  coverPath: string;
  coverEntry: string | null;
  lastPage: number;
  lastRead: number;
}

/** Imagen suelta o entrada indexada dentro de un archivo CBZ/CBR. */
export interface PageSource {
  path: string;
  entry: string | null;
  label: string;
}

/** Datos mínimos para construir la navegación al iniciar la aplicación. */
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
