# Comparisons for the legacy suite (legacy-steps.json: load, wait, save); dot-sourced by verify.ps1. Third-party SKSE
# plugins built for 1.7.x on the Address Library run next to the host for one load and one save, with their own logs
# collected into legacy-logs. Each listed plugin is either loaded (by the standalone loader under -Launch standalone, by
# the script extender otherwise) and proves in its own log that it ran, or refused with the reason the acceptance rules
# give; a plugin missing from $LegacyDir is reported and skipped. The host loads in file name order among them.
$Phases = @()

# The reason a plugin is expected to be refused for; every other listed plugin is expected to load.
$Refusals = @{}
# What a loaded plugin's own log must show (a regex over its lines) to count as having worked.
$Proof = @{
    CrashLogger = '^.*CrashLogger has finished loading.$'
}
$standaloneLog = @(if (Test-Path "$OutDir\skyrim-runtime-extender-standalone.log") { Get-Content "$OutDir\skyrim-runtime-extender-standalone.log" })
function Refused($file) {
    $f = [regex]::Escape($file)
    foreach ($l in $LoaderLog) { if ($l -match "$f.*disabled, (.*)$") { return $Matches[1] } }
    $null
}
function Queried($handle) { @($standaloneLog | Where-Object { $_ -match "^QueryInterface\((\d+)\) from handle $handle`: (.*)$" } | ForEach-Object { "$($Matches[1]) $($Matches[2])" } | Select-Object -Unique) }
function OwnLog($name) { Get-ChildItem "$OutDir\legacy-logs" -File -ErrorAction SilentlyContinue | Where-Object { $_.BaseName -like "$name*" } | Select-Object -First 1 }

$hostHandle = Loaded "skyrim_runtime_extender.dll"
$rows = @()
$before = @()
foreach ($name in $Mod.legacy) {
    $file = "$name.dll"
    if ($name -in $LegacyMissing) {
        $rows += @{ name = "$name (absent)"; plugin = "not in $LegacyDir"; devbench = "skipped"; ok = $true }
        continue
    }
    $handle = Loaded $file
    $refused = Refused $file
    if ($Refusals[$name]) {
        $rows += @{ name = $name; plugin = "${LoaderLogName}: refused, $refused"; devbench = "expected: $($Refusals[$name])"; ok = ($refused -eq $Refusals[$name] -and $handle -eq $null) }
        continue
    }
    $log = OwnLog $name
    $lines = @(if ($log) { Get-Content $log.FullName })
    $proof = @($lines | Where-Object { $_ -match $Proof[$name] })
    $queried = if ($Launch -eq "standalone") { "; queried $((Queried $handle) -join ', ')" } else { "" }
    $rows += @{ name = $name; plugin = "${LoaderLogName}: handle $handle$queried$(if ($refused) { "; refused, $refused" })"; devbench = "$(if ($log) { "$($log.Name), $($lines.Count) lines: $($proof | Select-Object -Last 2)" } else { 'no log of its own' })";
                ok = ($handle -ne $null -and $proof.Count -gt 0) }
    if ($handle -ne $null -and [string]::CompareOrdinal($file.ToUpperInvariant(), "SKYRIM_RS.DLL") -lt 0) { $before += $handle }
}
$rows += @{ name = "host after them"; plugin = "(host log) $(@($HostLog -match '^hosted by ') -join ' | ')"; devbench = "${LoaderLogName}: skyrim_runtime_extender.dll handle $hostHandle, plugins before it by name: handles $($before -join ', ')";
            ok = ((Hosted) -and $hostHandle -ne $null -and @($before | Where-Object { $_ -gt $hostHandle }).Count -eq 0) }
$dialogs = @(if (Test-Path "$OutDir\dialogs.txt") { Get-Content "$OutDir\dialogs.txt" })
$rows += @{ name = "no message box"; plugin = "message boxes closed by devbench.ps1: $($dialogs.Count)"; devbench = ($dialogs -join ' | '); ok = ($dialogs.Count -eq 0) }
$rows += @{ name = "save trigger"; plugin = "legacy logs collected: $(@(Get-ChildItem "$OutDir\legacy-logs" -File -ErrorAction SilentlyContinue).Name -join ', ')"; devbench = "waitFor saveGame satisfied: $(($steps | Where-Object { $_.index -eq 5 }).satisfied)";
            ok = (($steps | Where-Object { $_.index -eq 5 }).satisfied -eq $true) }
