import { convertFileSrc } from "@tauri-apps/api/core";

export function formatCount(value: number): string {
  return new Intl.NumberFormat("es-ES").format(value);
}

export function imageSource(path: string): string {
  return convertFileSrc(path);
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
