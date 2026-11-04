# DevBench comparisons for the audio suite (step indices of audio-steps.json); dot-sourced by verify.ps1.
# The plugin runs its sound sequence on its own thread after the load, then samples AudioCategoryUI's volume while
# DevBench's Papyrus calls play the UISkillIncrease marker and set the category to 0.5 and back; it reports at the save.
$Phases = "postLoadGame", "saveGame"

$lastOwn = Captured (Detail "category_samples") 'last (\d+)$'
$rows = @(
    @{ name = "manager"; notRun = (PluginNotRun "manager");       plugin = (Detail "manager"); devbench = "(plugin only)";
       ok = $plugin["manager"].ok }
    @{ name = "marker";        plugin = (Detail "marker"); devbench = "Sound.Play(UISkillIncrease, player) = $(Returned 4)";
       ok = ($plugin["marker"].ok -and $null -ne (Returned 4)) }
    @{ name = "ids grow";      plugin = "own ids up to $lastOwn"; devbench = "Sound.Play after them returned $(Returned 4)";
       ok = ($null -ne $lastOwn -and [int64](Returned 4) -gt [int64]$lastOwn) }
    @{ name = "one-shot";      plugin = (Detail "one_shot"); devbench = "(plugin only)";
       ok = $plugin["one_shot"].ok }
    @{ name = "handle volume"; plugin = (Detail "handle_volume"); devbench = "(plugin only)";
       ok = $plugin["handle_volume"].ok }
    @{ name = "category";      plugin = "$(Detail "category") | $(Detail "category_samples")"; devbench = "SoundCategory.SetVolume(0.5) called = $((Step 6).called), SetVolume(1.0) called = $((Step 8).called)";
       ok = ($plugin["category"].ok -and $plugin["category_samples"].ok -and (Step 6).called -eq $true -and (Step 8).called -eq $true) }
)
