param(
  [Parameter(Mandatory = $true)]
  [string]$InstallRoot,
  [switch]$Remove
)

$ErrorActionPreference = "Stop"
$target = [System.IO.Path]::GetFullPath($InstallRoot).TrimEnd("\")
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$entries = @()
if ($userPath) {
  $entries = $userPath.Split(";", [StringSplitOptions]::RemoveEmptyEntries)
}
$remaining = @($entries | Where-Object {
  $_.Trim().TrimEnd("\") -ne $target
})
if (-not $Remove) {
  $remaining += $target
}
[Environment]::SetEnvironmentVariable("Path", ($remaining -join ";"), "User")
