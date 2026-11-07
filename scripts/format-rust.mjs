import { existsSync } from "node:fs";
import { homedir, platform } from "node:os";
import { delimiter, join } from "node:path";
import { spawnSync } from "node:child_process";

// Busca Cargo en rustup aunque su carpeta bin no esté incluida en PATH.
const executable = platform() === "win32" ? "cargo.exe" : "cargo";
const candidates = [
  process.env.CARGO,
  join(homedir(), ".cargo", "bin", executable),
  ...(process.env.PATH ?? "").split(delimiter).map((directory) => join(directory, executable)),
].filter(Boolean);
const cargo = candidates.find((candidate) => existsSync(candidate));

if (!cargo) {
  console.error("No encuentro Cargo. Instala Rust con rustup o añade ~/.cargo/bin al PATH.");
  process.exit(1);
}

const checkOnly = process.argv.includes("--check");
const args = ["fmt", "--all", ...(checkOnly ? ["--", "--check"] : [])];
const result = spawnSync(cargo, args, {
  cwd: join(import.meta.dirname, "..", "src-tauri"),
  stdio: "inherit",
});

if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}
process.exit(result.status ?? 1);
