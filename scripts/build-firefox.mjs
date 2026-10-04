// Assemble `extension-firefox/` : les mêmes fichiers que `extension/`, avec le
// manifeste Firefox. Les navigateurs Chromium exigent un service worker en
// arrière-plan, Firefox une page d'événements : une seule clé change, d'où ce
// script plutôt que deux copies du code à garder alignées.
import { cpSync, mkdirSync, readdirSync, rmSync, copyFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const racine = join(dirname(fileURLToPath(import.meta.url)), "..");
const source = join(racine, "extension");
const cible = join(racine, "extension-firefox");

rmSync(cible, { recursive: true, force: true });
mkdirSync(cible, { recursive: true });
for (const nom of readdirSync(source)) {
  if (nom === "manifest.json" || nom === "manifest.firefox.json") continue;
  cpSync(join(source, nom), join(cible, nom), { recursive: true });
}
copyFileSync(join(source, "manifest.firefox.json"), join(cible, "manifest.json"));
console.log(`extension-firefox/ assemblée depuis ${source}`);
