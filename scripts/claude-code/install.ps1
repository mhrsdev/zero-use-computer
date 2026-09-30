<#
Register computer-use-mcp with Claude Code (Windows).

  .\install.ps1                        # user scope, approvals asked per app
  .\install.ps1 -Approval allow-all    # every app not blocked by policy
  .\install.ps1 -Scope project         # only for the current project
  .\install.ps1 -NoRegister           # only copy the program (for the plugin route)
  .\install.ps1 -Uninstall

Run it from the folder that contains computer-use-mcp.exe
(if scripts are blocked: powershell -ExecutionPolicy Bypass -File .\install.ps1).
#>
param(
  [ValidateSet('local', 'project', 'user')] [string] $Scope = 'user',
  [ValidateSet('', 'prompt', 'allowlist', 'allow-all')] [string] $Approval = '',
  [string] $Name = 'computer-use',
  [switch] $NoRegister,
  [switch] $Uninstall
)
$ErrorActionPreference = 'Stop'

if ((-not $NoRegister) -and (-not (Get-Command claude -ErrorAction SilentlyContinue))) {
  throw 'claude (Claude Code) is not on PATH. Install it first: https://docs.claude.com/claude-code'
}
if ($Uninstall) {
  claude mcp remove $Name --scope $Scope
  Write-Host "Removed '$Name'."
  return
}

$source = Join-Path $PSScriptRoot 'computer-use-mcp.exe'
if (-not (Test-Path $source)) { throw "computer-use-mcp.exe not found next to this script ($source)" }

# Install the program in a stable place, so this folder can be moved or deleted
# and the plugin route (which runs it from there) finds it.
$home = if ($env:COMPUTER_USE_HOME) { $env:COMPUTER_USE_HOME } else { Join-Path $env:USERPROFILE '.computer-use' }
$binDir = Join-Path $home 'bin'
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
$exe = Join-Path $binDir 'computer-use-mcp.exe'
Copy-Item -Force $source $exe
Write-Host "Installed: $exe"
if ($NoRegister) { Write-Host 'Done (not registered with Claude Code). Upload the plugin zip, or run this again without -NoRegister.'; return }

$serverArgs = @()
if ($Approval) { $serverArgs += @('--approval', $Approval) }
$serverArgs += 'serve'

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
