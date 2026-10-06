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

if ((-not $NoRegister) -and (-not (Get-Command claude -ErrorAction SilentlyContinue))) {
  throw 'claude (Claude Code) is not on PATH. Install it first: https://docs.claude.com/claude-code'
}
if ($Uninstall) {
  # A failing native command doesn't throw: check its exit code.
  claude mcp remove $Name --scope $Scope
  if ($LASTEXITCODE -ne 0) {
    Write-Host "Nothing removed: '$Name' wasn't found in the $Scope scope (see claude mcp list)."
    exit 1
  }
  Write-Host "Removed '$Name' ($Scope scope)."
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

try { claude mcp remove $Name --scope $Scope 2>$null | Out-Null } catch {}
claude mcp add --scope $Scope $Name -- $exe @serverArgs
Write-Host ''
Write-Host "Added '$Name' ($Scope scope). Check with:  claude mcp list"
Write-Host 'Apps running as administrator cannot be controlled from a normal process.'
Write-Host 'Give the agent the skills in skills\ (computer-use, computer-use-security, and computer-use-design for design work): the server asks no one for permission, the security skill sets the rules.'
