# Runs package.ps1 for the git hooks, one run at a time and again if HEAD moved during the build, publishes each clean package to the website,
# then releases it on GitHub with the website's notes, where the Nexus Mods workflow picks it up.
$Root = Split-Path $PSScriptRoot -Parent
$Log = "$Root\dist\package.log"
$running = New-Object Threading.Mutex($false, "Local\package-$($Root -replace '[\\:]', '_')")
try { if (-not $running.WaitOne(0)) { exit } } catch [Threading.AbandonedMutexException] {}
New-Item -ItemType Directory -Force "$Root\dist" | Out-Null

function Publish-GitHubRelease($commit, $version, $zip) {
    $site = Invoke-WebRequest -UseBasicParsing https://twistedskyrim.com/downloads.json
    $latest = ([Text.Encoding]::UTF8.GetString($site.RawContentStream.ToArray()) | ConvertFrom-Json).projects | Where-Object id -eq runtime-extender | ForEach-Object latest
    $notes = Join-Path ([IO.Path]::GetTempPath()) "skyrim-rs-release-$version.md"
    [IO.File]::WriteAllText($notes, $(if ($latest.version -eq $version) { $latest.notes } else { '' }), [Text.UTF8Encoding]::new($false))
    git -C $Root push origin "${commit}:refs/tags/v$version"
    if ($LASTEXITCODE -eq 0) { gh release create "v$version" $zip --repo TwistedModding/skyrim-rs --verify-tag --title "Skyrim Runtime Extender $version" --notes-file $notes }
    Remove-Item $notes
}

do {
    $head = git -C $Root rev-parse HEAD
    cmd /c "powershell -NoProfile -ExecutionPolicy Bypass -File `"$PSScriptRoot\package.ps1`" > `"$Log`" 2>&1"
    if ((Get-Content $Log -Tail 1) -match '^.*\\skyrim-runtime-extender-(\d+\.\d+\.\d+)\.zip$') {
        $zip = $Matches[0]
        $version = $Matches[1]
        cmd /c "powershell -NoProfile -ExecutionPolicy Bypass -File `"$Root\..\Website\scripts\Publish-Download.ps1`" -Project runtime-extender -Version $version -Archive `"$zip`" -NotesFromRepo `"$Root`" -Announce >> `"$Log`" 2>&1"
        if ($LASTEXITCODE -eq 0) { Publish-GitHubRelease $head $version $zip *>&1 | Add-Content $Log }
    }
} while ((git -C $Root rev-parse HEAD) -ne $head)
