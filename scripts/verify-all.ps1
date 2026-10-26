# Runs every suite of scripts/verify.ps1 under one launch mode and writes one line per suite to <OutRoot>\summary.txt,
# keeping each run's logs in <OutRoot>\<suite>. A suite that fails is retried once (focus loss can stall a scenario).
param(
    [ValidateSet("skse", "skse-proxy", "standalone")][string]$Launch = "skse",
    [string[]]$Suites = @("forms", "actors", "events", "papyrus", "render", "shadows", "frame", "animation", "bone", "ui", "input", "save", "audio", "overlay", "framework", "hooks", "reload", "compat", "legacy", "gui"),
    [string]$OutRoot = "verify-all-$Launch"
)
# Continue: the child's stderr (cargo's progress) would otherwise stop this script in Windows PowerShell.
$ErrorActionPreference = "Continue"
New-Item -ItemType Directory -Force $OutRoot | Out-Null
$summary = Join-Path $OutRoot "summary.txt"
"launch $Launch, $(Get-Date -Format s)" | Out-File -Encoding utf8 $summary
foreach ($suite in $Suites) {
    $out = Join-Path $OutRoot $suite
    $code = 1
    for ($try = 1; $try -le 2 -and $code -ne 0; $try++) {
        Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue
        & powershell -NoProfile -ExecutionPolicy Bypass -File "$PSScriptRoot\verify.ps1" -Suite $suite -Launch $Launch -OutDir $out *> (Join-Path $OutRoot "$suite.txt")
        $code = $LASTEXITCODE
    }
    $result = Get-Content (Join-Path $OutRoot "$suite.txt") | Where-Object { $_ -match '^(OK|DIFF|FAIL|no clean) ' }
    $diffs = @($result | Where-Object { $_ -notmatch '^OK ' })
    "$suite`: exit $code after $($try - 1) run(s), $(@($result | Where-Object { $_ -match '^OK ' }).Count) OK, $($diffs.Count) not OK $($diffs -join '; ')" | Out-File -Append -Encoding utf8 $summary
}
Get-Content $summary
