# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# Writes THIRD-PARTY-NOTICES.txt, the license notices of every crate linked into the packaged DLLs, from cargo-about
# (cargo install --locked cargo-about --features cli) with about.toml. -Check only confirms that the file lists exactly
# the crates cargo tree resolves for those DLLs, so packaging needs neither cargo-about nor the network.
param([switch]$Check)
$ErrorActionPreference = "Stop"
$Root = Split-Path $PSScriptRoot -Parent
$Out = "$Root\THIRD-PARTY-NOTICES.txt"
$Packages = "host", "timing", "standalone"

if (-not $Check) {
    $crates = @{}
    $texts = New-Object 'Collections.Generic.Dictionary[string,object]'
    $json = New-TemporaryFile
    try {
        foreach ($p in $Packages) {
            # cargo-about only warns when a clarification's checksum or fetch fails, then falls back to what it detects.
            $log = cmd /c "cargo about generate --fail --locked --format json -c `"$Root\about.toml`" -m `"$Root\crates\$p\Cargo.toml`" -o `"$json`" 2>&1"
            if ($LASTEXITCODE -ne 0 -or $log -match "failed to validate") { $log | Write-Host; throw "cargo about failed for $p" }
            $about = [IO.File]::ReadAllText($json) | ConvertFrom-Json
            foreach ($c in $about.crates) { $crates["$($c.package.name) $($c.package.version)"] = $c.license }
            foreach ($l in $about.licenses) {
                $users = $l.used_by | ForEach-Object { "$($_.crate.name) $($_.crate.version)" }
                $text = ($l.text -replace "`r`n", "`n" -replace '^\uFEFF').Trim("`n")
                if ($text -match "<year>|<copyright holders>") { throw "cargo-about fell back to the bare $($l.id) template for $($users -join ', '); clarify it in about.toml" }
                if (-not $texts.ContainsKey($text)) { $texts[$text] = @{ Id = $l.id; Users = @{} } }
                foreach ($u in $users) { $texts[$text].Users[$u] = $true }
            }
        }
    } finally { Remove-Item $json }

    $rule = "=" * 100
    $doc = @(
        "THIRD-PARTY NOTICES"
        ""
        "Skyrim Runtime Extender's DLLs (skyrim_runtime_extender.dll, !!!skyrim-runtime-extender-timing.dll and"
        "optional-standalone\dinput8.dll) contain the open-source components listed below. Skyrim Runtime Extender itself"
        "is licensed under LICENSE.txt. Each component is listed with its license expression; one offered under a choice"
        "of licenses is used under the license whose text below names it."
    )
    if (@($crates.Keys) -match "^minhook ") {
        $doc += "", "The minhook crate compiles in the MinHook C library by Tsuda Kageyu and the Hacker Disassembler Engine by",
            "Vyacheslav Patkov, under the BSD-2-Clause notice listed for minhook."
    }
    $doc += "", "COMPONENTS", ""
    $doc += $crates.Keys | Sort-Object | ForEach-Object { "${_}: $($crates[$_])" }
    $doc += "", "LICENSE TEXTS"
    $sections = $texts.GetEnumerator() | ForEach-Object {
        [pscustomobject]@{ Id = $_.Value.Id; Users = @($_.Value.Users.Keys | Sort-Object); Text = $_.Key }
    } | Sort-Object Id, { $_.Users[0] }
    foreach ($s in $sections) { $doc += "", $rule, "$($s.Id), used by $($s.Users -join ', ')", $rule, "", $s.Text }
    [IO.File]::WriteAllText($Out, ($doc -join "`n") + "`n", (New-Object Text.UTF8Encoding $false))
}

$selection = $Packages | ForEach-Object { "-p", $_ }
$tree = cargo tree --manifest-path "$Root\Cargo.toml" @selection -e normal --target x86_64-pc-windows-msvc --prefix none --format "{p}"
if ($LASTEXITCODE -ne 0) { throw "cargo tree failed" }
# Workspace crates print with their path, and the project's own license covers them.
$linked = $tree | Where-Object { $_ -and $_ -notmatch "\([A-Za-z]:\\" } | ForEach-Object { $_ -replace " \(.*$" -replace " v(\d)", ' $1' } | Sort-Object -Unique
$notices = Get-Content $Out
$listed = $notices[[array]::IndexOf($notices, "COMPONENTS")..[array]::IndexOf($notices, "LICENSE TEXTS")] | Where-Object { $_ -match "^(\S+ \d\S*): " } | ForEach-Object { $Matches[1] } | Sort-Object -Unique
$diff = Compare-Object $linked $listed
if ($diff) {
    $lines = $diff | ForEach-Object { "  $($_.InputObject) " + $(if ($_.SideIndicator -eq "<=") { "is linked but not listed" } else { "is listed but not linked" }) }
    throw "THIRD-PARTY-NOTICES.txt does not match the linked crates; run scripts\notices.ps1:`n$($lines -join "`n")"
}
