# Throws unless the shared Steam install is free, and takes the modding toolchain's game lock for this process:
# %TEMP%\modding-toolchain\game.lock holds the holder's PID, created only if absent, a dead holder's lock is stale.
# The RE session takes no lock, so a running SkyrimSE also counts as busy. Returns whether the lock was taken now;
# the caller that took it gives it back with -Release, and a script run inside one that holds it takes nothing.
param([switch]$Release)
$lock = Join-Path $env:TEMP "modding-toolchain\game.lock"
$holder = { (Get-Content $lock -Raw -ErrorAction SilentlyContinue) -as [string] }
if ($Release) {
    if ((& $holder).Trim() -eq "$PID") { Remove-Item $lock -ErrorAction SilentlyContinue }
    return
}
if ((& $holder).Trim() -eq "$PID") { return $false }
if (Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue) { throw "SkyrimSE is already running" }
New-Item -ItemType Directory -Force (Split-Path $lock) | Out-Null
for ($try = 0; ; $try++) {
    try {
        $file = [IO.File]::Open($lock, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
        $bytes = [Text.Encoding]::ASCII.GetBytes("$PID")
        $file.Write($bytes, 0, $bytes.Length)
        $file.Close()
        break
    } catch [IO.IOException] {
        $other = (& $holder).Trim()
        if ($other -match '^\d+$' -and (Get-Process -Id ([int]$other) -ErrorAction SilentlyContinue)) { throw "$lock is held by live process $other" }
        # An empty file is a holder still writing its PID; give it a moment before calling it stale.
        if (-not $other -and $try -lt 5) { Start-Sleep -Milliseconds 200; continue }
        if ($try -ge 20) { throw "$lock could not be taken" }
        Remove-Item $lock -ErrorAction SilentlyContinue
    }
}
if (Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue) {
    Remove-Item $lock -ErrorAction SilentlyContinue
    throw "SkyrimSE is already running"
}
return $true
