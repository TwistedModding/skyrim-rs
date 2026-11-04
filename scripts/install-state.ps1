# One line per file in a game folder (path, size, time), to compare before and after a run. DevBench rewrites its own
# three files at every start.
param([Parameter(Mandatory)][string]$GameDir)
$start = $GameDir.Length + 1
Get-ChildItem $GameDir -Recurse -File -Force | ForEach-Object {
    $path = $_.FullName.Substring($start)
    if ($path -notmatch '^Data\\SKSE\\Plugins\\devbench\\(config|runtime|mcp-bridge)\.json$') { "$path|$($_.Length)|$($_.LastWriteTimeUtc.Ticks)" }
} | Sort-Object
