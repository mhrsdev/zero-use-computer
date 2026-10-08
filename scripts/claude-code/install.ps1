<#
Register computer-use-mcp with Claude Code (Windows).

  .\install.ps1                        # user scope
  .\install.ps1 -Scope project         # only for the current project
  .\install.ps1 -NoRegister           # only copy the program (for the plugin route)
  .\install.ps1 -Uninstall

Run it from the folder that contains computer-use-mcp.exe
(if scripts are blocked: powershell -ExecutionPolicy Bypass -File .\install.ps1).
#>
param(
  [ValidateSet('local', 'project', 'user')] [string] $Scope = 'user',
  [string] $Name = 'computer-use',
  [switch] $NoRegister,
  [switch] $Uninstall
)
$ErrorActionPreference = 'Stop'
# Newer Claude Code keeps "computer-use" for itself ("this name is
# reserved"): then the program goes in under this name, as the panel's
# Connect page does.
$AltName = 'zero-use-computer'
$NameGiven = $PSBoundParameters.ContainsKey('Name')

if ((-not $NoRegister) -and (-not (Get-Command claude -ErrorAction SilentlyContinue))) {
  throw 'claude (Claude Code) is not on PATH. Install it first: https://docs.claude.com/claude-code'
}
if ($Uninstall) {
  $names = @($Name)
  if (-not $NameGiven) { $names += $AltName }
  $removed = $false
  # A failed removal's error output must not stop the script: its exit code tells.
  $ErrorActionPreference = 'Continue'
  foreach ($n in $names) {
    claude mcp remove $n --scope $Scope 2>&1 | Out-Null
    if ($LASTEXITCODE -eq 0) { Write-Host "Removed '$n' ($Scope scope)."; $removed = $true }
  }
  if (-not $removed) {
    Write-Host "Nothing removed: '$Name' wasn't found in the $Scope scope (see claude mcp list)."
    exit 1
  }
  return
}

$source = Join-Path $PSScriptRoot 'computer-use-mcp.exe'
if (-not (Test-Path $source)) { throw "computer-use-mcp.exe not found next to this script ($source)" }

# Install the program in a stable place, so this folder can be moved or deleted
# and the plugin route (which runs it from there) finds it. ($HOME is a
# read-only PowerShell variable, hence another name.)
$cuHome = if ($env:COMPUTER_USE_HOME) { $env:COMPUTER_USE_HOME } else { Join-Path $env:USERPROFILE '.computer-use' }
$binDir = Join-Path $cuHome 'bin'
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
$exe = Join-Path $binDir 'computer-use-mcp.exe'
# Old copies left by earlier upgrades (see below), now that nothing runs them.
Get-ChildItem -Path $binDir -Filter 'computer-use-mcp.exe.old*' -ErrorAction SilentlyContinue |
  Remove-Item -Force -ErrorAction SilentlyContinue
try {
  Copy-Item -Force $source $exe
} catch {
  # A running server locks its program, but it may be renamed: move it
  # aside (it is removed next time) and put the new one in its place.
  $old = "$exe.old-$(Get-Date -Format yyyyMMddHHmmss)"
  Rename-Item -Path $exe -NewName (Split-Path $old -Leaf)
  Copy-Item -Force $source $exe
  Write-Host 'The server was running: the new version is used from its next start.'
}
Write-Host "Installed: $exe"
if ($env:COMPUTER_USE_HOME -and ($binDir -ne (Join-Path (Join-Path $env:USERPROFILE '.computer-use') 'bin'))) {
  Write-Host "Note: COMPUTER_USE_HOME is set, so the program is in $binDir; the plugin zip looks in %USERPROFILE%\.computer-use\bin, so register it with this script instead (without -NoRegister)."
}
if ($NoRegister) { Write-Host 'Done (not registered with Claude Code). Upload the plugin zip, or run this again without -NoRegister.'; return }

$serverArgs = @('serve')

Write-Host '== doctor =='
try { & $exe doctor } catch { Write-Host '(doctor reported problems; see docs\CONNECT.md)' }

# Make the settings file easy to find (and edit) if it doesn't exist yet.
try {
  $cfg = (& $exe config path | Select-Object -First 1)
  if ($cfg -and -not (Test-Path $cfg)) { & $exe config init | Out-Null }
  if ($cfg) { Write-Host "Settings file: $cfg" }
} catch {}

# A native command's error output must not stop the script here (Windows
# PowerShell turns it into an error under 'Stop'): its exit code tells.
function Add-ToClaude([string] $n) {
  $ErrorActionPreference = 'Continue'
  try { claude mcp remove $n --scope $Scope 2>&1 | Out-Null } catch {}
  # Each line as text (an error line would print with PowerShell's own notes).
  $out = (claude mcp add --scope $Scope $n -- $exe @serverArgs 2>&1 | ForEach-Object { "$_" } | Out-String).Trim()
  return @{ Code = $LASTEXITCODE; Out = $out }
}
$added = Add-ToClaude $Name
if ($added.Code -ne 0 -and -not $NameGiven -and $added.Out -match 'reserved') {
  Write-Host "Claude Code keeps the name '$Name' for itself: adding the program as '$AltName'."
  $Name = $AltName
  $added = Add-ToClaude $Name
}
if ($added.Out) { Write-Host $added.Out }
if ($added.Code -ne 0) {
  Write-Host "Could not add '$Name' to Claude Code (see above)."
  exit $added.Code
}
Write-Host ''
Write-Host "Added '$Name' ($Scope scope). Check with:  claude mcp list"
Write-Host 'Apps running as administrator cannot be controlled from a normal process.'
Write-Host 'Give the agent the skills in skills\ (computer-use, computer-use-security, and computer-use-design for design work): the server asks no one for permission, the security skill sets the rules.'
