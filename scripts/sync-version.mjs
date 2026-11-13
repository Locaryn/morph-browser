// Aligne la version de l'extension sur celle du morph (morph.json).
//
// Chrome n'accepte que des nombres dans `version` : « 0.1.0-beta.17 » devient
// « 0.1.0.17 », et le libellé complet va dans `version_name`, celui que la
// barre latérale et Locaryn affichent. Ainsi la personne voit tout de suite si
// son extension est celle du morph installé.
//
//   node scripts/sync-version.mjs          écrit les deux manifestes
//   node scripts/sync-version.mjs --check  échoue s'ils ne sont pas alignés (CI)
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const racine = join(dirname(fileURLToPath(import.meta.url)), "..");
const verifier = process.argv.includes("--check");

/** « 0.1.0-beta.17 » → « 0.1.0.17 » ; « 0.2.0 » reste « 0.2.0 ». */
function versionChrome(version) {
  const m = /^(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z]+\.(\d+))?$/.exec(version);
  if (!m) throw new Error(`version du morph illisible : ${version}`);
  const [, maj, min, cor, pre] = m;
  return pre === undefined ? `${maj}.${min}.${cor}` : `${maj}.${min}.${cor}.${pre}`;
}

const morph = JSON.parse(readFileSync(join(racine, "morph.json"), "utf8"));
const attendue = { version: versionChrome(morph.version), version_name: morph.version };

let ecarts = 0;
for (const nom of ["manifest.json", "manifest.firefox.json"]) {
  const chemin = join(racine, "extension", nom);
  const texte = readFileSync(chemin, "utf8");
  const manifeste = JSON.parse(texte);
  const aligne =
    manifeste.version === attendue.version && manifeste.version_name === attendue.version_name;
  if (aligne) continue;
  if (verifier) {
    console.error(
      `${nom} : ${manifeste.version} / ${manifeste.version_name ?? "—"}, attendu ${attendue.version} / ${attendue.version_name}`,
    );
    ecarts += 1;
    continue;
  }
  // `version_name` juste après `version`, le reste du manifeste inchangé.
  const ordonne = {};
  for (const [cle, valeur] of Object.entries(manifeste)) {
    if (cle === "version_name") continue;
    ordonne[cle] = cle === "version" ? attendue.version : valeur;
    if (cle === "version") ordonne.version_name = attendue.version_name;
  }
  const fin = texte.endsWith("\n") ? "\n" : "";
  writeFileSync(chemin, JSON.stringify(ordonne, null, 2) + fin);
  console.log(`${nom} → ${attendue.version} (${attendue.version_name})`);
}
if (ecarts > 0) {
  console.error("Lancez « node scripts/sync-version.mjs » puis recommittez.");
  process.exit(1);
}
