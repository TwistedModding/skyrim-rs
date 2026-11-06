# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# Runs every suite of scripts/verify.ps1 under one launch mode on one runtime and writes one line per suite to
# <OutRoot>\summary.txt, keeping each run's logs and results.json in <OutRoot>\<suite>. A suite that fails is retried
# once (focus loss can stall a scenario); one that cannot run on the runtime or whose plugins the loader refused is not.
param(
    [ValidateSet("skse", "skse-proxy", "standalone")][string]$Launch = "skse",
    [string[]]$Suites = @("forms", "actors", "events", "papyrus", "render", "shadows", "frame", "animation", "bone", "ui", "input", "save", "audio", "overlay", "framework", "hooks", "reload", "compat", "legacy", "gui"),
    [string]$Runtime = "1.7.104",
    [string]$GameDir = "",
    [switch]$WaitForGame,
    [string]$OutRoot = "verify-all-$Runtime-$Launch"
)
# Continue: the child's stderr (cargo's progress) would otherwise stop this script in Windows PowerShell.
$ErrorActionPreference = "Continue"
New-Item -ItemType Directory -Force $OutRoot | Out-Null
$summary = Join-Path $OutRoot "summary.txt"
"runtime $Runtime, launch $Launch, $(Get-Date -Format s)" | Out-File -Encoding utf8 $summary
foreach ($suite in $Suites) {
    $out = Join-Path $OutRoot $suite
    $code = 1
    for ($try = 1; $try -le 2 -and $code -eq 1; $try++) {
        Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue
        & powershell -NoProfile -ExecutionPolicy Bypass -File "$PSScriptRoot\verify.ps1" -Suite $suite -Launch $Launch -Runtime $Runtime $(if ($GameDir) { "-GameDir"; $GameDir }) $(if ($WaitForGame) { "-WaitForGame" }) -OutDir $out *> (Join-Path $OutRoot "$suite.txt")
        $code = $LASTEXITCODE
    }
    $results = Join-Path $out "results.json"
    $line = if (Test-Path $results) {
        $r = Get-Content $results -Raw | ConvertFrom-Json
        "$($r.verdict), $($r.pass) pass, $($r.fail) fail, $($r.notRun) not run$(if ($r.reason) { ": $($r.reason)" })"
    } else { "no results" }
    $diffs = @(Get-Content (Join-Path $OutRoot "$suite.txt") | Where-Object { $_ -match '^(DIFF|FAIL|no clean) ' })
    "$suite`: exit $code after $($try - 1) run(s), $line $($diffs -join '; ')" | Out-File -Append -Encoding utf8 $summary
}
Get-Content $summary
