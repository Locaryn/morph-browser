<#
.SYNOPSIS
  Installe morph-browser (serveur MCP de controle du navigateur + extension) et prepare la configuration a coller.

.DESCRIPTION
  1. telecharge la derniere release depuis github.com/Locaryn/morph-browser ;
  2. verifie son empreinte SHA-256 (SHA256SUMS.txt publie avec la release) ;
  3. l'installe dans %LOCALAPPDATA%\Locaryn\morph-browser (aucun droit administrateur) ;
  4. cree le code d'appairage de l'extension (propre a votre machine) ;
  5. affiche le bloc MCP a coller dans votre outil (" + MCP " dans Freebuff, par exemple) et le copie
     dans le presse-papiers. Avec -Client, l'ajoute lui-meme a la configuration d'un outil, apres sauvegarde.

  Rien n'est modifie en dehors du dossier d'installation sans que vous l'ayez demande avec -Client.

.EXAMPLE
  irm https://raw.githubusercontent.com/Locaryn/morph-browser/main/install.ps1 | iex

.EXAMPLE
  & ([scriptblock]::Create((irm https://raw.githubusercontent.com/Locaryn/morph-browser/main/install.ps1))) -Client antigravity
#>
[CmdletBinding()]
param(
  [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Locaryn\morph-browser'),
  [ValidateSet('none', 'claude', 'antigravity', 'all')][string]$Client = 'none',
  [string]$Version = '',
  [string]$DataDir = (Join-Path $env:APPDATA 'locaryn\morph-browser'),
  [string]$ClaudeConfig = (Join-Path $env:APPDATA 'Claude\claude_desktop_config.json'),
  [string]$AntigravityConfig = (Join-Path $env:USERPROFILE '.gemini\antigravity\mcp_config.json')
)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$repo = 'Locaryn/morph-browser'
$name = 'morph-browser'
$headers = @{ 'User-Agent' = 'locaryn-install' }

function Step($t) { Write-Host "-> $t" -ForegroundColor Green }

# -- 1. La release -----------------------------------------------------------
Step 'Recherche de la derniere version'
if ($Version) {
  $rel = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/tags/$Version" -Headers $headers
} else {
  # /releases/latest ignorerait les preversions : toutes les versions actuelles en sont.
  $rel = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases?per_page=1" -Headers $headers)[0]
}
if (-not $rel) { throw "Aucune release trouvee sur github.com/$repo." }
$zip = $rel.assets | Where-Object { $_.name -like '*windows-x86_64.zip' } | Select-Object -First 1
$sums = $rel.assets | Where-Object { $_.name -eq 'SHA256SUMS.txt' } | Select-Object -First 1
if (-not $zip) { throw "La release $($rel.tag_name) ne contient pas d'archive Windows." }
if (-not $sums) { throw "La release $($rel.tag_name) ne publie pas SHA256SUMS.txt : installation refusee, impossible de verifier le telechargement." }
Write-Host "   version $($rel.tag_name)"

# -- 2. Telechargement et verification ---------------------------------------
$tmp = Join-Path $env:TEMP ('locaryn-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  Step "Telechargement ($([math]::Round($zip.size / 1MB, 1)) Mo)"
  $zipPath = Join-Path $tmp $zip.name
  Invoke-WebRequest $zip.browser_download_url -OutFile $zipPath -UseBasicParsing -Headers $headers
  $sumsPath = Join-Path $tmp 'SHA256SUMS.txt'
  Invoke-WebRequest $sums.browser_download_url -OutFile $sumsPath -UseBasicParsing -Headers $headers

  Step 'Verification SHA-256'
  $ligne = Get-Content $sumsPath | Where-Object { $_ -match [regex]::Escape($zip.name) } | Select-Object -First 1
  if (-not $ligne) { throw "SHA256SUMS.txt ne mentionne pas $($zip.name)." }
  $attendue = ($ligne -split '\s+')[0].ToLower()
  $reelle = (Get-FileHash $zipPath -Algorithm SHA256).Hash.ToLower()
  if ($attendue -ne $reelle) {
    throw "EMPREINTE DIFFERENTE : attendue $attendue, recue $reelle. Rien n'a ete installe."
  }
  Write-Host "   $reelle  (conforme)"

  # -- 3. Installation -------------------------------------------------------
  Step "Installation dans $InstallDir"
  $extrait = Join-Path $tmp 'x'
  Expand-Archive $zipPath -DestinationPath $extrait -Force
  New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
  try {
    Copy-Item (Join-Path $extrait '*') $InstallDir -Recurse -Force
  } catch {
    throw "Copie impossible ($($_.Exception.Message)). Un outil utilise peut-etre deja le serveur : fermez-le (Claude, Antigravity, Freebuff...) puis relancez l'installation."
  }
} finally {
  Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

$exe = Join-Path $InstallDir 'bin\locaryn-browser-mcp.exe'
if (-not (Test-Path $exe) -or -not (Test-Path (Join-Path $InstallDir 'extension\manifest.json'))) { throw 'Installation incomplete : le serveur ou l''extension manque.' }

# -- 4. Code d'appairage (jamais ecrase s'il existe deja) ------------------
# Meme emplacement et meme format que le serveur : il le reutilise tel quel.
$dataDir = $DataDir
$tokenFile = Join-Path $dataDir 'pairing-token'
New-Item -ItemType Directory -Path $dataDir -Force | Out-Null
if (-not (Test-Path $tokenFile) -or (Get-Content $tokenFile -Raw).Trim().Length -lt 32) {
  $octets = New-Object byte[] 20
  $rng = [Security.Cryptography.RandomNumberGenerator]::Create(); $rng.GetBytes($octets); $rng.Dispose()
  $code = -join ($octets | ForEach-Object { $_.ToString('x2') })
  [IO.File]::WriteAllText($tokenFile, $code, (New-Object Text.UTF8Encoding($false)))
}
$code = (Get-Content $tokenFile -Raw).Trim()

# -- 5. Le bloc MCP ----------------------------------------------------------
$entree = [ordered]@{
  command = $exe
  args    = @()
}
# Le serveur cherche le code dans %APPDATA%\locaryn\morph-browser ; ailleurs, il faut le lui dire.
if ($DataDir -ne (Join-Path $env:APPDATA 'locaryn\morph-browser')) {
  $entree.env = [ordered]@{ LOCARYN_EXTENSION_DATA_DIR = $DataDir }
}
$bloc = ([ordered]@{ mcpServers = [ordered]@{ $name = $entree } } | ConvertTo-Json -Depth 6)

function Add-Mcp([string]$chemin, [string]$libelle) {
  if (-not (Test-Path (Split-Path $chemin))) { Write-Warning "$libelle introuvable ($chemin) : ignore."; return }
  $json = if (Test-Path $chemin) { Get-Content $chemin -Raw | ConvertFrom-Json } else { [pscustomobject]@{} }
  if (Test-Path $chemin) { Copy-Item $chemin "$chemin.bak-$(Get-Date -Format yyyyMMddHHmmss)" }
  if (-not $json.PSObject.Properties['mcpServers']) { $json | Add-Member -NotePropertyName mcpServers -NotePropertyValue ([pscustomobject]@{}) }
  $json.mcpServers | Add-Member -NotePropertyName $name -NotePropertyValue ([pscustomobject]$entree) -Force
  [IO.File]::WriteAllText($chemin, ($json | ConvertTo-Json -Depth 20), (New-Object Text.UTF8Encoding($false)))
  Write-Host "   ajoute a $libelle ($chemin) - sauvegarde : $chemin.bak-*"
}

if ($Client -in 'claude', 'all') { Step 'Ajout a Claude Desktop'; Add-Mcp $ClaudeConfig 'Claude Desktop' }
if ($Client -in 'antigravity', 'all') { Step 'Ajout a Antigravity'; Add-Mcp $AntigravityConfig 'Antigravity' }

try { Set-Clipboard -Value $bloc } catch { }

Write-Host ''
Write-Host 'Installe.' -ForegroundColor Green
Write-Host ''
Write-Host 'Bloc MCP a coller dans votre outil (deja copie dans le presse-papiers) :'
Write-Host $bloc -ForegroundColor Cyan
Write-Host ''
Write-Host "  Freebuff     : '+ MCP', collez le bloc."
Write-Host '  Antigravity  : menu des serveurs MCP, configuration brute, collez le bloc (ou relancez avec -Client antigravity).'
Write-Host '  Claude       : relancez avec -Client claude, puis quittez et rouvrez Claude Desktop.'
Write-Host ''
Write-Host 'ETAPE SUIVANTE - installer l''extension dans le navigateur (une seule fois) :' -ForegroundColor Yellow
Write-Host "  Chrome / Edge / Brave / Opera / Vivaldi : page des extensions -> mode developpeur -> 'Charger l'extension non empaquetee' ->"
Write-Host "      $InstallDir\extension"
Write-Host "  Firefox : about:debugging#/runtime/this-firefox -> 'Charger un module complementaire temporaire' ->"
Write-Host "      $InstallDir\extension-firefox\manifest.json   (puis about:addons -> Autorisations -> tous les sites)"
Write-Host "  Puis cliquez l'icone de l'extension, collez ce code d'appairage et 'Enregistrer' :"
Write-Host "      $code" -ForegroundColor Cyan
Write-Host ''
Write-Host 'Securite : le modele agit dans vos onglets deja connectes (messagerie, banque...). Un profil de navigateur'
Write-Host 'dedie est le meilleur reglage. Cadre anime + pastille + bouton Stop sur chaque onglet controle.'
Write-Host "Guide complet : https://github.com/$repo/blob/main/docs/INSTALL-AUTRES-OUTILS-IA.md"
