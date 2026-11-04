# DevBench comparisons for the overlay suite (step indices of overlay-steps.json); dot-sourced by verify.ps1, which runs
# the plugin in mode overlay. F11 opens the overlay, W is held for 2 s while it is open, Tab and Enter, Space and the
# left mouse button click the suite's button, the plugin types "skyrim" into its field with SendInput (only while the
# game's window is in the foreground; otherwise typed and drawn read as not run), F11 closes it
# and W is tapped once; the plugin reports at the save and leaves the frame with the typed text as a bitmap, kept here
# as a half-size JPEG.
$Phases = "load", "dataLoaded", "postLoadGame", "saveGame"

function Pos($i) { [double](Captured (Lines $i) '>>\s*(-?[\d.]+)') }
$moved = [math]::Sqrt([math]::Pow((Pos 12) - (Pos 4), 2) + [math]::Pow((Pos 13) - (Pos 5), 2))

$bmp = "$SkseLogs\skyrim-rs-test-overlay.bmp"
if (Test-Path $bmp) {
    Add-Type -AssemblyName System.Drawing
    $img = [System.Drawing.Image]::FromFile($bmp)
    $half = New-Object System.Drawing.Bitmap $img, ([int]($img.Width / 2)), ([int]($img.Height / 2))
    $half.Save("$OutDir\overlay-$Runtime.jpg", [System.Drawing.Imaging.ImageFormat]::Jpeg)
    $half.Dispose(); $img.Dispose()
    Remove-Item $bmp
}

$rows = @(
    @{ name = "W hidden";     plugin = (Detail "capture"); devbench = "player moved $([math]::Round($moved, 2)) units during the 2 s W hold with the overlay open (x $(Pos 4) -> $(Pos 12), y $(Pos 5) -> $(Pos 13))";
       ok = ($plugin["capture"].ok -and (Lines 4) -ne "" -and (Lines 12) -ne "" -and $moved -lt 1) }
    @{ name = "clicks";       plugin = (Detail "clicks"); devbench = "input sequence Tab, Enter, Space, mouseLeft ok = $((Step 14) -ne $null)";
       ok = ($plugin["clicks"].ok -and (Step 14) -ne $null) }
    @{ name = "typed";        plugin = (Detail "typed"); devbench = "(no DevBench counterpart: its keys never become characters)"; notRun = (PluginNotRun "typed");
       ok = $plugin["typed"].ok }
    @{ name = "drawn";        plugin = (Detail "drawn"); devbench = "frame kept as overlay-$Runtime.jpg: $(Test-Path "$OutDir\overlay-$Runtime.jpg")"; notRun = (PluginNotRun "drawn");
       ok = ($plugin["drawn"].ok -and (Test-Path "$OutDir\overlay-$Runtime.jpg")) }
    @{ name = "closed";       plugin = (Detail "closed"); devbench = "input tap F11 ok = $((Step 16) -ne $null)";
       ok = ($plugin["closed"].ok -and (Step 16) -ne $null) }
    @{ name = "save trigger"; plugin = ($summaries -match 'overlay saveGame') -join ' '; devbench = "waitFor saveGame satisfied: $(($steps | Where-Object { $_.index -eq 21 }).satisfied)";
       ok = (($steps | Where-Object { $_.index -eq 21 }).satisfied -eq $true) }
)
