export function formatCount(value: number): string {
  return new Intl.NumberFormat("es-ES").format(value);
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
