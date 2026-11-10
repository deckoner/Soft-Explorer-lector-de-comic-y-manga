import { readFile } from "node:fs/promises";
import { join } from "node:path";

const root = join(import.meta.dirname, "..");

const readJson = async (path) => JSON.parse(await readFile(join(root, path), "utf8"));
const readText = (path) => readFile(join(root, path), "utf8");

const [packageJson, packageLock, tauriConfig, cargoToml, cargoLock] = await Promise.all([
  readJson("package.json"),
  readJson("package-lock.json"),
  readJson("src-tauri/tauri.conf.json"),
  readText("src-tauri/Cargo.toml"),
  readText("src-tauri/Cargo.lock"),
]);

const cargoTomlVersion = cargoToml.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1];
const cargoPackageName = cargoToml.match(/^\[package\][\s\S]*?^name\s*=\s*"([^"]+)"/m)?.[1];
const cargoLockVersion = cargoLock.match(
  /^\[\[package\]\]\s*\nname\s*=\s*"soft-explorer"\s*\nversion\s*=\s*"([^"]+)"/m,
)?.[1];

const versions = {
  "package.json": packageJson.version,
  "package-lock.json": packageLock.version,
  "src-tauri/tauri.conf.json": tauriConfig.version,
  "src-tauri/Cargo.toml": cargoTomlVersion,
  "src-tauri/Cargo.lock": cargoLockVersion,
};
const uniqueVersions = new Set(Object.values(versions));

if (Object.values(versions).some((version) => !version) || uniqueVersions.size !== 1) {
  console.error("Las versiones de publicación no coinciden:");
  for (const [file, version] of Object.entries(versions)) {
    console.error(`  ${file}: ${version ?? "no encontrada"}`);
  }
  process.exit(1);
}

const expectedUpgradeCode = "8f92cf54-fee5-5b0e-81b6-8d036104a15d";
const releaseIdentityIsStable =
  tauriConfig.productName === "Soft Explorer" &&
  tauriConfig.identifier === "local.softexplorer.manga" &&
  tauriConfig.mainBinaryName === "soft-explorer" &&
  cargoPackageName === "soft-explorer" &&
  tauriConfig.bundle?.windows?.nsis?.installMode === "currentUser" &&
  tauriConfig.bundle?.windows?.wix?.upgradeCode === expectedUpgradeCode;

if (!releaseIdentityIsStable) {
  console.error(
    "La identidad del instalador ha cambiado. Revisa el nombre, identificador y código de actualización antes de publicar.",
  );
  process.exit(1);
}

console.log(`Versiones de publicación coherentes: ${packageJson.version}`);
