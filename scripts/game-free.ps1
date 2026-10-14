# Throws unless the shared Steam install is free: no SkyrimSE running (the RE session launches the same copy) and no
# live process holding %TEMP%\modding-toolchain\game.lock (the toolchain swaps the user INIs and Plugins.txt under it).
if (Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue) { throw "SkyrimSE is already running" }
$lock = Join-Path $env:TEMP "modding-toolchain\game.lock"
if (Test-Path $lock) {
    $holder = (Get-Content $lock -Raw).Trim()
    if ($holder -match '^\d+$' -and (Get-Process -Id ([int]$holder) -ErrorAction SilentlyContinue)) { throw "$lock is held by live process $holder" }
}
