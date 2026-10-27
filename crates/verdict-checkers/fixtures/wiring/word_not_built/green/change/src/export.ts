export function exportCsv(rows: string[][]): string {
  return rows.map((r) => r.join(",")).join("\n");
}
